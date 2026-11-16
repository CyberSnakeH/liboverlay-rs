//! Inspect server presentation clocks and quality flags on a transparent surface.
use liboverlay::{
    BackendPreference, OverlayBuilder, OverlayEvent, RendererPreference, RendererType, Support,
};
use std::{
    fs::File,
    io::{BufWriter, Write},
    time::{Duration, Instant},
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
fn monotonic() -> Duration {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
}
fn main() -> Result {
    let mut backend = BackendPreference::Auto;
    let mut renderer = RendererPreference::Cpu;
    let mut seconds = 10u64;
    let mut fps = 240u32;
    let mut path = "presentation.tsv".to_owned();
    let mut monitor = liboverlay::OutputSelector::Automatic;
    let mut arguments = std::env::args().skip(1);
    while let Some(name) = arguments.next() {
        let value = arguments.next().ok_or("each option requires a value")?;
        match name.as_str() {
            "--backend" => {
                backend = match value.as_str() {
                    "wayland" => BackendPreference::WaylandOnly,
                    "x11" => BackendPreference::X11Only,
                    "auto" => BackendPreference::Auto,
                    _ => return Err("backend: auto, wayland or x11".into()),
                }
            }
            "--renderer" => {
                renderer = match value.as_str() {
                    "cpu" => RendererPreference::Cpu,
                    "gpu" => RendererPreference::GpuRequired,
                    _ => return Err("renderer: cpu or gpu".into()),
                }
            }
            "--seconds" => seconds = value.parse()?,
            "--fps" => fps = value.parse()?,
            "--output" => path = value,
            "--monitor" => monitor = liboverlay::OutputSelector::Name(value),
            _ => return Err(format!("unknown option {name}").into()),
        }
    }
    if !(1..=3600).contains(&seconds) || !(1..=1000).contains(&fps) {
        return Err("seconds: 1..3600; fps: 1..1000".into());
    }
    let overlay = OverlayBuilder::new()
        .with_title("Presentation timing")
        .with_backend(backend)
        .with_renderer(renderer)
        .with_output(monitor)
        .with_size(64, 32)
        .with_presentation_feedback(true)
        .build()?;
    println!("{:#?}", overlay.backend_report());
    if overlay.backend_report().capabilities.presentation_feedback != Support::Supported {
        return Err("presentation feedback is unavailable for this renderer".into());
    }
    let mut output = BufWriter::new(File::create(path)?);
    writeln!(output,"elapsed_ms\tsubmission\tclock_id\tpresent_ns\tsequence\trefresh_ns\tqueued_to_present_us\treceipt_us\tvsync\thw_clock\thw_completion\tzero_copy")?;
    let start = Instant::now();
    let interval = Duration::from_secs_f64(1. / f64::from(fps));
    let mut next = start;
    let mut pending: Option<(Instant, Duration)> = None;
    let mut presented = 0u64;
    let mut discarded = 0u64;
    while start.elapsed() < Duration::from_secs(seconds) {
        for event in overlay.poll_events()? {
            match event {
                OverlayEvent::FramePresented(feedback) => {
                    if let Some((queued, clock)) = pending.take() {
                        let latency = if feedback.clock_id == Some(1) {
                            feedback
                                .timestamp
                                .checked_sub(clock)
                                .map(|d| d.as_micros().to_string())
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        writeln!(
                            output,
                            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                            start.elapsed().as_millis(),
                            feedback.submission,
                            feedback.clock_id.map(|c| c.to_string()).unwrap_or_default(),
                            feedback.timestamp.as_nanos(),
                            feedback.sequence,
                            feedback
                                .refresh_interval
                                .map(|d| d.as_nanos().to_string())
                                .unwrap_or_default(),
                            latency,
                            queued.elapsed().as_micros(),
                            feedback.vsync,
                            feedback.hardware_clock,
                            feedback.hardware_completion,
                            feedback.zero_copy
                        )?;
                        presented += 1;
                    }
                }
                OverlayEvent::PresentationDiscarded { .. } | OverlayEvent::FrameDiscarded => {
                    pending = None;
                    discarded += 1;
                }
                OverlayEvent::Closed => return Err("overlay closed during measurement".into()),
                _ => {}
            }
        }
        if pending.is_some_and(|(at, _)| at.elapsed() > Duration::from_secs(5)) {
            return Err("no presentation feedback for five seconds".into());
        }
        if pending.is_none() && Instant::now() >= next {
            let clock = monotonic();
            let at = Instant::now();
            if overlay.backend_report().capabilities.renderer == RendererType::Cpu {
                overlay.draw(|p, _, _| p.fill(0))?;
            } else {
                #[cfg(feature = "gpu")]
                overlay.draw_gpu(|_| Ok(()))?;
                #[cfg(not(feature = "gpu"))]
                return Err("GPU feature required".into());
            }
            pending = Some((at, clock));
            next = at + interval;
        }
        std::thread::sleep(Duration::from_micros(250));
    }
    overlay.close()?;
    output.flush()?;
    if presented == 0 {
        return Err(format!("no presentations; discarded={discarded}").into());
    }
    println!("presented={presented} discarded={discarded}; server timing excludes optical response and physical input latency");
    Ok(())
}
