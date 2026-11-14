//! `cargo run --features gpu --example gpu_hud -- [auto|wayland|x11] [seconds] [allow-software]`
use liboverlay::{
    gpu::wgpu, BackendPreference, OverlayBuilder, OverlayError, OverlayEvent, RendererPreference,
};
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
    let software = std::env::args().nth(3).as_deref() == Some("allow-software");
    let overlay = OverlayBuilder::new()
        .with_title("liboverlay Vulkan HUD")
        .with_size(360, 120)
        .with_backend(backend)
        .with_renderer(RendererPreference::GpuRequired)
        .with_software_gpu(software)
        .with_startup_timeout(Duration::from_secs(20))
        .build()?;
    println!(
        "report={:#?}; info={:?}",
        overlay.backend_report(),
        overlay.info()?
    );
    let context = overlay.gpu_context()?;
    let shader = context.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("HUD triangle"),
        source: wgpu::ShaderSource::Wgsl(r#"
            @vertex fn vertex_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                var vertices = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
                return vec4(vertices[i], 0.0, 1.0);
            }
            @fragment fn fragment_main() -> @location(0) vec4<f32> {
                return vec4(0.04, 0.28, 0.20, 0.45);
            }
        "#.into()),
    });
    let pipeline = std::sync::Arc::new(context.device.create_render_pipeline(
        &wgpu::RenderPipelineDescriptor {
            label: Some("HUD pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vertex_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: context.format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        },
    ));
    let started = Instant::now();
    let mut submitted = 0;
    while overlay.is_running() && started.elapsed() < Duration::from_secs(seconds) {
        for event in overlay.poll_events()? {
            if matches!(event, OverlayEvent::FrameSubmitted { .. }) {
                submitted += 1;
            }
        }
        let phase = started.elapsed().as_secs_f64();
        let pipeline = pipeline.clone();
        match overlay.draw_gpu(move |frame| {
            let mut pass = frame
                .encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("HUD color"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: frame.view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            let width = frame.info.physical_size.width;
            let height = frame.info.physical_size.height;
            let margin = ((12.0 * frame.info.scale_factor.as_f64()).round() as u32)
                .min(width / 4)
                .min(height / 4);
            let available = width - 2 * margin;
            let bar = ((f64::from(available) * (0.60 + 0.30 * phase.sin())).round() as u32)
                .clamp(1, available);
            pass.set_pipeline(&pipeline);
            pass.set_scissor_rect(margin, margin, bar, height - 2 * margin);
            pass.draw(0..3, 0..1);
            Ok(())
        }) {
            Ok(()) | Err(OverlayError::QueueFull) => {}
            Err(error) => return Err(error.into()),
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    overlay.set_click_through(false)?;
    overlay.set_click_through(true)?;
    overlay.close()?;
    // Devices may be retained by application resources after the window closes.
    context.device.poll(wgpu::PollType::Poll)?;
    println!(
        "frames_submitted={submitted}; input_toggle=ok; closed={}",
        !overlay.is_running()
    );
    if submitted == 0 && seconds > 0 {
        return Err("GPU did not submit a frame".into());
    }
    Ok(())
}
