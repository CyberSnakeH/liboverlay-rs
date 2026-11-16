//! CPU text HUD with a caller-supplied font: text_hud FONT.ttf [auto|wayland|x11] [seconds].
use fontdue::{
    layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle},
    Font, FontSettings,
};
use liboverlay::{
    premultiply, Anchor, BackendPreference, OverlayBuilder, OverlayError, OverlayEvent, WindowInfo,
};
use std::{
    fs, io,
    time::{Duration, Instant},
};

struct Glyph {
    x: i32,
    y: i32,
    width: usize,
    coverage: Vec<u8>,
}

fn layout(font: &Font, info: WindowInfo) -> Vec<Glyph> {
    let scale = info.scale_factor.as_f64() as f32;
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings {
        x: 16.0 * scale,
        y: 12.0 * scale,
        max_width: Some((info.physical_size.width as f32 - 32.0 * scale).max(1.0)),
        ..Default::default()
    });
    layout.append(&[font], &TextStyle::new("liboverlay\n", 24.0 * scale, 0));
    layout.append(
        &[font],
        &TextStyle::new(
            "CPU text · click-through\nUnicode: café / naïve",
            16.0 * scale,
            0,
        ),
    );
    layout
        .glyphs()
        .iter()
        .filter(|glyph| !glyph.parent.is_control())
        .map(|glyph| {
            let (metrics, coverage) = font.rasterize_config(glyph.key);
            Glyph {
                x: glyph.x as i32,
                y: glyph.y as i32,
                width: metrics.width,
                coverage,
            }
        })
        .collect()
}

fn source_over(destination: u32, source: u32) -> u32 {
    let inverse = 255 - (source >> 24);
    let channel = |shift: u32| -> u32 {
        ((source >> shift) & 255) + (((destination >> shift) & 255) * inverse + 127) / 255
    };
    (channel(24) << 24) | (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let font_path = args
        .next()
        .ok_or("usage: text_hud FONT.ttf [auto|wayland|x11] [seconds]")?;
    let backend = match args.next().as_deref() {
        None | Some("auto") => BackendPreference::Auto,
        Some("wayland") => BackendPreference::WaylandOnly,
        Some("x11") => BackendPreference::X11Only,
        _ => return Err("backend: auto, wayland or x11".into()),
    };
    let seconds = args
        .next()
        .map(|s| s.parse::<u64>())
        .transpose()?
        .unwrap_or(10);
    if args.next().is_some() || seconds == 0 || seconds > 3600 {
        return Err("duration must be in 1..=3600 seconds".into());
    }
    if fs::metadata(&font_path)?.len() > 16 * 1024 * 1024 {
        return Err("example font limit: 16 MiB".into());
    }
    let font = Font::from_bytes(fs::read(font_path)?, FontSettings::default())
        .map_err(io::Error::other)?;
    let overlay = OverlayBuilder::new()
        .with_title("liboverlay text HUD")
        .with_backend(backend)
        .with_size(420, 120)
        .with_anchor(Anchor::TopLeft)
        .with_margin(16)
        .build()?;
    let start = Instant::now();
    let mut generation = u64::MAX;
    let mut glyphs = Vec::new();
    let mut submitted = 0;
    while overlay.is_running() && start.elapsed() < Duration::from_secs(seconds) {
        for event in overlay.poll_events()? {
            if matches!(event, OverlayEvent::FrameSubmitted { .. }) {
                submitted += 1;
            }
        }
        let info = overlay.info()?;
        if info.generation != generation {
            glyphs = layout(&font, info);
            generation = info.generation;
        }
        match overlay.draw(|pixels, width, height| {
            pixels.fill(premultiply(20, 24, 30, 210));
            for glyph in &glyphs {
                if glyph.width == 0 {
                    continue;
                }
                for (offset, alpha) in glyph.coverage.iter().enumerate() {
                    let x = i64::from(glyph.x) + (offset % glyph.width) as i64;
                    let y = i64::from(glyph.y) + (offset / glyph.width) as i64;
                    if x >= 0 && y >= 0 && x < i64::from(width) && y < i64::from(height) {
                        let index = y as usize * width as usize + x as usize;
                        pixels[index] =
                            source_over(pixels[index], premultiply(240, 245, 255, *alpha));
                    }
                }
            }
        }) {
            Ok(()) | Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. }) => {}
            Err(error) => return Err(error.into()),
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    overlay.close()?;
    if submitted == 0 {
        return Err("no text frame was submitted".into());
    }
    println!("text frames submitted={submitted}");
    Ok(())
}
