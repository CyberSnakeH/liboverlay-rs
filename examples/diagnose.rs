//! Opens a small transparent overlay, submits frames, toggles input without
//! drawing, and exits. No pointer movement or synthetic clicks are generated.
use liboverlay::{
    premultiply, BackendPreference, OverlayBuilder, OverlayError, OverlayEvent, RendererPreference,
    RendererType,
};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let backend = match std::env::args().nth(1).as_deref() {
        Some("wayland") => BackendPreference::WaylandOnly,
        Some("x11") => BackendPreference::X11Only,
        None | Some("auto") => BackendPreference::Auto,
        _ => return Err("backend: auto, wayland or x11".into()),
    };
    let renderer = match std::env::args().nth(2).as_deref() {
        None | Some("cpu") => RendererPreference::Cpu,
        Some("gpu") => RendererPreference::GpuRequired,
        Some("prefer-gpu") => RendererPreference::GpuPreferred,
        _ => return Err("renderer: cpu, gpu or prefer-gpu".into()),
    };
    let transparency = match std::env::args().nth(3).as_deref() {
        None => liboverlay::Requirement::Required,
        Some("allow-opaque") => liboverlay::Requirement::Preferred,
        _ => return Err("third argument: allow-opaque (or omit to require transparency)".into()),
    };
    let overlay = OverlayBuilder::new()
        .with_title("liboverlay diagnostic")
        .with_size(160, 80)
        .with_backend(backend)
        .with_renderer(renderer)
        .with_transparency(transparency)
        .build()?;
    println!(
        "backend={:?}; info={:?}; report={:#?}",
        overlay.backend_type(),
        overlay.info()?,
        overlay.backend_report()
    );
    let started = Instant::now();
    let mut submitted = 0;
    while started.elapsed() < Duration::from_millis(800) {
        for event in overlay.poll_events()? {
            if matches!(event, OverlayEvent::FrameSubmitted { .. }) {
                submitted += 1;
            }
        }
        let draw = if overlay.backend_report().capabilities.renderer == RendererType::Gpu {
            #[cfg(feature = "gpu")]
            {
                overlay.draw_gpu(|frame| {
                    use liboverlay::gpu::wgpu;
                    let _pass = frame
                        .encoder
                        .begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("diagnostic color"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: frame.view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color {
                                        r: 0.0,
                                        g: 0.28,
                                        b: 0.14,
                                        a: 0.39,
                                    }),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });
                    Ok(())
                })
            }
            #[cfg(not(feature = "gpu"))]
            {
                Err(OverlayError::Unsupported("GPU feature"))
            }
        } else {
            overlay.draw(|pixels, _, _| pixels.fill(premultiply(0, 180, 90, 100)))
        };
        match draw {
            Ok(()) | Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. }) => {}
            Err(error) => return Err(error.into()),
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    if submitted == 0 {
        return Err("no frame was submitted to the display server".into());
    }
    // Input updates must work even after drawing stops.
    overlay.set_click_through(false)?;
    overlay.set_click_through(true)?;
    println!(
        "frames_submitted={submitted}; input_toggle=ok; running={}",
        overlay.is_running()
    );
    overlay.close()?;
    println!("closed={}", !overlay.is_running());
    Ok(())
}
