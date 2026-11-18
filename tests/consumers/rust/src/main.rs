use liboverlay::{premultiply, BackendPreference, OutputSelector, OverlayBuilder, OverlayError};
use std::io;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(premultiply(255, 0, 0, 128), 0x8080_0000);
    if std::env::args().nth(1).as_deref() != Some("--draw") {
        return Ok(());
    }

    let overlay = OverlayBuilder::new()
        .with_backend(BackendPreference::WaylandOnly)
        .with_output(OutputSelector::Name("HEADLESS-1".into()))
        .with_size(96, 64)
        .build()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match overlay.draw(|pixels, _, _| pixels.fill(premultiply(20, 220, 100, 255))) {
            Ok(()) => break,
            Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. })
                if Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(16));
            }
            Err(error) => return Err(error.into()),
        }
    }
    // The capture process releases the client after it observes the submitted image.
    io::stdin().read_line(&mut String::new())?;
    overlay.close()?;
    Ok(())
}
