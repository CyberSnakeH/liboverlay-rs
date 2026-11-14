//! Opt-in native Vulkan lifecycle and callback checks on an owned overlay.
#![cfg(all(feature = "gpu", feature = "wayland"))]
use liboverlay::{
    gpu::wgpu, BackendPreference, OverlayBuilder, OverlayError, RendererPreference, RendererType,
    Size,
};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires Wayland layer-shell and a hardware Vulkan adapter"]
fn callback_commands_and_raii_native_cleanup() -> Result<(), Box<dyn std::error::Error>> {
    // Odd dimensions exercise the fractional-scale rounding boundary on a
    // compositor that announces e.g. 125%, without changing monitor settings.
    let overlay = OverlayBuilder::new()
        .with_title("liboverlay GPU lifecycle test")
        .with_backend(BackendPreference::WaylandOnly)
        .with_renderer(RendererPreference::GpuRequired)
        .with_size(102, 101)
        .with_startup_timeout(Duration::from_secs(20))
        .build()?;
    assert_eq!(
        overlay.backend_report().capabilities.renderer,
        RendererType::Gpu
    );
    let info = overlay.info()?;
    assert_eq!(info.logical_size, Size::new(102, 101));
    assert_eq!(
        info.physical_size,
        info.scale_factor.physical_size(info.logical_size)?
    );
    let context = overlay.gpu_context()?;
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = overlay.clone();
    overlay.draw_gpu(move |frame| {
        let input_rejected = matches!(
            handle.set_click_through(false),
            Err(OverlayError::Unsupported(_))
        );
        let close_rejected = matches!(handle.close(), Err(OverlayError::Unsupported(_)));
        tx.send(input_rejected && close_rejected)
            .map_err(|error| OverlayError::Gpu(error.to_string()))?;
        let _pass = frame
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lifecycle test clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: frame.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.05,
                            g: 0.1,
                            b: 0.2,
                            a: 0.4,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        Ok(())
    })?;
    assert!(
        rx.recv_timeout(Duration::from_secs(3))?,
        "synchronous callback commands were not rejected"
    );
    assert!(overlay.is_running());
    overlay.set_click_through(false)?;
    overlay.set_click_through(true)?;
    println!("info={info:?}; synchronous callback commands rejected");
    // No explicit close: the final handle must wait for native thread cleanup.
    // Resources belonging to this device may outlive the window safely.
    drop(overlay);
    context.device.poll(wgpu::PollType::Poll)?;
    println!("GPU overlay dropped; retained device remains usable");
    Ok(())
}

#[test]
#[ignore = "requires two owned Wayland surfaces and a hardware Vulkan adapter"]
fn closing_one_gpu_window_keeps_the_other_usable() -> Result<(), Box<dyn std::error::Error>> {
    use liboverlay::{Anchor, OverlayEvent, OverlayWindow};
    fn paint_and_wait(window: &OverlayWindow) -> Result<(), Box<dyn std::error::Error>> {
        window.poll_events()?;
        window.draw_gpu(|frame| {
            let _pass = frame
                .encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("independent surface clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: frame.view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.02,
                                g: 0.08,
                                b: 0.04,
                                a: 0.2,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            Ok(())
        })?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if window
                .poll_events()?
                .iter()
                .any(|event| matches!(event, OverlayEvent::FrameSubmitted { .. }))
            {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err("independent GPU surface stopped submitting".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let builder = OverlayBuilder::new()
        .with_size(80, 60)
        .with_backend(BackendPreference::WaylandOnly)
        .with_renderer(RendererPreference::GpuRequired)
        .with_margin(400)
        .with_startup_timeout(Duration::from_secs(20));
    let first = builder
        .clone()
        .with_title("liboverlay independent GPU surface A")
        .with_anchor(Anchor::TopLeft)
        .build()?;
    let second = builder
        .with_title("liboverlay independent GPU surface B")
        .with_anchor(Anchor::BottomRight)
        .build()?;
    let retained = first.gpu_context()?;
    paint_and_wait(&first)?;
    paint_and_wait(&second)?;
    first.close()?;
    assert!(second.is_running());
    paint_and_wait(&second)?;
    second.close()?;
    retained.device.poll(wgpu::PollType::Poll)?;
    println!("Two GPU windows shared instance; closing A preserved rendering on B; retained A device usable");
    Ok(())
}
