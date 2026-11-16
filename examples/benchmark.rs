//! Transparent CPU/Vulkan presentation benchmark. Measures submission, not scanout.
use liboverlay::{
    BackendPreference, OverlayBuilder, OverlayError, OverlayEvent, RendererPreference,
    RendererType, Size,
};
use std::{
    fs::File,
    io::{BufWriter, Write},
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Options {
    backend: BackendPreference,
    renderer: RendererPreference,
    size: Size,
    seconds: u64,
    fps: u32,
    output: String,
    scene: String,
    damage: bool,
    monitor: Option<String>,
}
impl Options {
    fn parse() -> Result<Self> {
        let mut result = Self {
            backend: BackendPreference::Auto,
            renderer: RendererPreference::Cpu,
            size: Size::new(640, 360),
            seconds: 10,
            fps: 60,
            output: "benchmark.tsv".into(),
            scene: "transparent".into(),
            damage: false,
            monitor: None,
        };
        let mut arguments = std::env::args().skip(1);
        while let Some(name) = arguments.next() {
            let value = arguments.next().ok_or("each option needs a value")?;
            match name.as_str() {
                "--backend" => {
                    result.backend = match value.as_str() {
                        "auto" => BackendPreference::Auto,
                        "wayland" => BackendPreference::WaylandOnly,
                        "x11" => BackendPreference::X11Only,
                        _ => return Err("backend: auto, wayland or x11".into()),
                    }
                }
                "--renderer" => {
                    result.renderer = match value.as_str() {
                        "cpu" => RendererPreference::Cpu,
                        "gpu" => RendererPreference::GpuRequired,
                        "prefer-gpu" => RendererPreference::GpuPreferred,
                        _ => return Err("renderer: cpu, gpu or prefer-gpu".into()),
                    }
                }
                "--size" => {
                    let (width, height) = value.split_once('x').ok_or("size: WIDTHxHEIGHT")?;
                    result.size = Size::new(width.parse()?, height.parse()?);
                }
                "--seconds" => result.seconds = value.parse()?,
                "--fps" => result.fps = value.parse()?,
                "--output" => result.output = value,
                "--monitor" => result.monitor = Some(value),
                "--scene"
                    if matches!(
                        value.as_str(),
                        "transparent" | "counter" | "moving" | "full"
                    ) =>
                {
                    result.scene = value
                }
                "--damage" => {
                    result.damage = match value.as_str() {
                        "full" => false,
                        "partial" => true,
                        _ => return Err("damage: full or partial".into()),
                    }
                }
                _ => return Err(format!("unknown option: {name}").into()),
            }
        }
        if result.seconds == 0 || result.seconds > 3600 || result.fps == 0 || result.fps > 1000 {
            return Err("seconds must be in 1..=3600 and fps in 1..=1000".into());
        }
        Ok(result)
    }
}

fn main() -> Result<()> {
    let options = Options::parse()?;
    if options.scene != "transparent"
        && std::env::var_os("LIBOVERLAY_TEST_PRIVATE_BENCHMARK").is_none()
    {
        return Err(
            "colored benchmark scenes require scripts/benchmark_damage.py on a private display"
                .into(),
        );
    }
    let overlay = OverlayBuilder::new()
        .with_title("liboverlay presentation benchmark")
        .with_backend(options.backend)
        .with_renderer(options.renderer)
        .with_cpu_damage_tracking(options.damage)
        .with_output(
            options
                .monitor
                .clone()
                .map(liboverlay::OutputSelector::Name)
                .unwrap_or(liboverlay::OutputSelector::Automatic),
        )
        .with_size(options.size.width, options.size.height)
        .build()?;
    println!(
        "report={:#?}; geometry={:?}",
        overlay.backend_report(),
        overlay.info()?
    );
    let renderer = overlay.backend_report().capabilities.renderer;
    let mut output = BufWriter::new(File::create(&options.output)?);
    writeln!(
        output,
        "elapsed_ms\tframe\tsubmission_latency_us\tphysical_width\tphysical_height"
    )?;
    let start = Instant::now();
    let interval = Duration::from_secs_f64(1.0 / f64::from(options.fps));
    let mut next = start;
    let mut pending: Option<Instant> = None;
    let mut queued = 0u64;
    let mut submitted = 0u64;
    let mut discarded = 0u64;
    let mut latency_sum = 0u128;
    let mut maximum_latency = 0u128;
    let mut presentation_pauses = 0;
    while start.elapsed() < Duration::from_secs(options.seconds) {
        for event in overlay.poll_events()? {
            match event {
                OverlayEvent::FrameSubmitted { .. } => {
                    if let Some(queued_at) = pending.take() {
                        let latency = queued_at.elapsed().as_micros();
                        latency_sum += latency;
                        maximum_latency = maximum_latency.max(latency);
                        submitted += 1;
                        let info = overlay.info()?;
                        writeln!(
                            output,
                            "{}\t{submitted}\t{latency}\t{}\t{}",
                            start.elapsed().as_millis(),
                            info.physical_size.width,
                            info.physical_size.height
                        )?;
                    }
                }
                OverlayEvent::FrameDiscarded => {
                    pending = None;
                    discarded += 1;
                }
                OverlayEvent::Closed => return Err("display runtime closed".into()),
                _ => {}
            }
        }
        if pending.is_some_and(|time| time.elapsed() > Duration::from_secs(5)) {
            // Callback pacing can stop for hidden Wayland surfaces. End this
            // sample explicitly instead of assigning a fabricated throughput.
            presentation_pauses += 1;
            break;
        }
        let now = Instant::now();
        if pending.is_none() && now >= next {
            let result = if renderer == RendererType::Cpu {
                overlay.draw(|pixels, width, height| {
                    scene(pixels, width, height, queued, &options.scene)
                })
            } else {
                #[cfg(feature = "gpu")]
                {
                    overlay.draw_gpu(|_| Ok(()))
                }
                #[cfg(not(feature = "gpu"))]
                {
                    Err(OverlayError::Unsupported("GPU feature"))
                }
            };
            match result {
                Ok(()) => {
                    queued += 1;
                    pending = Some(now);
                    next = now + interval;
                }
                Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. }) => {}
                Err(error) => return Err(error.into()),
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let elapsed = start.elapsed();
    overlay.close()?;
    output.flush()?;
    if submitted == 0 {
        return Err("no measured submissions".into());
    }
    println!("queued={queued} submitted={submitted} discarded={discarded} presentation_pauses={presentation_pauses} submitted_per_second={:.3} mean_submission_us={} max_submission_us={maximum_latency}",
        submitted as f64 / elapsed.as_secs_f64(), latency_sum / u128::from(submitted));
    Ok(())
}

fn scene(pixels: &mut [u32], width: u32, height: u32, frame: u64, scene: &str) {
    pixels.fill(0);
    if scene == "transparent" {
        return;
    }
    let color = liboverlay::premultiply(20 + (frame % 120) as u8, 160, 210, 220);
    if scene == "full" {
        pixels.fill(color);
        return;
    }
    let mut rectangle = |x: u32, y: u32, w: u32, h: u32| {
        for row in y..(y + h).min(height) {
            for column in x..(x + w).min(width) {
                pixels[(row * width + column) as usize] = color;
            }
        }
    };
    if scene == "moving" {
        let x = (frame * 7 % u64::from(width.saturating_sub(96).max(1))) as u32;
        rectangle(x, height / 2, 96, 48);
    } else {
        // A changing three-digit status counter, rendered as seven segments.
        let digits = [0x3f, 0x06, 0x5b, 0x4f, 0x66, 0x6d, 0x7d, 0x07, 0x7f, 0x6f];
        for i in 0..3 {
            let digit = ((frame / 10u64.pow(i)) % 10) as usize;
            let x = 8 + (2 - i) * 20;
            for (bit, (dx, dy, w, h)) in [
                (2, 0, 12, 3),
                (14, 3, 3, 10),
                (14, 16, 3, 10),
                (2, 26, 12, 3),
                (0, 16, 3, 10),
                (0, 3, 3, 10),
                (2, 13, 12, 3),
            ]
            .into_iter()
            .enumerate()
            {
                if digits[digit] & (1 << bit) != 0 {
                    rectangle(x + dx, 8 + dy, w, h);
                }
            }
        }
    }
}
