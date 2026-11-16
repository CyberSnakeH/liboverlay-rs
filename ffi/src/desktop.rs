//! Desktop integration behind the generic ABI; never loaded into the target.
use super::canvas::{Event, Target, CONFLICT, UNSUPPORTED};
use super::{FAILED, INVALID};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    os::unix::{fs::FileTypeExt, net::UnixStream},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{
        randr::ConnectionExt as _,
        xproto::{AtomEnum, ConnectionExt as _, MapState, WindowClass},
    },
    rust_connection::RustConnection,
};
type Result<T> = std::result::Result<T, (i32, String)>;
fn error(e: impl std::fmt::Display) -> (i32, String) {
    (FAILED, e.to_string())
}
pub fn hyprland_path() -> Option<PathBuf> {
    std::env::var_os("WAYLAND_DISPLAY")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    if sig.is_empty()
        || !sig
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return None;
    }
    let path = PathBuf::from(runtime).join("hypr").join(sig);
    std::fs::metadata(path.join(".socket.sock"))
        .ok()?
        .file_type()
        .is_socket()
        .then_some(path)
}
fn connect(path: &std::path::Path) -> Result<UnixStream> {
    use rustix::{
        event::{poll, PollFd, PollFlags, Timespec},
        net::{connect, socket_with, AddressFamily, SocketAddrUnix, SocketFlags, SocketType},
    };
    let fd = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        None,
    )
    .map_err(error)?;
    let addr = SocketAddrUnix::new(path).map_err(error)?;
    match connect(&fd, &addr) {
        Ok(()) => {}
        Err(rustix::io::Errno::INPROGRESS) => {
            let mut fds = [PollFd::new(&fd, PollFlags::OUT)];
            if poll(
                &mut fds,
                Some(&Timespec {
                    tv_sec: 0,
                    tv_nsec: 150_000_000,
                }),
            )
            .map_err(error)?
                == 0
            {
                return Err((FAILED, "Hyprland connect timeout".into()));
            }
        }
        Err(e) => return Err(error(e)),
    }
    let stream = UnixStream::from(fd);
    if let Some(e) = stream.take_error().map_err(error)? {
        return Err(error(e));
    }
    stream.set_nonblocking(false).map_err(error)?;
    Ok(stream)
}
fn request(command: &str) -> Result<String> {
    let path = hyprland_path().ok_or((UNSUPPORTED, "Hyprland integration unavailable".into()))?;
    let mut stream = connect(&path.join(".socket.sock"))?;
    let deadline = Instant::now() + Duration::from_millis(150);
    let mut sent = 0;
    while sent < command.len() {
        stream
            .set_write_timeout(Some(
                deadline
                    .checked_duration_since(Instant::now())
                    .ok_or((FAILED, "Hyprland write timeout".into()))?,
            ))
            .map_err(error)?;
        let n = stream.write(&command.as_bytes()[sent..]).map_err(error)?;
        if n == 0 {
            return Err((FAILED, "Hyprland closed during request".into()));
        }
        sent += n;
    }
    stream.shutdown(std::net::Shutdown::Write).map_err(error)?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or((FAILED, "Hyprland read timeout".into()))?;
        stream.set_read_timeout(Some(remaining)).map_err(error)?;
        let n = stream.read(&mut buffer).map_err(error)?;
        if n == 0 {
            break;
        }
        if bytes.len() + n > 1024 * 1024 {
            return Err((FAILED, "Hyprland response exceeds 1 MiB".into()));
        }
        bytes.extend_from_slice(&buffer[..n]);
    }
    String::from_utf8(bytes).map_err(error)
}
fn number(value: &Value) -> Option<f64> {
    value.as_f64().filter(|v| v.is_finite())
}

fn decode(
    pid: u32,
    clients: &[u8],
    monitors: &[u8],
) -> std::result::Result<Option<Target>, String> {
    let clients: Value = serde_json::from_slice(clients).map_err(|e| e.to_string())?;
    let monitors: Value = serde_json::from_slice(monitors).map_err(|e| e.to_string())?;
    let clients = clients.as_array().ok_or("Expected a clients array")?;
    let monitors = monitors.as_array().ok_or("Expected a monitors array")?;
    if clients.len() > 8192 || monitors.len() > 256 {
        return Err("Hyprland snapshot exceeds limits".into());
    }
    let mut result = None;
    let mut best_area = 0.0;
    for client in clients
        .iter()
        .filter(|c| c["pid"].as_u64() == Some(u64::from(pid)))
    {
        let Some(monitor) = monitors
            .iter()
            .find(|m| m["id"].as_i64().is_some() && m["id"].as_i64() == client["monitor"].as_i64())
        else {
            continue;
        };
        let Some(name) = monitor["name"]
            .as_str()
            .filter(|n| !n.is_empty() && n.len() < 128 && !n.contains('\0'))
        else {
            continue;
        };
        let mut geometry = Target::default();
        geometry.output[..name.len()].copy_from_slice(name.as_bytes());
        // A window on another workspace may still have mapped=true/visible=true.
        let workspace = client["workspace"]["id"].as_i64();
        let active = monitor["activeWorkspace"]["id"].as_i64();
        let special = monitor["specialWorkspace"]["id"]
            .as_i64()
            .filter(|id| *id != 0);
        let visible = client["mapped"].as_bool() == Some(true)
            && client["hidden"].as_bool() != Some(true)
            && client["visible"].as_bool() != Some(false)
            && monitor["disabled"].as_bool() != Some(true)
            && (client["pinned"].as_bool() == Some(true)
                || (workspace.is_some() && (workspace == active || workspace == special)));
        if !visible {
            if result.is_none() {
                result = Some(geometry);
            }
            continue;
        }
        let Some(scale) = number(&monitor["scale"]).filter(|s| *s > 0.0 && *s <= 16.0) else {
            continue;
        };
        let (Some(mut mw), Some(mut mh), Some(mx), Some(my), Some(x), Some(y), Some(w), Some(h)) = (
            number(&monitor["width"]),
            number(&monitor["height"]),
            number(&monitor["x"]),
            number(&monitor["y"]),
            number(&client["at"][0]),
            number(&client["at"][1]),
            number(&client["size"][0]),
            number(&client["size"][1]),
        ) else {
            continue;
        };
        let transform = monitor["transform"].as_u64().unwrap_or(0);
        if transform > 7 {
            continue;
        }
        if transform % 2 == 1 {
            std::mem::swap(&mut mw, &mut mh);
        }
        mw /= scale;
        mh /= scale;
        if mw <= 0.0 || mh <= 0.0 || w <= 0.0 || h <= 0.0 {
            continue;
        }
        let values = [(x - mx) / mw, (y - my) / mh, w / mw, h / mh];
        if values.iter().any(|v| !v.is_finite() || v.abs() > 16.0) {
            continue;
        }
        let area = w * h;
        if area > best_area {
            geometry.visible = 1;
            geometry.protocol = if client["xwayland"].as_bool() == Some(true) {
                3
            } else {
                1
            };
            [geometry.x, geometry.y, geometry.width, geometry.height] = values.map(|v| v as f32);
            best_area = area;
            result = Some(geometry);
        }
    }
    Ok(result)
}

#[derive(Default)]
pub struct Desktop {
    x11: Option<(RustConnection, usize)>,
}
impl Desktop {
    pub fn query(&mut self, pid: u32, filter: &str) -> Result<Target> {
        if hyprland_path().is_some() {
            let clients = request("j/clients")?;
            let monitors = request("j/monitors")?;
            if let Some(mut t) =
                decode(pid, clients.as_bytes(), monitors.as_bytes()).map_err(error)?
            {
                if !filter.is_empty() && t.output() != filter {
                    t.visible = 0;
                }
                return Ok(t);
            }
            // A known Hyprland session is authoritative for native clients.
            return Ok(Target::default());
        }
        if self.x11.is_none() {
            self.x11 = x11rb::connect(None).ok();
        }
        let (c, screen) = self
            .x11
            .as_ref()
            .ok_or((UNSUPPORTED, "no window tracking integration".into()))?;
        query_x11(c, *screen, pid, filter)
    }
}
fn query_x11(c: &RustConnection, screen: usize, pid: u32, filter: &str) -> Result<Target> {
    let root = c.setup().roots[screen].root;
    let atom = c
        .intern_atom(true, b"_NET_WM_PID")
        .map_err(error)?
        .reply()
        .map_err(error)?
        .atom;
    if atom == 0 {
        return Ok(Target::default());
    }
    let resources = c
        .randr_get_screen_resources_current(root)
        .map_err(error)?
        .reply()
        .map_err(error)?;
    let mut monitors = Vec::new();
    for output in resources.outputs.iter().take(256) {
        let Ok(o) = c
            .randr_get_output_info(*output, resources.config_timestamp)
            .map_err(error)?
            .reply()
        else {
            continue;
        };
        if o.crtc == 0 {
            continue;
        }
        let Ok(crtc) = c
            .randr_get_crtc_info(o.crtc, resources.config_timestamp)
            .map_err(error)?
            .reply()
        else {
            continue;
        };
        if crtc.width > 0 && crtc.height > 0 {
            monitors.push((crtc, String::from_utf8_lossy(&o.name).into_owned()));
        }
    }
    let mut pending = vec![(root, 0)];
    let mut examined = 0;
    let mut best = Target::default();
    let mut area = 0u64;
    while let Some((window, depth)) = pending.pop() {
        if examined >= 8192 {
            break;
        }
        examined += 1;
        let Ok(attributes) = c.get_window_attributes(window).map_err(error)?.reply() else {
            continue;
        };
        if attributes.map_state != MapState::VIEWABLE {
            continue;
        }
        if window != root && attributes.class == WindowClass::INPUT_OUTPUT {
            if let Ok(prop) = c
                .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1)
                .map_err(error)?
                .reply()
            {
                if prop.type_ == u32::from(AtomEnum::CARDINAL)
                    && prop.bytes_after == 0
                    && prop.value32().and_then(|mut v| v.next()) == Some(pid)
                {
                    if let (Ok(g), Ok(p)) = (
                        c.get_geometry(window).map_err(error)?.reply(),
                        c.translate_coordinates(window, root, 0, 0)
                            .map_err(error)?
                            .reply(),
                    ) {
                        let size = u64::from(g.width) * u64::from(g.height);
                        let cx = f32::from(p.dst_x) + f32::from(g.width) / 2.;
                        let cy = f32::from(p.dst_y) + f32::from(g.height) / 2.;
                        if let Some((m, name)) = monitors.iter().find(|(m, n)| {
                            (filter.is_empty() || filter == n)
                                && cx >= f32::from(m.x)
                                && cy >= f32::from(m.y)
                                && cx < f32::from(m.x) + f32::from(m.width)
                                && cy < f32::from(m.y) + f32::from(m.height)
                        }) {
                            if size > area && name.len() < 128 {
                                best = Target {
                                    visible: 1,
                                    protocol: 2,
                                    x: (f32::from(p.dst_x) - f32::from(m.x)) / f32::from(m.width),
                                    y: (f32::from(p.dst_y) - f32::from(m.y)) / f32::from(m.height),
                                    width: f32::from(g.width) / f32::from(m.width),
                                    height: f32::from(g.height) / f32::from(m.height),
                                    ..Default::default()
                                };
                                best.output[..name.len()].copy_from_slice(name.as_bytes());
                                area = size;
                            }
                        }
                    }
                }
            }
        }
        if depth < 8 {
            if let Ok(tree) = c.query_tree(window).map_err(error)?.reply() {
                let room = 8192usize.saturating_sub(examined + pending.len());
                pending.extend(
                    tree.children
                        .into_iter()
                        .rev()
                        .take(room)
                        .map(|w| (w, depth + 1)),
                );
            }
        }
    }
    Ok(best)
}
pub fn active_pid() -> Result<u32> {
    if hyprland_path().is_some() {
        let v: Value = serde_json::from_str(&request("j/activewindow")?).map_err(error)?;
        return Ok(v["pid"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0));
    }
    let (c, screen) = x11rb::connect(None)
        .map_err(|_| (UNSUPPORTED, "active window integration unavailable".into()))?;
    let root = c.setup().roots[screen].root;
    let atom = c
        .intern_atom(true, b"_NET_ACTIVE_WINDOW")
        .map_err(error)?
        .reply()
        .map_err(error)?
        .atom;
    if atom == 0 {
        return Ok(0);
    }
    let p = c
        .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)
        .map_err(error)?
        .reply()
        .map_err(error)?;
    let window = p.value32().and_then(|mut v| v.next()).unwrap_or(0);
    if window == 0 {
        return Ok(0);
    }
    let atom = c
        .intern_atom(true, b"_NET_WM_PID")
        .map_err(error)?
        .reply()
        .map_err(error)?
        .atom;
    if atom == 0 {
        return Ok(0);
    }
    let p = c
        .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 1)
        .map_err(error)?
        .reply()
        .map_err(error)?;
    Ok(p.value32().and_then(|mut v| v.next()).unwrap_or(0))
}
fn lua(s: &str) -> String {
    serde_json::to_string(s).expect("string serialization")
}
#[derive(Clone)]
struct Binding {
    slot: String,
    token: String,
}
#[derive(Default)]
pub struct Shortcuts {
    bindings: BTreeMap<u32, Binding>,
    stream: Option<UnixStream>,
    pending: Vec<u8>,
}
static SERIAL: AtomicU64 = AtomicU64::new(1);
fn conflict(json: &str, key: &str, mods: u32) -> Result<bool> {
    let values: Value = serde_json::from_str(json).map_err(error)?;
    let list = values
        .as_array()
        .filter(|a| a.len() <= 8192)
        .ok_or((FAILED, "invalid bindings snapshot".into()))?;
    Ok(list.iter().any(|b| {
        // A code-based binding can alias any keysym depending on the keymap.
        // Conservatively preserve it when modifiers match.
        let code = b["keycode"].as_u64().unwrap_or(0);
        (b["modmask"].as_u64() == Some(u64::from(mods)) || b["ignore_mods"].as_bool() == Some(true))
            && (b["key"]
                .as_str()
                .is_some_and(|k| k.eq_ignore_ascii_case(key))
                || code > 0)
    }))
}
impl Shortcuts {
    pub fn is_active(&self) -> bool {
        self.stream.is_some() && !self.bindings.is_empty()
    }
    pub fn register(&mut self, key: &str, mods: u32, id: u32) -> Result<()> {
        if id == 0
            || key.is_empty()
            || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || mods & !(1 | 4 | 8 | 64) != 0
        {
            return Err((INVALID, "invalid shortcut id/key/modifiers".into()));
        }
        if self.bindings.contains_key(&id) {
            return Err((CONFLICT, "shortcut id already registered".into()));
        }
        if self.bindings.len() >= 16 {
            return Err((INVALID, "too many shortcuts".into()));
        }
        let path = hyprland_path().ok_or((
            UNSUPPORTED,
            "global shortcuts currently require Hyprland".into(),
        ))?;
        if conflict(&request("j/binds")?, key, mods)? {
            return Err((CONFLICT, "existing key binding preserved".into()));
        }
        if self.stream.is_none() {
            let s = connect(&path.join(".socket2.sock"))?;
            s.set_nonblocking(true).map_err(error)?;
            self.stream = Some(s);
        }
        let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let token = format!("liboverlay_{}_{}_{}", std::process::id(), nonce, serial);
        let slot = format!("_G.{token}");
        let mut keys = String::new();
        for (mask, name) in [(1, "SHIFT"), (4, "CTRL"), (8, "ALT"), (64, "SUPER")] {
            if mods & mask != 0 {
                keys.push_str(name);
                keys.push_str(" + ");
            }
        }
        keys.push_str(key);
        let code=format!("/eval {slot} = hl.bind({}, hl.dsp.event({}), {{ non_consuming = true, description = {} }})",lua(&keys),lua(&token),lua(&token));
        let response = request(&code);
        let b = Binding { slot, token };
        let registration_error = match response {
            Ok(response) if response.starts_with("ok") => None,
            Ok(response) => Some((
                UNSUPPORTED,
                format!("Hyprland Lua binding unavailable: {response}"),
            )),
            Err(error) => Some(error),
        };
        if let Some(error) = registration_error {
            let _ = request(&format!(
                "/eval if {} then {}:unbind(); {} = nil end",
                b.slot, b.slot, b.slot
            ));
            return Err(error);
        }
        let confirmed = request("j/binds")
            .ok()
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| {
                v.as_array().map(|a| {
                    a.iter().any(|v| {
                        v["description"].as_str() == Some(&b.token)
                            && v["non_consuming"].as_bool() == Some(true)
                    })
                })
            })
            .unwrap_or(false);
        if !confirmed {
            let _ = request(&format!(
                "/eval if {} then {}:unbind(); {} = nil end",
                b.slot, b.slot, b.slot
            ));
            return Err((FAILED, "shortcut registration not confirmed".into()));
        }
        self.bindings.insert(id, b);
        Ok(())
    }
    pub fn remove(&mut self, id: u32) -> Result<()> {
        if let Some(b) = self.bindings.get(&id) {
            let response = request(&format!(
                "/eval if {} then {}:unbind(); {} = nil end",
                b.slot, b.slot, b.slot
            ))?;
            if !response.starts_with("ok") {
                return Err((FAILED, response));
            }
            self.bindings.remove(&id);
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Vec<Event> {
        let mut lost = false;
        if let Some(s) = self.stream.as_mut() {
            let mut buffer = [0u8; 8192];
            let mut read = 0;
            loop {
                match s.read(&mut buffer) {
                    Ok(0) => {
                        lost = true;
                        break;
                    }
                    Ok(n) => {
                        self.pending.extend_from_slice(&buffer[..n]);
                        read += n;
                        if self.pending.len() > 1024 * 1024 {
                            lost = true;
                            break;
                        }
                        if read >= 65536 {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        lost = true;
                        break;
                    }
                }
            }
        }
        let mut events = Vec::new();
        let mut consumed = 0;
        for line in self.pending.split_inclusive(|b| *b == b'\n') {
            if line.last() != Some(&b'\n') || events.len() >= 112 {
                break;
            }
            consumed += line.len();
            if let Some(token) = line[..line.len() - 1].strip_prefix(b"custom>>") {
                for (&id, b) in &self.bindings {
                    if token == b.token.as_bytes() {
                        events.push(Event {
                            kind: 3,
                            id,
                            ..Default::default()
                        });
                    }
                }
            }
        }
        self.pending.drain(..consumed);
        if lost {
            self.stream = None;
            self.pending.clear();
            for &id in self.bindings.keys() {
                events.push(Event {
                    kind: 4,
                    id,
                    ..Default::default()
                });
            }
            // Retain owned objects so destruction still cleans up exact bindings.
        }
        events
    }
}
impl Drop for Shortcuts {
    fn drop(&mut self) {
        for (_, b) in std::mem::take(&mut self.bindings) {
            let _ = request(&format!(
                "/eval if {} then {}:unbind(); {} = nil end",
                b.slot, b.slot, b.slot
            ));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const MONITORS:&[u8]=br#"[{"id":1,"name":"DP-3","x":0,"y":0,"width":2560,"height":1440,"scale":1,"activeWorkspace":{"id":1}},{"id":0,"name":"DP-1","x":2560,"y":0,"width":2560,"height":1440,"scale":1,"activeWorkspace":{"id":7}}]"#;
    #[test]
    fn native_window_on_second_monitor_is_relative_to_that_monitor() {
        let clients=br#"[{"pid":42,"mapped":true,"hidden":false,"visible":true,"xwayland":false,"monitor":0,"at":[2600,100],"size":[1200,700],"workspace":{"id":7}}]"#;
        let geometry = decode(42, clients, MONITORS).unwrap().unwrap();
        assert_eq!(geometry.visible, 1);
        assert_eq!(geometry.output(), "DP-1");
        assert!((geometry.x - 40.0 / 2560.0).abs() < 1e-6);
        assert!((geometry.y - 100.0 / 1440.0).abs() < 1e-6);
        assert!((geometry.width - 1200.0 / 2560.0).abs() < 1e-6);
    }
    #[test]
    fn hidden_workspace_is_not_a_visible_window() {
        let clients=br#"[{"pid":42,"mapped":true,"visible":true,"monitor":0,"at":[2600,100],"size":[1200,700],"workspace":{"id":6}}]"#;
        let geometry = decode(42, clients, MONITORS).unwrap().unwrap();
        assert_eq!(geometry.visible, 0);
        assert_eq!(geometry.output(), "DP-1");
        assert!(decode(99, clients, MONITORS).unwrap().is_none());
    }
    #[test]
    fn moved_rotated_and_scaled_window_uses_logical_coordinates() {
        let monitors=br#"[{"id":1,"name":"LEFT","x":-720,"y":0,"width":2560,"height":1440,"scale":2,"transform":1,"activeWorkspace":{"id":2}}]"#;
        let clients=br#"[{"pid":42,"mapped":true,"monitor":1,"at":[-700,40],"size":[360,640],"workspace":{"id":2}}]"#;
        let geometry = decode(42, clients, monitors).unwrap().unwrap();
        assert_eq!(geometry.visible, 1);
        assert!((geometry.x - 20.0 / 720.0).abs() < 1e-6);
        assert_eq!(geometry.width, 0.5);
        assert_eq!(geometry.height, 0.5);
        assert!(decode(42, b"not json", monitors).is_err());
    }
}

#[cfg(test)]
mod binding_tests {
    use super::*;
    #[test]
    fn existing_bindings_are_preserved() {
        assert!(!conflict("[]", "Insert", 0).unwrap());
        assert!(conflict(r#"[{"key":"Insert","modmask":0}]"#, "Insert", 0).unwrap());
        assert!(!conflict(r#"[{"key":"Insert","modmask":64}]"#, "Insert", 0).unwrap());
        assert!(conflict(r#"[{"keycode":118,"modmask":0}]"#, "Insert", 0).unwrap());
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    #[test]
    fn shortcuts_preserve_repeated_presses_and_partial_messages() {
        let mut shortcuts = Shortcuts::default();
        shortcuts.bindings.insert(
            7,
            Binding {
                slot: String::new(),
                token: "owned".into(),
            },
        );
        shortcuts
            .pending
            .extend_from_slice(b"activewindow>>other\ncustom>>owned\ncustom>>owned\ncustom>>ow");
        let events = shortcuts.poll();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.kind == 3 && e.id == 7));
        shortcuts.pending.extend_from_slice(b"ned\n");
        assert_eq!(shortcuts.poll().len(), 1);
        let (reader, writer) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        shortcuts.stream = Some(reader);
        drop(writer);
        let lost = shortcuts.poll();
        shortcuts.bindings.clear(); // Synthetic bindings must not touch a compositor.
        assert_eq!(lost[0].kind, 4);
        assert!(shortcuts.poll().is_empty());
    }
}
