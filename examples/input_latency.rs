//! Manual input-to-light target. Run explicitly with external measurement equipment.
use liboverlay::{ButtonState, KeyboardMode, OverlayBuilder, OverlayEvent, Support};
use std::{
    fs::File,
    io::{BufWriter, Write},
    time::{Duration, Instant},
};
fn monotonic_ns() -> u128 {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    time.tv_sec as u128 * 1_000_000_000 + time.tv_nsec as u128
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "input-latency.tsv".into());
    let overlay = OverlayBuilder::new()
        .with_title("Input latency target")
        .with_size(320, 180)
        .with_click_through(false)
        .with_keyboard_mode(KeyboardMode::OnDemand)
        .with_presentation_feedback(true)
        .build()?;
    let feedback =
        overlay.backend_report().capabilities.presentation_feedback == Support::Supported;
    let mut output = BufWriter::new(File::create(path)?);
    writeln!(
        output,
        "kind\tsample\tobserved_monotonic_ns\tpresent_ns\tclock_id\tsubmission\twhite"
    )?;
    println!(
        "Click the rectangle or press Space to alternate black and white. Stops after 60 seconds."
    );
    println!("{:#?}", overlay.backend_report());
    overlay.draw(|p, _, _| p.fill(0xff000000))?;
    let start = Instant::now();
    let mut pending = Some(0u64);
    let mut sample = 0u64;
    let mut white = false;
    while overlay.is_running() && start.elapsed() < Duration::from_secs(60) {
        for event in overlay.poll_events()? {
            match event {
                OverlayEvent::FramePresented(p) => {
                    if let Some(id) = pending.take() {
                        writeln!(
                            output,
                            "present\t{id}\t{}\t{}\t{}\t{}\t{white}",
                            monotonic_ns(),
                            p.timestamp.as_nanos(),
                            p.clock_id.map(|c| c.to_string()).unwrap_or_default(),
                            p.submission
                        )?;
                        output.flush()?;
                    }
                }
                OverlayEvent::PresentationDiscarded { .. } | OverlayEvent::FrameDiscarded => {
                    pending = None;
                }
                OverlayEvent::FrameSubmitted { .. } if !feedback => {
                    pending = None;
                }
                event if pending.is_none() && is_trigger(&event) => {
                    let observed = monotonic_ns();
                    let next = !white;
                    overlay.draw(|p, _, _| p.fill(if next { 0xffffffff } else { 0xff000000 }))?;
                    white = next;
                    sample += 1;
                    pending = Some(sample);
                    writeln!(output, "input\t{sample}\t{observed}\t\t\t\t{white}")?;
                    output.flush()?;
                }
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_micros(250));
    }
    overlay.close()?;
    output.flush()?;
    Ok(())
}

fn is_trigger(event: &OverlayEvent) -> bool {
    matches!(
        event,
        OverlayEvent::PointerButton {
            state: ButtonState::Pressed,
            ..
        }
    ) || matches!(event, OverlayEvent::KeyboardInput(key)
            if key.code == 57 && key.state == ButtonState::Pressed && !key.repeat)
}
