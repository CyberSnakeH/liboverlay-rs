//! Opt-in server-side checks on this test's own X11 window only.
#![cfg(feature = "x11")]
use liboverlay::{premultiply, BackendPreference, OverlayBuilder, OverlayEvent};
use std::time::{Duration, Instant};
use x11rb::{
    connection::Connection,
    protocol::{
        shape::{self, ConnectionExt as _},
        xproto::{self, ConnectionExt as _},
    },
};

fn wait_for_compositor(
    conn: &impl Connection,
    screen: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if conn.query_extension(b"XWAYLAND")?.reply()?.present {
        return Ok(());
    }
    let name = format!("_NET_WM_CM_S{screen}");
    let atom = conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
    let deadline = Instant::now() + Duration::from_secs(3);
    while conn.get_selection_owner(atom)?.reply()?.owner == x11rb::NONE {
        if Instant::now() >= deadline {
            return Err("no X11 compositor became ready".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn owned_window(
    conn: &impl Connection,
    root: u32,
    title: &str,
) -> Result<u32, Box<dyn std::error::Error>> {
    let clients = conn.intern_atom(false, b"_NET_CLIENT_LIST")?.reply()?.atom;
    let property = conn
        .get_property(false, root, clients, xproto::AtomEnum::WINDOW, 0, 4096)?
        .reply()?;
    let managed: Vec<_> = property
        .value32()
        .map(Iterator::collect)
        .unwrap_or_default();
    for child in managed
        .into_iter()
        .chain(conn.query_tree(root)?.reply()?.children)
    {
        let name = conn
            .get_property(
                false,
                child,
                xproto::AtomEnum::WM_NAME,
                xproto::AtomEnum::STRING,
                0,
                1024,
            )?
            .reply()?;
        if name.value == title.as_bytes() {
            return Ok(child);
        }
    }
    Err("diagnostic window not found".into())
}

#[test]
#[ignore = "uses the runner's private Openbox/Xvfb and clicks only its owned window"]
fn keyboard_focus_and_fullscreen_policy() -> Result<(), Box<dyn std::error::Error>> {
    use liboverlay::{KeyboardMode, Requirement};
    use x11rb::protocol::xtest::ConnectionExt as _;
    if std::env::var_os("LIBOVERLAY_TEST_PRIVATE_WM").is_none() {
        return Err("run this test through scripts/check_x11.py --wm".into());
    }
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &[u8]| -> Result<u32, Box<dyn std::error::Error>> {
        Ok(conn.intern_atom(false, name)?.reply()?.atom)
    };
    let supporting = atom(b"_NET_SUPPORTING_WM_CHECK")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while conn
        .get_property(false, root, supporting, xproto::AtomEnum::WINDOW, 0, 1)?
        .reply()?
        .value32()
        .and_then(|mut v| v.next())
        .is_none()
    {
        assert!(Instant::now() < deadline, "window manager did not start");
        std::thread::sleep(Duration::from_millis(5));
    }
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
        &xproto::CreateWindowAux::new().background_pixel(0),
    )?
    .check()?;
    let mut hints = x11rb::properties::WmHints::new();
    hints.input = Some(true);
    hints.initial_state = Some(x11rb::properties::WmHintsState::Normal);
    hints.set(&conn, background)?.check()?;
    conn.map_window(background)?.check()?;
    let wait_focus = |expected: u32| -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(3);
        while conn.get_input_focus()?.reply()?.focus != expected {
            assert!(
                Instant::now() < deadline,
                "expected focus on owned window {expected}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while conn.get_window_attributes(background)?.reply()?.map_state != xproto::MapState::VIEWABLE {
        if Instant::now() >= deadline {
            let state = conn.get_window_attributes(background)?.reply()?.map_state;
            let tree = conn.query_tree(background)?.reply()?;
            panic!(
                "owned background was not mapped: state={state:?}, parent={}, root={}",
                tree.parent, tree.root
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    conn.set_input_focus(xproto::InputFocus::PARENT, background, x11rb::CURRENT_TIME)?
        .check()?;
    wait_focus(background)?;
    let state = atom(b"_NET_WM_STATE")?;
    let fullscreen = atom(b"_NET_WM_STATE_FULLSCREEN")?;
    let change_fullscreen = |action: u32| -> Result<(), Box<dyn std::error::Error>> {
        let event =
            xproto::ClientMessageEvent::new(32, background, state, [action, fullscreen, 0, 1, 0]);
        conn.send_event(
            false,
            root,
            xproto::EventMask::SUBSTRUCTURE_REDIRECT | xproto::EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )?
        .check()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let property = conn
                .get_property(false, background, state, xproto::AtomEnum::ATOM, 0, 128)?
                .reply()?;
            let active = property
                .value32()
                .is_some_and(|mut v| v.any(|a| a == fullscreen));
            if active == (action == 1) {
                break;
            }
            assert!(Instant::now() < deadline, "fullscreen state did not change");
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    };
    change_fullscreen(1)?;
    let passive_title = format!("liboverlay-passive-focus-{}", std::process::id());
    let passive = OverlayBuilder::new()
        .with_title(&passive_title)
        .with_backend(BackendPreference::X11Only)
        .with_transparency(Requirement::Preferred)
        .with_size(80, 40)
        .build()?;
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(
        conn.get_input_focus()?.reply()?.focus,
        background,
        "passive overlay stole fullscreen focus"
    );
    let passive_window = owned_window(&conn, root, &passive_title)?;
    let mut background_frame = background;
    loop {
        let parent = conn.query_tree(background_frame)?.reply()?.parent;
        if parent == root {
            break;
        }
        background_frame = parent;
    }
    let stacking = conn.query_tree(root)?.reply()?.children;
    assert!(
        stacking.iter().position(|&w| w == passive_window)
            > stacking.iter().position(|&w| w == background_frame),
        "passive overlay was below the fullscreen background"
    );
    passive.close()?;
    let title = format!("liboverlay-managed-focus-{}", std::process::id());
    let interactive = OverlayBuilder::new()
        .with_title(&title)
        .with_backend(BackendPreference::X11Only)
        .with_transparency(Requirement::Preferred)
        .with_keyboard_mode(KeyboardMode::OnDemand)
        .with_size(160, 80)
        .build()?;
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        conn.get_input_focus()?.reply()?.focus,
        background,
        "on-demand overlay stole initial focus"
    );
    let window = owned_window(&conn, root, &title)?;
    let position = conn.translate_coordinates(window, root, 0, 0)?.reply()?;
    assert_eq!((position.dst_x, position.dst_y), (240, 200));
    let extents = atom(b"_NET_FRAME_EXTENTS")?;
    let frame = conn
        .get_property(false, window, extents, xproto::AtomEnum::CARDINAL, 0, 4)?
        .reply()?;
    let frame: Vec<_> = frame.value32().map(Iterator::collect).unwrap_or_default();
    assert_eq!(
        frame,
        vec![0, 0, 0, 0],
        "overlay requested managed window decorations"
    );
    change_fullscreen(0)?;
    interactive.set_click_through(false)?;
    let point = conn.translate_coordinates(window, root, 20, 20)?.reply()?;
    conn.warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, point.dst_x, point.dst_y)?
        .check()?;
    conn.xtest_fake_input(
        xproto::BUTTON_PRESS_EVENT,
        1,
        x11rb::CURRENT_TIME,
        root,
        0,
        0,
        0,
    )?
    .check()?;
    conn.xtest_fake_input(
        xproto::BUTTON_RELEASE_EVENT,
        1,
        x11rb::CURRENT_TIME,
        root,
        0,
        0,
        0,
    )?
    .check()?;
    wait_focus(window)?;
    let user_time = atom(b"_NET_WM_USER_TIME")?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let timestamp = conn
            .get_property(false, window, user_time, xproto::AtomEnum::CARDINAL, 0, 1)?
            .reply()?
            .value32()
            .and_then(|mut v| v.next())
            .unwrap_or(0);
        if timestamp != 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "user interaction timestamp was not updated"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    for event in [xproto::KEY_PRESS_EVENT, xproto::KEY_RELEASE_EVENT] {
        conn.xtest_fake_input(event, 38, x11rb::CURRENT_TIME, root, 0, 0, 0)?
            .check()?;
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut pressed = false;
    let mut released = false;
    while !pressed || !released {
        for event in interactive.poll_events()? {
            if let OverlayEvent::Key { code: 30, state } = event {
                pressed |= state == liboverlay::ButtonState::Pressed;
                released |= state == liboverlay::ButtonState::Released;
            }
        }
        assert!(
            Instant::now() < deadline,
            "focused overlay did not receive raw keys"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    interactive.close()?;
    wait_focus(background)?;
    conn.destroy_window(background)?.check()?;
    Ok(())
}

#[test]
#[ignore = "opens a temporary X11 overlay; requires a composited local display"]
fn alpha_input_region_focus_and_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen) = x11rb::connect(None)?;
    wait_for_compositor(&conn, screen)?;
    let original_focus = conn.get_input_focus()?.reply()?.focus;
    let title = format!("liboverlay-protocol-test-{}", std::process::id());
    let overlay = OverlayBuilder::new()
        .with_title(&title)
        .with_size(64, 32)
        .with_backend(BackendPreference::X11Only)
        .build()?;
    let root = conn.setup().roots[screen].root;
    let window = owned_window(&conn, root, &title)?;
    assert!(conn
        .shape_get_rectangles(window, shape::SK::INPUT)?
        .reply()?
        .rectangles
        .is_empty());
    println!(
        "pacing={:?}",
        overlay.backend_report().capabilities.frame_pacing
    );
    // Several distinct frames verify that Present releases and completes each
    // upload, rather than allowing only the first request to make progress.
    for color in [
        premultiply(200, 80, 40, 128),
        premultiply(40, 160, 80, 160),
        premultiply(80, 40, 200, 96),
    ] {
        overlay.draw(|pixels, _, _| {
            pixels.fill(color);
            pixels[0] = 0;
        })?;
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut submitted = false;
        while Instant::now() < deadline {
            if overlay
                .poll_events()?
                .iter()
                .any(|event| matches!(event, OverlayEvent::FrameSubmitted { .. }))
            {
                submitted = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(submitted, "frame was never submitted");
        // Submission precedes the scheduled vertical blank. Read only this
        // test's window until the server has applied the requested pixels.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let image = conn
                .get_image(xproto::ImageFormat::Z_PIXMAP, window, 0, 0, 2, 1, u32::MAX)?
                .reply()?;
            assert_eq!(image.depth, 32);
            assert_eq!(image.data.len(), 8);
            let read_pixel = |bytes: &[u8]| -> u32 {
                let array: [u8; 4] = bytes.try_into().unwrap();
                if conn.setup().image_byte_order == xproto::ImageOrder::LSB_FIRST {
                    u32::from_le_bytes(array)
                } else {
                    u32::from_be_bytes(array)
                }
            };
            if read_pixel(&image.data[..4]) == 0 && read_pixel(&image.data[4..]) == color {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "transparent/premultiplied pixels were not presented"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    overlay.set_click_through(false)?;
    let input = conn
        .shape_get_rectangles(window, shape::SK::INPUT)?
        .reply()?
        .rectangles;
    let rectangles: Vec<_> = input
        .iter()
        .map(|r| (r.x, r.y, r.width, r.height))
        .collect();
    assert_eq!(rectangles, vec![(0, 0, 64, 32)]);
    overlay.set_click_through(true)?;
    assert!(conn
        .shape_get_rectangles(window, shape::SK::INPUT)?
        .reply()?
        .rectangles
        .is_empty());
    assert_eq!(
        conn.get_input_focus()?.reply()?.focus,
        original_focus,
        "passive overlay changed focus"
    );
    overlay.close()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut destroyed = false;
    while Instant::now() < deadline {
        match conn.get_window_attributes(window)?.reply() {
            Err(x11rb::errors::ReplyError::X11Error(error))
                if error.error_kind == x11rb::protocol::ErrorKind::Window =>
            {
                destroyed = true;
                break;
            }
            Err(error) => return Err(error.into()),
            Ok(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    assert!(destroyed, "overlay window leaked after close");
    println!("ARGB readback, input regions, unchanged focus, and window cleanup verified");
    Ok(())
}

#[test]
#[ignore = "reads composited pixels only on the runner's private Xvfb/picom display"]
fn composed_alpha_matches_premultiplied_pixels() -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::protocol::composite::ConnectionExt as _;
    if std::env::var_os("LIBOVERLAY_TEST_PRIVATE_COMPOSITOR").is_none() {
        return Err("run this test through scripts/check_x11.py --composited".into());
    }
    let (conn, screen) = x11rb::connect(None)?;
    wait_for_compositor(&conn, screen)?;
    conn.composite_query_version(0, 4)?.reply()?;
    let root = conn.setup().roots[screen].root;
    let background = conn.generate_id()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        background,
        root,
        0,
        0,
        conn.setup().roots[screen].width_in_pixels,
        conn.setup().roots[screen].height_in_pixels,
        0,
        xproto::WindowClass::INPUT_OUTPUT,
        x11rb::COPY_FROM_PARENT,
        &xproto::CreateWindowAux::new()
            .background_pixel(0)
            .override_redirect(1),
    )?
    .check()?;
    conn.map_window(background)?.check()?;
    let title = format!("liboverlay-composed-alpha-{}", std::process::id());
    let overlay = OverlayBuilder::new()
        .with_title(&title)
        .with_size(64, 32)
        .with_cpu_damage_tracking(true)
        .with_backend(BackendPreference::X11Only)
        .build()?;
    let window = owned_window(&conn, root, &title)?;
    let geometry = conn.get_geometry(window)?.reply()?;
    let composite = conn
        .composite_get_overlay_window(root)?
        .reply()?
        .overlay_win;
    for color in [
        premultiply(200, 80, 40, 128),
        premultiply(40, 160, 80, 160),
        0,
    ] {
        overlay.draw(|pixels, width, _| pixels[width as usize + 1] = color)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            overlay.poll_events()?;
            let image = conn
                .get_image(
                    xproto::ImageFormat::Z_PIXMAP,
                    composite,
                    geometry.x + 1,
                    geometry.y + 1,
                    1,
                    1,
                    u32::MAX,
                )?
                .reply()?;
            let bytes: [u8; 4] = image.data[..4].try_into()?;
            let pixel = if conn.setup().image_byte_order == xproto::ImageOrder::LSB_FIRST {
                u32::from_le_bytes(bytes)
            } else {
                u32::from_be_bytes(bytes)
            };
            if pixel & 0x00ff_ffff == color & 0x00ff_ffff {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "composed alpha mismatch: {pixel:08x}, expected RGB {color:08x}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    overlay.close()?;
    conn.composite_release_overlay_window(root)?.check()?;
    conn.destroy_window(background)?.check()?;
    Ok(())
}
