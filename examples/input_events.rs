//! Inspect input on an owned interactive surface; close after thirty seconds.
use liboverlay::{
    InputRectangle, InputRegion, OverlayBuilder, OverlayError, OverlayEvent, OverlayWindow,
    TextInputState,
};
use std::time::{Duration, Instant};

fn redraw(overlay: &OverlayWindow) -> liboverlay::Result<()> {
    let info = overlay.info()?;
    let result = overlay.draw(|pixels, width, height| {
        for y in 0..height {
            for x in 0..width {
                let logical_x =
                    u64::from(x) * u64::from(info.logical_size.width) / u64::from(width);
                let logical_y =
                    u64::from(y) * u64::from(info.logical_size.height) / u64::from(height);
                if (16..304).contains(&logical_x) && (16..104).contains(&logical_y) {
                    pixels[(y * width + x) as usize] = liboverlay::premultiply(40, 90, 180, 180);
                }
            }
        }
    });
    match result {
        Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. }) => Ok(()),
        result => result,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let overlay = OverlayBuilder::new()
        .with_title("Input events")
        .with_size(320, 120)
        .with_input_region(InputRegion::Rectangles(vec![InputRectangle::new(
            16, 16, 288, 88,
        )]))
        .with_text_input(TextInputState::enabled())
        .build()?;
    println!("Click the blue area to focus it, then type. Events appear here.");
    println!("{:#?}", overlay.backend_report().capabilities);
    redraw(&overlay)?;
    let start = Instant::now();
    while overlay.is_running() && start.elapsed() < Duration::from_secs(30) {
        for event in overlay.poll_events()? {
            if matches!(event, OverlayEvent::Resized(_)) {
                redraw(&overlay)?;
            }
            println!("{event:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    overlay.close()?;
    Ok(())
}
