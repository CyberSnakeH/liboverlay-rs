//! Toggles pointer input every two seconds without requesting keyboard focus.
use liboverlay::{premultiply, OverlayBuilder, OverlayError};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let overlay = OverlayBuilder::new()
        .with_title("liboverlay input toggle")
        .with_size(300, 150)
        .build()?;
    println!("{:#?}", overlay.backend_report());
    let started = Instant::now();
    let mut last_mode = None;
    while overlay.is_running() && started.elapsed() < Duration::from_secs(12) {
        let pass_through = (started.elapsed().as_secs() / 2) % 2 == 0;
        if last_mode != Some(pass_through) {
            overlay.set_click_through(pass_through)?;
            println!("click-through = {pass_through}");
            last_mode = Some(pass_through);
        }
        for event in overlay.poll_events()? {
            println!("{event:?}");
        }
        let result = overlay.draw(|pixels, _, _| {
            pixels.fill(if pass_through {
                premultiply(20, 150, 230, 140)
            } else {
                premultiply(230, 120, 20, 170)
            })
        });
        match result {
            Ok(()) | Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. }) => {}
            Err(error) => return Err(error.into()),
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    overlay.close()?;
    Ok(())
}
