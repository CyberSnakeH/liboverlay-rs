//! `cargo run --example basic_hud -- [auto|wayland|x11] [seconds]`
use liboverlay::{premultiply, BackendPreference, OverlayBuilder, OverlayError};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend = match std::env::args().nth(1).as_deref() {
        None | Some("auto") => BackendPreference::Auto,
        Some("wayland") => BackendPreference::WaylandOnly,
        Some("x11") => BackendPreference::X11Only,
        _ => return Err("backend: auto, wayland or x11".into()),
    };
    let seconds = std::env::args()
        .nth(2)
        .map(|s| s.parse::<u64>())
        .transpose()?
        .unwrap_or(10);
    let overlay = OverlayBuilder::new()
        .with_title("liboverlay basic HUD")
        .with_size(360, 120)
        .with_backend(backend)
        .build()?;
    println!("{:#?}", overlay.backend_report());
    let started = Instant::now();
    while overlay.is_running() && started.elapsed() < Duration::from_secs(seconds) {
        overlay.poll_events()?;
        let phase = (started.elapsed().as_secs_f64() * 90.0) as u32;
        let result = overlay.draw(|pixels, width, height| {
            pixels.fill(0);
            let bar_width = 20 + phase % width.saturating_sub(40).max(1);
            for y in 12..height.saturating_sub(12) {
                for x in 12..width.saturating_sub(12) {
                    let color = if y > height / 2 && x < bar_width {
                        premultiply(40, 220, 130, 220)
                    } else {
                        premultiply(20, 35, 60, 155)
                    };
                    pixels[(y * width + x) as usize] = color;
                }
            }
        });
        match result {
            Ok(()) | Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. }) => {}
            Err(error) => return Err(error.into()),
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    overlay.close()?;
    println!("HUD closed cleanly");
    Ok(())
}
