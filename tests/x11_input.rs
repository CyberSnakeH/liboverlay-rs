//! Input injection is restricted to the runner's private Xvfb display.
#![cfg(feature = "x11")]
use liboverlay::{
    Anchor, BackendPreference, ButtonState, InputRectangle, InputRegion, OverlayBuilder,
    OverlayEvent, OverlayWindow, Requirement, TextInputState,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::{
        xproto::{self, ConnectionExt as _},
        xtest::ConnectionExt as _,
    },
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

fn private_display() -> Result {
    if std::env::var_os("LIBOVERLAY_TEST_PRIVATE_X11").is_none() {
        return Err(
            "use scripts/check_x11.py; this test injects input only in its private display".into(),
        );
    }
    Ok(())
}
fn window(conn: &impl Connection, root: u32, title: &str) -> Result<u32> {
    let clients = conn.intern_atom(false, b"_NET_CLIENT_LIST")?.reply()?.atom;
    let managed = conn
        .get_property(false, root, clients, xproto::AtomEnum::WINDOW, 0, 4096)?
        .reply()?;
    let mut windows: Vec<_> = managed.value32().into_iter().flatten().collect();
    windows.extend(conn.query_tree(root)?.reply()?.children);
    for child in windows {
        if conn
            .get_property(
                false,
                child,
                xproto::AtomEnum::WM_NAME,
                xproto::AtomEnum::STRING,
                0,
                1024,
            )?
            .reply()?
            .value
            == title.as_bytes()
        {
            return Ok(child);
        }
    }
    Err("owned test window not found".into())
}
fn wait(
    overlay: &OverlayWindow,
    mut predicate: impl FnMut(&OverlayEvent) -> bool,
) -> Result<Vec<OverlayEvent>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut all = Vec::new();
    while Instant::now() < deadline {
        let events = overlay.poll_events()?;
        let found = events.iter().any(&mut predicate);
        all.extend(events);
        if found {
            return Ok(all);
        }
        thread::sleep(Duration::from_millis(2));
    }
    Err(format!("expected input event did not arrive; received {all:?}").into())
}
fn key(conn: &impl Connection, root: u32, code: u8) -> Result {
    for event in [xproto::KEY_PRESS_EVENT, xproto::KEY_RELEASE_EVENT] {
        conn.xtest_fake_input(event, code, x11rb::CURRENT_TIME, root, 0, 0, 0)?
            .check()?;
    }
    Ok(())
}
fn focus(
    conn: &impl Connection,
    overlay: &OverlayWindow,
    window: u32,
) -> Result<Vec<OverlayEvent>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while conn.get_window_attributes(window)?.reply()?.map_state != xproto::MapState::VIEWABLE {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    overlay.poll_events()?;
    conn.set_input_focus(xproto::InputFocus::PARENT, window, x11rb::CURRENT_TIME)?
        .check()?;
    wait(overlay, |e| {
        matches!(e, OverlayEvent::KeyboardFocus { focused: true, .. })
    })
}
fn layout(name: &str) -> Result {
    let result = std::process::Command::new(
        std::env::var_os("SETXKBMAP").unwrap_or_else(|| "setxkbmap".into()),
    )
    .args(["-layout", name])
    .output()?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).into_owned().into());
    }
    Ok(())
}

#[test]
#[ignore = "requires the runner's owned Xvfb; clicks only test surfaces"]
fn custom_regions_restore_and_follow_surface_extent() -> Result {
    private_display()?;
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let background = conn.generate_id()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        background,
        root,
        0,
        0,
        640,
        480,
        0,
        xproto::WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &xproto::CreateWindowAux::new()
            .background_pixel(0)
            .override_redirect(1)
            .event_mask(xproto::EventMask::BUTTON_PRESS),
    )?
    .check()?;
    conn.map_window(background)?.check()?;
    let title = format!("liboverlay-region-{}", std::process::id());
    let region = InputRegion::Rectangles(vec![
        InputRectangle::new(-5, -5, 15, 15),
        InputRectangle::new(30, 30, 10, 10),
        InputRectangle::new(70, 70, 10, 10),
    ]);
    let overlay = OverlayBuilder::new()
        .with_title(&title)
        .with_backend(BackendPreference::X11Only)
        .with_transparency(Requirement::Preferred)
        .with_anchor(Anchor::TopLeft)
        .with_size(64, 64)
        .with_input_region(region)
        .build()?;
    let owned = window(&conn, root, &title)?;
    let click = |x: i16, y: i16, expected_overlay: bool| -> Result {
        overlay.poll_events()?;
        while conn.poll_for_event()?.is_some() {}
        conn.warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, x, y)?
            .check()?;
        for event in [xproto::BUTTON_PRESS_EVENT, xproto::BUTTON_RELEASE_EVENT] {
            conn.xtest_fake_input(event, 1, x11rb::CURRENT_TIME, root, 0, 0, 0)?
                .check()?;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut hit_overlay = false;
        let mut hit_background = false;
        while Instant::now() < deadline {
            hit_overlay |= overlay.poll_events()?.iter().any(|e| {
                matches!(
                    e,
                    OverlayEvent::PointerButton {
                        state: ButtonState::Pressed,
                        ..
                    }
                )
            });
            while let Some(event) = conn.poll_for_event()? {
                hit_background |=
                    matches!(event,x11rb::protocol::Event::ButtonPress(e) if e.event==background);
            }
            if hit_overlay || hit_background {
                break;
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            (hit_overlay, hit_background),
            (expected_overlay, !expected_overlay),
            "hit test at {x},{y}"
        );
        Ok(())
    };
    click(5, 5, true)?;
    click(10, 5, false)?;
    click(35, 35, true)?;
    click(40, 35, false)?;
    overlay.set_click_through(true)?;
    click(5, 5, false)?;
    overlay.set_click_through(false)?;
    click(5, 5, true)?;
    click(75, 75, false)?;
    conn.configure_window(
        owned,
        &xproto::ConfigureWindowAux::new().width(100).height(100),
    )?
    .check()?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::Resized(info) if info.logical_size.width==100),
    )?;
    click(75, 75, true)?;
    assert!(overlay
        .set_input_region(InputRegion::Rectangles(vec![InputRectangle::new(
            0, 0, 0, 1
        )]))
        .is_err());
    assert!(overlay.is_running());
    overlay.set_input_region(InputRegion::Empty)?;
    click(5, 5, false)?;
    overlay.set_input_region(InputRegion::Full)?;
    click(90, 90, true)?;
    overlay.close()?;
    conn.destroy_window(background)?.check()?;
    Ok(())
}

#[test]
#[ignore = "requires private Openbox/Xvfb and changes only that server's keymap"]
fn xkb_layout_changes_and_local_composition() -> Result {
    private_display()?;
    layout("us")?;
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let title = format!("liboverlay-text-{}", std::process::id());
    let overlay = OverlayBuilder::new()
        .with_title(&title)
        .with_backend(BackendPreference::X11Only)
        .with_transparency(Requirement::Preferred)
        .with_click_through(false)
        .with_text_input(TextInputState::enabled())
        .build()?;
    focus(&conn, &overlay, window(&conn, root, &title)?)?;
    key(&conn, root, 24)?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="q"),
    )?;
    layout("fr")?;
    wait(&overlay, |e| {
        matches!(
            e,
            OverlayEvent::InputReset {
                scope: liboverlay::InputResetScope::Keyboard,
                reason: liboverlay::InputResetReason::KeymapChanged,
                ..
            }
        )
    })?;
    key(&conn, root, 24)?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="a"),
    )?;
    key(&conn, root, 34)?;
    key(&conn, root, 26)?;
    let events = wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="ê"),
    )?;
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="ê"))
            .count(),
        1
    );
    overlay.set_text_input(TextInputState::default())?;
    key(&conn, root, 24)?;
    let events = wait(
        &overlay,
        |e| matches!(e,OverlayEvent::KeyboardInput(input) if input.state==ButtonState::Pressed),
    )?;
    assert!(!events
        .iter()
        .any(|e| matches!(e, OverlayEvent::TextCommitted { .. })));
    overlay.close()?;
    layout("us")?;
    Ok(())
}

struct MockIme {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl MockIme {
    fn new() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let marker = stop.clone();
        let (ready, rx) = std::sync::mpsc::sync_channel(1);
        let worker = thread::spawn(move || {
            let (conn, screen) = x11rb::connect(None).unwrap();
            let mut server = xim::x11rb::X11rbServer::init(
                &conn,
                screen,
                "liboverlay-fixture",
                xim::ALL_LOCALES,
            )
            .unwrap();
            let mut connections = xim::XimConnections::new();
            let mut handler = ImeHandler;
            ready.send(()).unwrap();
            while !marker.load(Ordering::Acquire) {
                while let Some(event) = conn.poll_for_event().unwrap() {
                    server
                        .filter_event(&event, &mut connections, &mut handler)
                        .unwrap();
                }
                conn.flush().unwrap();
                thread::sleep(Duration::from_millis(1));
            }
        });
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        Self {
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for MockIme {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}
struct ImeHandler;
impl<S: xim::Server<XEvent = xproto::KeyPressEvent> + xim::ServerCore> xim::ServerHandler<S>
    for ImeHandler
{
    type InputContextData = ();
    type InputStyleArray = [xim::InputStyle; 1];
    fn new_ic_data(
        &mut self,
        _: &mut S,
        _: xim::InputStyle,
    ) -> std::result::Result<(), xim::ServerError> {
        Ok(())
    }
    fn input_styles(&self) -> Self::InputStyleArray {
        [xim::InputStyle::PREEDIT_CALLBACKS | xim::InputStyle::STATUS_NOTHING]
    }
    fn filter_events(&self) -> u32 {
        1
    }
    fn handle_connect(&mut self, _: &mut S) -> std::result::Result<(), xim::ServerError> {
        Ok(())
    }
    fn handle_create_ic(
        &mut self,
        server: &mut S,
        ic: &mut xim::UserInputContext<()>,
    ) -> std::result::Result<(), xim::ServerError> {
        server.set_event_mask(&ic.ic, 1, 1)
    }
    fn handle_destroy_ic(
        &mut self,
        _: &mut S,
        _: xim::UserInputContext<()>,
    ) -> std::result::Result<(), xim::ServerError> {
        Ok(())
    }
    fn handle_reset_ic(
        &mut self,
        _: &mut S,
        _: &mut xim::UserInputContext<()>,
    ) -> std::result::Result<String, xim::ServerError> {
        Ok(String::new())
    }
    fn handle_set_focus(
        &mut self,
        _: &mut S,
        _: &mut xim::UserInputContext<()>,
    ) -> std::result::Result<(), xim::ServerError> {
        Ok(())
    }
    fn handle_unset_focus(
        &mut self,
        _: &mut S,
        _: &mut xim::UserInputContext<()>,
    ) -> std::result::Result<(), xim::ServerError> {
        Ok(())
    }
    fn handle_set_ic_values(
        &mut self,
        _: &mut S,
        _: &mut xim::UserInputContext<()>,
    ) -> std::result::Result<(), xim::ServerError> {
        Ok(())
    }
    fn handle_forward_event(
        &mut self,
        server: &mut S,
        ic: &mut xim::UserInputContext<()>,
        event: &xproto::KeyPressEvent,
    ) -> std::result::Result<bool, xim::ServerError> {
        if event.response_type & 0x7f != xproto::KEY_PRESS_EVENT {
            return Ok(false);
        }
        match event.detail {
            24 => server.preedit_draw(&mut ic.ic, "に")?,
            25 => server.preedit_draw(&mut ic.ic, "にほ")?,
            27 => {
                // Commit a syllable while retaining the server's preedit buffer.
                server.send_req(
                    ic.ic.client_win(),
                    xim::Request::Commit {
                        input_method_id: ic.ic.input_method_id().get(),
                        input_context_id: ic.ic.input_context_id().get(),
                        data: xim::CommitData::Chars {
                            commited: xim_ctext::utf8_to_compound_text("先"),
                            syncronous: false,
                        },
                    },
                )?;
                server.preedit_draw(&mut ic.ic, "後")?;
            }
            36 => {
                server.preedit_draw(&mut ic.ic, "")?;
                server.send_req(
                    ic.ic.client_win(),
                    xim::Request::Commit {
                        input_method_id: ic.ic.input_method_id().get(),
                        input_context_id: ic.ic.input_context_id().get(),
                        data: xim::CommitData::Both {
                            keysym: 0,
                            commited: xim_ctext::utf8_to_compound_text("日本"),
                            syncronous: true,
                        },
                    },
                )?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

#[test]
#[ignore = "runs an owned XIM server and injects keys only into its private window"]
fn xim_preedit_commit_forwarding_and_server_loss() -> Result {
    private_display()?;
    layout("us")?;
    let server = MockIme::new();
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let title = format!("liboverlay-xim-{}", std::process::id());
    let overlay = OverlayBuilder::new()
        .with_title(&title)
        .with_backend(BackendPreference::X11Only)
        .with_transparency(Requirement::Preferred)
        .with_click_through(false)
        .with_text_input(TextInputState::enabled())
        .build()?;
    let initial = focus(&conn, &overlay, window(&conn, root, &title)?)?;
    if !initial
        .iter()
        .any(|e| matches!(e, OverlayEvent::InputMethod { active: true, .. }))
    {
        wait(&overlay, |e| {
            matches!(e, OverlayEvent::InputMethod { active: true, .. })
        })?;
    }
    key(&conn, root, 24)?;
    let events = wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextPreedit {text,cursor:Some(cursor),..} if text=="に" && cursor.start==3),
    )?;
    assert!(!events
        .iter()
        .any(|e| matches!(e, OverlayEvent::TextCommitted { .. })));
    key(&conn, root, 25)?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextPreedit {text,cursor:Some(cursor),..} if text=="にほ" && cursor.start==6),
    )?;
    key(&conn, root, 36)?;
    let events = wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="日本"),
    )?;
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, OverlayEvent::TextCommitted { .. }))
            .count(),
        1
    );
    key(&conn, root, 24)?;
    wait(
        &overlay,
        |e| matches!(e, OverlayEvent::TextPreedit {text,..} if text=="に"),
    )?;
    key(&conn, root, 27)?;
    let continuation = wait(
        &overlay,
        |e| matches!(e, OverlayEvent::TextPreedit {text,..} if text=="後"),
    )?;
    assert!(continuation
        .iter()
        .any(|e| matches!(e, OverlayEvent::TextCommitted {text,..} if text=="先")));
    assert!(!continuation
        .iter()
        .any(|e| matches!(e, OverlayEvent::InputMethodError { .. })));
    // Reset a live preedit, then immediately type through the ResetIC fence.
    key(&conn, root, 24)?;
    wait(
        &overlay,
        |e| matches!(e, OverlayEvent::TextPreedit {text,..} if text == "に"),
    )?;
    overlay.set_text_input(TextInputState::enabled())?;
    key(&conn, root, 26)?;
    let released = |event: &OverlayEvent| {
        matches!(event,
        OverlayEvent::KeyboardInput(input)
            if input.code == 18 && input.state == ButtonState::Released)
    };
    let mut events = wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="e"),
    )?;
    if !events.iter().any(released) {
        events.extend(wait(&overlay, released)?);
    }
    let press = events
        .iter()
        .position(|e| {
            matches!(e,
        OverlayEvent::KeyboardInput(input)
            if input.code == 18 && input.state == ButtonState::Pressed)
        })
        .unwrap();
    let release = events.iter().position(released).unwrap();
    assert!(
        press < release,
        "XIM passthrough reordered the key transitions"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e,
        OverlayEvent::TextCommitted {text,..} if text=="e"))
            .count(),
        1
    );
    drop(server);
    wait(&overlay, |e| {
        matches!(e, OverlayEvent::InputMethodError { .. })
    })?;
    assert!(overlay.is_running());
    key(&conn, root, 26)?;
    wait(
        &overlay,
        |e| matches!(e,OverlayEvent::TextCommitted {text,..} if text=="e"),
    )?;
    overlay.close()?;
    Ok(())
}

#[test]
#[ignore = "requires an actual desktop IME on the runner's private Xvfb"]
fn desktop_input_method_composes_and_commits() -> Result {
    private_display()?;
    let expected = std::env::var("LIBOVERLAY_TEST_IME_TEXT")?;
    let codes = std::env::var("LIBOVERLAY_TEST_IME_KEYS")?
        .split(',')
        .map(str::parse::<u8>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    layout("us")?;
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let title = format!("liboverlay-desktop-ime-{}", std::process::id());
    let overlay = OverlayBuilder::new()
        .with_title(&title)
        .with_size(320, 120)
        .with_backend(BackendPreference::X11Only)
        .with_transparency(Requirement::Preferred)
        .with_click_through(false)
        .with_text_input(TextInputState::enabled())
        .build()?;
    let initial = focus(&conn, &overlay, window(&conn, root, &title)?)?;
    if !initial
        .iter()
        .any(|e| matches!(e, OverlayEvent::InputMethod { active: true, .. }))
    {
        wait(&overlay, |e| {
            matches!(e, OverlayEvent::InputMethod { active: true, .. })
        })?;
    }
    println!("READY desktop-ime");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    assert_eq!(line.trim(), "configured");
    let mut events = Vec::new();
    for code in codes {
        key(&conn, root, code)?;
        thread::sleep(Duration::from_millis(30));
        events.extend(overlay.poll_events()?);
    }
    key(&conn, root, 36)?;
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
                format!("desktop IME did not commit {expected:?}; received {events:?}").into(),
            );
        }
        thread::sleep(Duration::from_millis(2));
    }
    thread::sleep(Duration::from_millis(50));
    events.extend(overlay.poll_events()?);
    let committed: String = events
        .iter()
        .filter_map(|e| match e {
            OverlayEvent::TextCommitted { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        committed, expected,
        "duplicate or unfiltered text: {events:?}"
    );
    if std::env::var("LIBOVERLAY_TEST_SERVER_PREEDIT").as_deref() != Ok("1") {
        assert!(
            events
                .iter()
                .any(|e| matches!(e, OverlayEvent::TextPreedit { text, .. } if !text.is_empty())),
            "desktop IME did not expose preedit: {events:?}"
        );
    }
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverlayEvent::InputMethodError { .. })),
        "desktop IME failed: {events:?}"
    );
    overlay.close()?;
    println!("Desktop IME preedit and ordered commits verified: {expected}");
    Ok(())
}
