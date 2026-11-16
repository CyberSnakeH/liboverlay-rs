//! Virtual devices and a controlled IME, restricted to a runner-owned Sway.
#![cfg(feature = "wayland")]
use liboverlay::{
    BackendPreference, ButtonState, InputRectangle, InputRegion, OutputSelector, OverlayBuilder,
    OverlayEvent, OverlayWindow, SurroundingText, TextChangeCause, TextInputState,
};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Write,
    os::fd::AsFd,
    time::{Duration, Instant},
};
use wayland_client::{
    delegate_noop,
    protocol::{wl_output, wl_registry, wl_seat},
    Connection, Dispatch, EventQueue, QueueHandle,
};
use wayland_protocols_misc::{
    zwp_input_method_v2::client::{
        zwp_input_method_keyboard_grab_v2 as grab, zwp_input_method_manager_v2 as im,
        zwp_input_method_v2 as method,
    },
    zwp_virtual_keyboard_v1::client::{
        zwp_virtual_keyboard_manager_v1 as km, zwp_virtual_keyboard_v1 as vk,
    },
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1 as pm, zwlr_virtual_pointer_v1 as vp,
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
#[path = "fixtures/wayland_background.rs"]
mod background;

trait EventSource {
    fn poll_events(&self) -> Result<Vec<OverlayEvent>>;
}
impl EventSource for OverlayWindow {
    fn poll_events(&self) -> Result<Vec<OverlayEvent>> {
        Ok(OverlayWindow::poll_events(self)?)
    }
}
impl EventSource for background::Background {
    fn poll_events(&self) -> Result<Vec<OverlayEvent>> {
        background::Background::poll_events(self)
    }
}
#[derive(Default)]
struct State {
    seat: Option<wl_seat::WlSeat>,
    keyboard_manager: Option<km::ZwpVirtualKeyboardManagerV1>,
    pointer_manager: Option<pm::ZwlrVirtualPointerManagerV1>,
    ime_manager: Option<im::ZwpInputMethodManagerV2>,
    outputs: BTreeMap<u32, (wl_output::WlOutput, String)>,
    ime_active: bool,
    ime_pending: Option<bool>,
    ime_serial: u32,
    ime_unavailable: bool,
    surrounding: Option<(String, u32, u32)>,
    grabbed: Vec<u32>,
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(registry.bind(name, version.min(5), qh, ()))
                }
                "wl_output" => {
                    state.outputs.insert(
                        name,
                        (registry.bind(name, version.min(4), qh, name), String::new()),
                    );
                }
                "zwp_virtual_keyboard_manager_v1" => {
                    state.keyboard_manager = Some(registry.bind(name, 1, qh, ()))
                }
                "zwlr_virtual_pointer_manager_v1" => {
                    state.pointer_manager = Some(registry.bind(name, version.min(2), qh, ()))
                }
                "zwp_input_method_manager_v2" => {
                    state.ime_manager = Some(registry.bind(name, 1, qh, ()))
                }
                _ => {}
            }
        }
    }
}
impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            if let Some(output) = state.outputs.get_mut(id) {
                output.1 = name;
            }
        }
    }
}
impl Dispatch<method::ZwpInputMethodV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &method::ZwpInputMethodV2,
        event: method::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            method::Event::Activate => state.ime_pending = Some(true),
            method::Event::Deactivate => state.ime_pending = Some(false),
            method::Event::Done => {
                state.ime_serial = state.ime_serial.wrapping_add(1);
                if let Some(active) = state.ime_pending.take() {
                    state.ime_active = active;
                }
            }
            method::Event::SurroundingText {
                text,
                cursor,
                anchor,
            } => state.surrounding = Some((text, cursor, anchor)),
            method::Event::Unavailable => state.ime_unavailable = true,
            _ => {}
        }
    }
}
impl Dispatch<grab::ZwpInputMethodKeyboardGrabV2, ()> for State {
    fn event(
        state: &mut Self,
        _: &grab::ZwpInputMethodKeyboardGrabV2,
        event: grab::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let grab::Event::Key {
            key,
            state:
                wayland_client::WEnum::Value(wayland_client::protocol::wl_keyboard::KeyState::Pressed),
            ..
        } = event
        {
            state.grabbed.push(key);
        }
    }
}
delegate_noop!(State: ignore wl_seat::WlSeat);
delegate_noop!(State: ignore km::ZwpVirtualKeyboardManagerV1);
delegate_noop!(State: ignore vk::ZwpVirtualKeyboardV1);
delegate_noop!(State: ignore pm::ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ignore vp::ZwlrVirtualPointerV1);
delegate_noop!(State: ignore im::ZwpInputMethodManagerV2);

struct Devices {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    keyboard: vk::ZwpVirtualKeyboardV1,
    pointer: vp::ZwlrVirtualPointerV1,
}

struct InputMethodPeer {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    method: method::ZwpInputMethodV2,
    grab: grab::ZwpInputMethodKeyboardGrabV2,
}
impl InputMethodPeer {
    fn new() -> Result<Self> {
        if std::env::var_os("LIBOVERLAY_TEST_PRIVATE_INPUT").is_none() {
            return Err("private compositor required".into());
        }
        let conn = Connection::connect_to_env()?;
        let mut queue = conn.new_event_queue();
        let mut state = State::default();
        conn.display().get_registry(&queue.handle(), ());
        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;
        let manager = state
            .ime_manager
            .as_ref()
            .ok_or("private input method protocol unavailable")?;
        let method = manager.get_input_method(
            state.seat.as_ref().ok_or("private seat unavailable")?,
            &queue.handle(),
            (),
        );
        let grab = method.grab_keyboard(&queue.handle(), ());
        conn.flush()?;
        Ok(Self {
            conn,
            queue,
            state,
            method,
            grab,
        })
    }
    fn pump(&mut self) -> Result {
        self.queue.roundtrip(&mut self.state)?;
        if self.state.ime_unavailable {
            return Err("private input method refused".into());
        }
        Ok(())
    }
    fn until(&mut self, predicate: impl Fn(&State) -> bool) -> Result {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate(&self.state) {
            self.pump()?;
            if Instant::now() > deadline {
                return Err("private IME state timed out".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
}
impl Drop for InputMethodPeer {
    fn drop(&mut self) {
        self.grab.release();
        self.method.destroy();
        let _ = self.conn.flush();
    }
}
impl Devices {
    fn new() -> Result<Self> {
        if std::env::var_os("LIBOVERLAY_TEST_PRIVATE_INPUT").is_none() {
            return Err("use scripts/check_wayland.py; virtual input is allowed only on its private compositor".into());
        }
        let conn = Connection::connect_to_env()?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let mut state = State::default();
        conn.display().get_registry(&qh, ());
        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;
        let seat = state.seat.as_ref().ok_or("no private seat")?;
        let keyboard = state
            .keyboard_manager
            .as_ref()
            .ok_or("virtual keyboard unavailable")?
            .create_virtual_keyboard(seat, &qh, ());
        let output = &state
            .outputs
            .values()
            .find(|(_, name)| name == "HEADLESS-1")
            .ok_or("private output unavailable")?
            .0;
        let pointer = state
            .pointer_manager
            .as_ref()
            .ok_or("virtual pointer unavailable")?
            .create_virtual_pointer_with_output(Some(seat), Some(output), &qh, ());
        let context = kbvm::xkb::Context::default();
        let groups =
            kbvm::xkb::rmlvo::Group::from_layouts_and_variants("us,fr", "").collect::<Vec<_>>();
        let keymap = context.keymap_from_names(
            (kbvm::xkb::diagnostic::WriteToStderr, |_, fatal| fatal),
            None,
            None,
            Some(&groups),
            None,
        );
        let mut text = keymap.format().to_string().into_bytes();
        text.push(0);
        let mut file = File::from(rustix::fs::memfd_create(
            c"liboverlay-keymap-fixture",
            rustix::fs::MemfdFlags::CLOEXEC,
        )?);
        file.write_all(&text)?;
        keyboard.keymap(1, file.as_fd(), text.len() as u32);
        keyboard.modifiers(0, 0, 0, 0);
        conn.flush()?;
        queue.roundtrip(&mut state)?;
        Ok(Self {
            conn,
            queue,
            state,
            keyboard,
            pointer,
        })
    }
    fn click(&mut self, x: u32, y: u32) -> Result {
        self.pointer.motion_absolute(1, x, y, 640, 480);
        self.pointer.frame();
        self.pointer.button(
            1,
            272,
            wayland_client::protocol::wl_pointer::ButtonState::Pressed,
        );
        self.pointer.frame();
        self.pointer.button(
            2,
            272,
            wayland_client::protocol::wl_pointer::ButtonState::Released,
        );
        self.pointer.frame();
        self.conn.flush()?;
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }
    fn key(&mut self, code: u32, pressed: bool) -> Result {
        self.keyboard.key(3, code, u32::from(pressed));
        self.conn.flush()?;
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }
    fn tap(&mut self, code: u32) -> Result {
        self.key(code, true)?;
        self.key(code, false)
    }
    fn modifiers(&mut self, shift: u32, group: u32) -> Result {
        self.keyboard.modifiers(shift, 0, 0, group);
        self.conn.flush()?;
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }
}
impl Drop for Devices {
    fn drop(&mut self) {
        self.keyboard.destroy();
        self.pointer.destroy();
        let _ = self.conn.flush();
    }
}
#[track_caller]
fn wait(
    overlay: &impl EventSource,
    mut predicate: impl FnMut(&OverlayEvent) -> bool,
) -> Result<Vec<OverlayEvent>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut all = Vec::new();
    while Instant::now() < deadline {
        let events = overlay.poll_events()?;
        let done = events.iter().any(&mut predicate);
        all.extend(events);
        if done {
            return Ok(all);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Err(format!(
        "private Wayland input at {} timed out: {all:?}",
        std::panic::Location::caller()
    )
    .into())
}
fn signal(stage: &str) -> Result {
    println!("READY {stage}");
    std::io::stdout().flush()?;
    let mut reply = String::new();
    std::io::stdin().read_line(&mut reply)?;
    if reply.trim().is_empty() {
        return Err("input runner disconnected".into());
    }
    Ok(())
}
fn builder() -> OverlayBuilder {
    OverlayBuilder::new()
        .with_backend(BackendPreference::WaylandOnly)
        .with_output(OutputSelector::Name("HEADLESS-1".into()))
}

#[test]
#[ignore = "injects virtual input only into the runner's private Sway"]
fn keyboard_regions_and_input_method() -> Result {
    let mut devices = Devices::new()?;
    let background = background::Background::new()?;
    let region = InputRegion::Rectangles(vec![InputRectangle::new(70, 30, 20, 20)]);
    let text = TextInputState {
        surrounding: Some(SurroundingText {
            text: "abcd".into(),
            cursor: 2,
            anchor: 2,
        }),
        ..TextInputState::enabled()
    };
    let overlay = builder()
        .with_size(160, 80)
        .with_input_region(region.clone())
        .with_text_input(text.clone())
        .build()?;
    overlay.draw(|p, _, _| p.fill(0))?;
    wait(&overlay, |e| {
        matches!(e, OverlayEvent::FrameSubmitted { .. })
    })?;
    devices.click(320, 240)?;
    wait(&overlay, |e| {
        matches!(e, OverlayEvent::KeyboardFocus { focused: true, .. })
    })?;
    background.poll_events()?;
    devices.click(330, 240)?;
    wait(&background, |e| {
        matches!(
            e,
            OverlayEvent::PointerButton {
                state: ButtonState::Pressed,
                ..
            }
        )
    })?;
    overlay.set_click_through(true)?;
    devices.click(320, 240)?;
    wait(&background, |e| {
        matches!(
            e,
            OverlayEvent::PointerButton {
                state: ButtonState::Pressed,
                ..
            }
        )
    })?;
    overlay.set_click_through(false)?;
    devices.click(320, 240)?;
    wait(&overlay, |e| {
        matches!(
            e,
            OverlayEvent::PointerButton {
                state: ButtonState::Pressed,
                ..
            }
        )
    })?;
    signal("input-scale")?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::Resized(info) if info.scale_factor.as_f64()==2.),
    )?;
    overlay.draw(|p, _, _| p.fill(0))?;
    wait(&overlay, |e| {
        matches!(e, OverlayEvent::FrameSubmitted { .. })
    })?;
    devices.click(340, 240)?;
    wait(&background, |e| {
        matches!(
            e,
            OverlayEvent::PointerButton {
                state: ButtonState::Pressed,
                ..
            }
        )
    })?;
    devices.click(320, 240)?;
    wait(&overlay, |e| {
        matches!(
            e,
            OverlayEvent::PointerButton {
                state: ButtonState::Pressed,
                ..
            }
        )
    })?;
    signal("input-scale-reset")?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::Resized(info) if info.scale_factor.as_f64()==1.),
    )?;
    overlay.draw(|p, _, _| p.fill(0))?;
    wait(&overlay, |e| {
        matches!(e, OverlayEvent::FrameSubmitted { .. })
    })?;
    overlay.poll_events()?;
    devices.tap(16)?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="q"),
    )?;
    devices.modifiers(0, 1)?;
    devices.tap(16)?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="a"),
    )?;
    devices.modifiers(1, 1)?;
    devices.tap(16)?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="A"),
    )?;
    devices.modifiers(0, 1)?;
    devices.tap(26)?;
    devices.tap(18)?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="ê"),
    )?;
    devices.modifiers(0, 0)?;
    devices.key(16, true)?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::KeyboardInput(input) if input.repeat),
    )?;
    devices.key(16, false)?;
    wait(&overlay, |e| {
        matches!(
            e,
            OverlayEvent::Key {
                code: 16,
                state: ButtonState::Released
            }
        )
    })?;
    std::thread::sleep(Duration::from_millis(80));
    assert!(!overlay
        .poll_events()?
        .iter()
        .any(|e| matches!(e,OverlayEvent::KeyboardInput(input) if input.repeat)));
    // A separate client is required: wlroots deliberately does not send an
    // IME its own virtual-keyboard events, avoiding a forwarding feedback loop.
    let mut input_method = InputMethodPeer::new()?;
    input_method.until(|state| state.ime_active)?;
    assert_eq!(
        input_method.state.surrounding.as_ref(),
        Some(&("abcd".to_owned(), 2, 2))
    );
    overlay.poll_events()?;
    devices.tap(16)?;
    input_method.until(|state| state.grabbed.contains(&16))?;
    assert!(!overlay
        .poll_events()?
        .iter()
        .any(|e| matches!(e, OverlayEvent::TextCommitted { .. })));
    input_method.method.set_preedit_string("に".into(), 3, 3);
    input_method.method.commit(input_method.state.ime_serial);
    input_method.conn.flush()?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextPreedit {text,cursor:Some(cursor),..} if text=="に" && cursor.start==3),
    )?;
    input_method.method.delete_surrounding_text(1, 1);
    input_method.method.commit_string("日本".into());
    input_method.method.set_preedit_string(String::new(), 0, 0);
    input_method.method.commit(input_method.state.ime_serial);
    input_method.conn.flush()?;
    let edits = wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="日本"),
    )?;
    let deletion = edits
        .iter()
        .position(|e| {
            matches!(
                e,
                OverlayEvent::TextDeleteSurrounding {
                    before: 1,
                    after: 1,
                    ..
                }
            )
        })
        .ok_or("IME deletion missing")?;
    let commit = edits
        .iter()
        .position(|e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="日本"))
        .unwrap();
    assert!(deletion < commit);
    assert_eq!(
        edits
            .iter()
            .filter(|e| matches!(e, OverlayEvent::TextCommitted { .. }))
            .count(),
        1
    );
    let updated = TextInputState {
        surrounding: Some(SurroundingText {
            text: "a日本d".into(),
            cursor: 7,
            anchor: 7,
        }),
        change_cause: TextChangeCause::InputMethod,
        ..text
    };
    overlay.set_text_input(updated)?;
    input_method.until(|state| state.surrounding.as_ref() == Some(&("a日本d".to_owned(), 7, 7)))?;
    overlay.set_text_input(TextInputState::default())?;
    input_method.until(|state| !state.ime_active)?;
    drop(input_method);
    overlay.close()?;
    background.close()?;
    Ok(())
}

#[test]
#[ignore = "requires a real desktop IME on the runner's private Wayland display"]
fn desktop_input_method_composes_and_commits() -> Result {
    let expected = std::env::var("LIBOVERLAY_TEST_IME_TEXT")?;
    let codes = std::env::var("LIBOVERLAY_TEST_IME_KEYS")?
        .split(',')
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut devices = Devices::new()?;
    let overlay = builder()
        .with_size(160, 80)
        .with_click_through(false)
        .with_text_input(TextInputState::enabled())
        .build()?;
    overlay.draw(|p, _, _| p.fill(0))?;
    let initial = wait(&overlay, |e| {
        matches!(e, OverlayEvent::FrameSubmitted { .. })
    })?;
    devices.click(320, 240)?;
    if !initial
        .iter()
        .any(|e| matches!(e, OverlayEvent::InputMethod { active: true, .. }))
    {
        wait(&overlay, |e| {
            matches!(e, OverlayEvent::InputMethod { active: true, .. })
        })?;
    }
    signal("desktop-ime")?;
    let mut events = Vec::new();
    for code in codes {
        devices.tap(code - 8)?;
        std::thread::sleep(Duration::from_millis(30));
        events.extend(overlay.poll_events()?);
    }
    devices.tap(28)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        events.extend(overlay.poll_events()?);
        let committed: String = events
            .iter()
            .filter_map(|e| match e {
                OverlayEvent::TextCommitted { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if committed == expected {
            break;
        }
        if Instant::now() >= deadline {
            return Err(
                format!("desktop Wayland IME did not commit {expected:?}: {events:?}").into(),
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(events
        .iter()
        .any(|e| matches!(e, OverlayEvent::TextPreedit {text,..} if !text.is_empty())));
    std::thread::sleep(Duration::from_millis(50));
    events.extend(overlay.poll_events()?);
    let committed: String = events
        .iter()
        .filter_map(|e| match e {
            OverlayEvent::TextCommitted { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(committed, expected);
    overlay.close()?;
    println!("Desktop Wayland IME preedit and ordered commits verified: {expected}");
    Ok(())
}

#[test]
#[ignore = "measures only virtual input on the runner's private Sway"]
fn virtual_input_to_presentation() -> Result {
    fn clock() -> Duration {
        let t = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
        Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
    }
    let mut devices = Devices::new()?;
    let overlay = builder()
        .with_size(160, 80)
        .with_click_through(false)
        .with_keyboard_mode(liboverlay::KeyboardMode::OnDemand)
        .with_presentation_feedback(true)
        .build()?;
    overlay.draw(|p, _, _| p.fill(0xff000000))?;
    wait(&overlay, |e| matches!(e, OverlayEvent::FramePresented(_)))?;
    devices.click(320, 240)?;
    overlay.poll_events()?;
    let mut rows=String::from("sample\tinject_to_key_us\tkey_to_present_us\tinject_to_present_us\tvsync\thw_clock\thw_completion\n");
    let mut sum = 0u128;
    for index in 0..30 {
        let injected = clock();
        devices.tap(30)?;
        wait(&overlay, |e| {
            matches!(
                e,
                OverlayEvent::Key {
                    code: 30,
                    state: ButtonState::Pressed
                }
            )
        })?;
        let key = clock();
        overlay.draw(|p, _, _| {
            p.fill(0xff000000);
            p[0] = 0xff000001 + (index % 2);
        })?;
        let events = wait(&overlay, |e| matches!(e, OverlayEvent::FramePresented(_)))?;
        let feedback = events
            .iter()
            .find_map(|e| {
                if let OverlayEvent::FramePresented(p) = e {
                    Some(p)
                } else {
                    None
                }
            })
            .unwrap();
        assert_eq!(
            feedback.clock_id,
            Some(1),
            "private Sway must expose CLOCK_MONOTONIC"
        );
        let total = feedback
            .timestamp
            .checked_sub(injected)
            .ok_or("presentation predates injected event")?
            .as_micros();
        let response = feedback
            .timestamp
            .checked_sub(key)
            .ok_or("presentation predates redraw")?
            .as_micros();
        sum += total;
        use std::fmt::Write as _;
        writeln!(
            rows,
            "{index}\t{}\t{response}\t{total}\t{}\t{}\t{}",
            (key - injected).as_micros(),
            feedback.vsync,
            feedback.hardware_clock,
            feedback.hardware_completion
        )?;
    }
    overlay.close()?;
    if let Some(path) = std::env::var_os("LIBOVERLAY_TEST_LATENCY_OUTPUT") {
        std::fs::write(path, rows)?;
    }
    println!("Virtual input to server presentation: 30 samples, mean={} us; excludes physical switch and optical response",sum/30);
    Ok(())
}

#[test]
#[ignore = "cycles an owned GTK window through fullscreen on the runner's private Sway"]
fn fullscreen_transitions_keep_layer_input_and_focus() -> Result {
    let mut devices = Devices::new()?;
    let overlay = builder()
        .with_size(160, 80)
        .with_click_through(false)
        .with_keyboard_mode(liboverlay::KeyboardMode::OnDemand)
        .build()?;
    overlay.draw(|p, _, _| p.fill(0xff223344))?;
    wait(&overlay, |e| {
        matches!(e, OverlayEvent::FrameSubmitted { .. })
    })?;
    for index in 0..8 {
        overlay.poll_events()?;
        signal(&format!("fullscreen-{index}"))?;
        devices.click(320, 240)?;
        wait(&overlay, |e| {
            matches!(
                e,
                OverlayEvent::PointerButton {
                    state: ButtonState::Pressed,
                    ..
                }
            )
        })?;
        devices.tap(30)?;
        wait(&overlay, |e| {
            matches!(
                e,
                OverlayEvent::Key {
                    code: 30,
                    state: ButtonState::Pressed
                }
            )
        })?;
        overlay.set_click_through(true)?;
        overlay.set_click_through(false)?;
    }
    overlay.close()?;
    Ok(())
}
