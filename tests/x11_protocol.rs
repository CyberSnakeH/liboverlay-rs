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

#[test]
#[ignore = "opens a temporary X11 overlay; requires a composited local display"]
fn alpha_input_region_focus_and_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen) = x11rb::connect(None)?;
    let original_focus = conn.get_input_focus()?.reply()?.focus;
    let title = format!("liboverlay-protocol-test-{}", std::process::id());
    let overlay = OverlayBuilder::new()
        .with_title(&title)
        .with_size(64, 32)
        .with_backend(BackendPreference::X11Only)
        .build()?;
    let root = conn.setup().roots[screen].root;
    let mut window = None;
    for child in conn.query_tree(root)?.reply()?.children {
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
            window = Some(child);
            break;
        }
    }
    let window = window.ok_or("diagnostic window not found")?;
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
