//! Isolated-server tests, driven by scripts/check_x11.py. Never uses the desktop.
#![cfg(feature = "x11")]
use liboverlay::{
    BackendPreference, FramePacing, OverlayBuilder, OverlayError, OverlayEvent, Requirement,
    Support,
};
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection,
    protocol::xproto::{self, ConnectionExt as _},
};

fn overlay(title: &str) -> liboverlay::Result<liboverlay::OverlayWindow> {
    OverlayBuilder::new()
        .with_title(title)
        .with_size(64, 32)
        .with_backend(BackendPreference::X11Only)
        .with_transparency(Requirement::Preferred)
        .build()
}

fn own_window(
    conn: &impl Connection,
    root: u32,
    title: &str,
) -> Result<u32, Box<dyn std::error::Error>> {
    for child in conn.query_tree(root)?.reply()?.children {
        let reply = conn
            .get_property(
                false,
                child,
                xproto::AtomEnum::WM_NAME,
                xproto::AtomEnum::STRING,
                0,
                1024,
            )?
            .reply()?;
        if reply.value == title.as_bytes() {
            return Ok(child);
        }
    }
    Err("test window not found".into())
}

#[test]
#[ignore = "requires the isolated Xvfb runner"]
fn opaque_upload_resize_and_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen) = x11rb::connect(None)?;
    let title = format!("liboverlay-headless-{}", std::process::id());
    let overlay = overlay(&title)?;
    let report = overlay.backend_report();
    assert_eq!(report.capabilities.transparency, Support::Unsupported);
    let expected_pacing = if std::env::var_os("LIBOVERLAY_TEST_NO_PRESENT").is_some() {
        FramePacing::Timer
    } else {
        FramePacing::XPresent
    };
    assert_eq!(report.capabilities.frame_pacing, expected_pacing);
    let fallback = report
        .warnings
        .iter()
        .any(|w| w.contains("CPU uploads use PutImage"));
    assert_eq!(
        fallback,
        std::env::var_os("LIBOVERLAY_TEST_NO_SHM").is_some()
    );
    let root = conn.setup().roots[screen].root;
    let window = own_window(&conn, root, &title)?;
    for (width, height) in [(64, 32), (127, 61), (32, 64), (64, 32)] {
        let stale = overlay.frame()?;
        let before = overlay.info()?;
        conn.configure_window(
            window,
            &xproto::ConfigureWindowAux::new()
                .width(width)
                .height(height),
        )?
        .check()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let info = overlay.info()?;
            if info.physical_size == liboverlay::Size::new(width, height) {
                break;
            }
            assert!(Instant::now() < deadline, "resize was not dispatched");
            std::thread::sleep(Duration::from_millis(2));
        }
        if before.physical_size != overlay.info()?.physical_size {
            assert!(matches!(
                overlay.present(stale),
                Err(OverlayError::OutdatedFrame { .. })
            ));
        }
        for color in [0xff12_3456, 0xffab_cdef, 0xff65_4321] {
            overlay.draw(|pixels, _, _| pixels.fill(color))?;
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                overlay.poll_events()?;
                let image = conn
                    .get_image(xproto::ImageFormat::Z_PIXMAP, window, 0, 0, 1, 1, u32::MAX)?
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
                    "uploaded pixels were not presented"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
    overlay.close()?;
    assert!(conn.get_window_attributes(window)?.reply().is_err());
    Ok(())
}

#[test]
#[ignore = "runner terminates its own isolated Xvfb after READY"]
fn server_disconnect_is_terminal() -> Result<(), Box<dyn std::error::Error>> {
    let overlay = overlay("liboverlay-disconnect")?;
    overlay.draw(|pixels, _, _| pixels.fill(0xff12_3456))?;
    println!("READY disconnect");
    io::stdout().flush()?;
    let mut signal = String::new();
    io::stdin().read_line(&mut signal)?;
    assert_eq!(signal.trim(), "disconnected");
    let deadline = Instant::now() + Duration::from_secs(5);
    while overlay.is_running() {
        assert!(
            Instant::now() < deadline,
            "runtime survived server disconnection"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(
        overlay.poll_events(),
        Err(OverlayError::Backend { .. }) | Err(OverlayError::Io(_))
    ));
    assert!(overlay
        .poll_events()?
        .iter()
        .any(|event| matches!(event, OverlayEvent::Closed)));
    overlay.close()?;
    assert!(matches!(
        overlay.draw(|_, _, _| {}),
        Err(OverlayError::Closed)
    ));
    Ok(())
}

#[test]
#[ignore = "creates and removes a monitor on the runner's isolated Xvfb"]
fn monitor_relocation_and_removal() -> Result<(), Box<dyn std::error::Error>> {
    use liboverlay::{Anchor, OutputSelector};
    use x11rb::protocol::randr::{ConnectionExt as _, MonitorInfo};
    let (conn, screen) = x11rb::connect(None)?;
    let version = conn.randr_query_version(1, 5)?.reply()?;
    assert!((version.major_version, version.minor_version) >= (1, 5));
    let root = conn.setup().roots[screen].root;
    let name = format!("liboverlay-monitor-{}", std::process::id());
    let atom = conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
    let mut monitor = MonitorInfo {
        name: atom,
        primary: false,
        automatic: false,
        x: -320,
        y: -240,
        width: 320,
        height: 240,
        width_in_millimeters: 100,
        height_in_millimeters: 75,
        outputs: Vec::new(),
    };
    conn.randr_set_monitor(root, monitor.clone())?.check()?;
    let title = format!("liboverlay-monitor-window-{}", std::process::id());
    let overlay = OverlayBuilder::new()
        .with_title(&title)
        .with_backend(BackendPreference::X11Only)
        .with_transparency(Requirement::Preferred)
        .with_output(OutputSelector::Name(name))
        .with_anchor(Anchor::Fill)
        .with_margin(8)
        .build()?;
    let window = own_window(&conn, root, &title)?;
    for (x, y, width, height) in [
        (-320, -240, 320, 240),
        (-640, 0, 640, 480),
        (0, -480, 480, 320),
    ] {
        monitor.x = x;
        monitor.y = y;
        monitor.width = width;
        monitor.height = height;
        // SetMonitor rejects an existing name. Replace the definition while
        // the private server is grabbed so geometry queries see one transition.
        conn.grab_server()?.check()?;
        conn.randr_delete_monitor(root, atom)?.check()?;
        conn.randr_set_monitor(root, monitor.clone())?.check()?;
        conn.ungrab_server()?.check()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            overlay.poll_events()?;
            let geometry = conn.get_geometry(window)?.reply()?;
            if (geometry.x, geometry.y, geometry.width, geometry.height)
                == (x + 8, y + 8, width - 16, height - 16)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "monitor relocation was not applied: got ({}, {}, {}, {}), expected ({}, {}, {}, {})",
                geometry.x, geometry.y, geometry.width, geometry.height,
                x + 8, y + 8, width - 16, height - 16
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    conn.randr_delete_monitor(root, atom)?.check()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while overlay.is_running() {
        assert!(
            Instant::now() < deadline,
            "removal of the selected monitor did not close the overlay"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(overlay
        .poll_events()?
        .iter()
        .any(|event| matches!(event, OverlayEvent::Closed)));
    overlay.close()?;
    Ok(())
}

#[test]
#[ignore = "sends scroll messages only to an owned window on isolated Xvfb"]
fn scroll_axes_preserve_direction_and_units() -> Result<(), Box<dyn std::error::Error>> {
    use liboverlay::{ScrollAxis, ScrollUnit};
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let title = format!("liboverlay-scroll-{}", std::process::id());
    let overlay = overlay(&title)?;
    let window = own_window(&conn, root, &title)?;
    for button in 4..=7 {
        let event = xproto::ButtonPressEvent {
            response_type: xproto::BUTTON_PRESS_EVENT,
            detail: button,
            sequence: 0,
            time: x11rb::CURRENT_TIME,
            root,
            event: window,
            child: x11rb::NONE,
            root_x: 0,
            root_y: 0,
            event_x: 0,
            event_y: 0,
            state: xproto::KeyButMask::default(),
            same_screen: true,
        };
        conn.send_event(false, window, xproto::EventMask::BUTTON_PRESS, event)?
            .check()?;
        let release = xproto::ButtonReleaseEvent {
            response_type: xproto::BUTTON_RELEASE_EVENT,
            ..event
        };
        conn.send_event(false, window, xproto::EventMask::BUTTON_RELEASE, release)?
            .check()?;
    }
    let mut axes = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while axes.len() < 4 {
        for event in overlay.poll_events()? {
            match event {
                OverlayEvent::PointerAxis { axis, value, unit } => {
                    assert_eq!(unit, ScrollUnit::Steps);
                    axes.push((axis, value));
                }
                OverlayEvent::PointerButton { .. } => {
                    panic!("scroll buttons leaked as pointer buttons")
                }
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "scroll events were not received");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        axes,
        vec![
            (ScrollAxis::Vertical, -1.0),
            (ScrollAxis::Vertical, 1.0),
            (ScrollAxis::Horizontal, -1.0),
            (ScrollAxis::Horizontal, 1.0)
        ]
    );
    overlay.close()?;
    Ok(())
}

#[test]
#[ignore = "checks full pixel contents on the runner's private Xvfb"]
fn partial_updates_preserve_unchanged_pixels_and_resize() -> Result<(), Box<dyn std::error::Error>>
{
    let (conn, screen) = x11rb::connect(None)?;
    let title = format!("liboverlay-damage-{}", std::process::id());
    let overlay = OverlayBuilder::new()
        .with_title(&title)
        .with_backend(BackendPreference::X11Only)
        .with_transparency(Requirement::Preferred)
        .with_size(64, 32)
        .with_cpu_damage_tracking(true)
        .with_presentation_feedback(true)
        .build()?;
    let window = own_window(&conn, conn.setup().roots[screen].root, &title)?;
    let mut feedback = Vec::new();
    for (width, height) in [(64, 32), (95, 47), (64, 32)] {
        conn.configure_window(
            window,
            &xproto::ConfigureWindowAux::new()
                .width(width)
                .height(height),
        )?
        .check()?;
        let deadline = Instant::now() + Duration::from_secs(3);
        while overlay.info()?.physical_size != liboverlay::Size::new(width, height) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        for index in 0..8 {
            let mut expected = vec![0; (width * height) as usize];
            // Includes repeated identical frames, erased pixels and distant changes.
            let x = if index < 2 { 3 } else { (index * 7) % width };
            for y in 2..height.min(10) {
                expected[(y * width + x) as usize] = 0xff12ab34;
            }
            if index == 5 {
                expected[(width * height - 1) as usize] = 0xffabc123;
            }
            overlay.draw(|p, _, _| p.copy_from_slice(&expected))?;
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                feedback.extend(overlay.poll_events()?);
                let image = conn
                    .get_image(
                        xproto::ImageFormat::Z_PIXMAP,
                        window,
                        0,
                        0,
                        width as u16,
                        height as u16,
                        u32::MAX,
                    )?
                    .reply()?;
                let little = conn.setup().image_byte_order == xproto::ImageOrder::LSB_FIRST;
                let matches = image.data.chunks_exact(4).zip(&expected).all(|(b, p)| {
                    let b: [u8; 4] = b.try_into().unwrap();
                    let value = if little {
                        u32::from_le_bytes(b)
                    } else {
                        u32::from_be_bytes(b)
                    };
                    value & 0xffffff == p & 0xffffff
                });
                if matches {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "partial upload corrupted pixels at frame {index}"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            // Wait for the accepted submission even when its pixels are unchanged.
            while !feedback
                .iter()
                .any(|e| matches!(e, OverlayEvent::FrameSubmitted { .. }))
            {
                assert!(Instant::now() < deadline);
                feedback.extend(overlay.poll_events()?);
                std::thread::sleep(Duration::from_millis(2));
            }
            feedback.retain(|e| !matches!(e, OverlayEvent::FrameSubmitted { .. }));
        }
    }
    if overlay.backend_report().capabilities.presentation_feedback == Support::Supported {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !feedback
            .iter()
            .any(|e| matches!(e, OverlayEvent::FramePresented(_)))
        {
            feedback.extend(overlay.poll_events()?);
            assert!(
                Instant::now() < deadline,
                "Present completion was not reported"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    overlay.close()?;
    Ok(())
}
