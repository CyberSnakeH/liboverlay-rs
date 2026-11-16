//! Opt-in hardware Vulkan validation on an owned, passive Wayland surface.
//! `cargo run --features gpu --example gpu_stability -- soak 300 metrics.tsv`
//! `cargo run --features gpu --example gpu_stability -- cycles 100 metrics.tsv`
use liboverlay::{
    gpu::{wgpu, GpuContext},
    Anchor, BackendPreference, OverlayBuilder, OverlayError, OverlayEvent, OverlayWindow,
    RendererPreference, Size,
};
use std::{
    fs::{self, File},
    io::{self, Write},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, Copy)]
struct Resources {
    rss_kib: u64,
    fds: usize,
    threads: usize,
}
impl Resources {
    fn read() -> Result<Self> {
        let status = fs::read_to_string("/proc/self/status")?;
        let value = |name: &str| -> Result<u64> {
            Ok(status
                .lines()
                .find_map(|line| line.strip_prefix(name))
                .ok_or_else(|| io::Error::other(format!("missing {name}")))?
                .split_whitespace()
                .next()
                .ok_or_else(|| io::Error::other("missing value"))?
                .parse()?)
        };
        Ok(Self {
            rss_kib: value("VmRSS:")?,
            fds: fs::read_dir("/proc/self/fd")?.count(),
            threads: value("Threads:")? as usize,
        })
    }
}

struct Metrics {
    file: File,
    start: Instant,
}
impl Metrics {
    fn new(path: &Path) -> Result<Self> {
        let mut file = File::create(path)?;
        writeln!(
            file,
            "phase\tindex\telapsed_ms\tsubmitted\tcallbacks\trss_kib\tfds\tthreads"
        )?;
        Ok(Self {
            file,
            start: Instant::now(),
        })
    }
    fn sample(
        &mut self,
        phase: &str,
        index: u64,
        submitted: u64,
        callbacks: u64,
    ) -> Result<Resources> {
        let resources = Resources::read()?;
        let elapsed = self.start.elapsed().as_millis();
        writeln!(
            self.file,
            "{phase}\t{index}\t{elapsed}\t{submitted}\t{callbacks}\t{}\t{}\t{}",
            resources.rss_kib, resources.fds, resources.threads
        )?;
        self.file.flush()?;
        if std::env::var_os("LIBOVERLAY_VALIDATION_DETAILS").is_some() {
            let mut descriptors = fs::read_dir("/proc/self/fd")?
                .filter_map(std::result::Result::ok)
                .filter_map(|entry| {
                    fs::read_link(entry.path()).ok().map(|target| {
                        format!(
                            "{}={}",
                            entry.file_name().to_string_lossy(),
                            target.display()
                        )
                    })
                })
                .collect::<Vec<_>>();
            descriptors.sort();
            println!("descriptors={descriptors:?}");
        }
        println!("phase={phase} index={index} elapsed_ms={elapsed} submitted={submitted} callbacks={callbacks} resources={resources:?}");
        io::stdout().flush()?;
        Ok(resources)
    }
}

fn window(size: Size) -> Result<OverlayWindow> {
    Ok(OverlayBuilder::new()
        .with_title("liboverlay GPU stability validation")
        .with_size(size.width, size.height)
        .with_anchor(Anchor::TopRight)
        .with_margin(16)
        .with_backend(BackendPreference::WaylandOnly)
        .with_renderer(RendererPreference::GpuRequired)
        .with_startup_timeout(Duration::from_secs(20))
        .build()?)
}

fn pipeline(context: &GpuContext) -> wgpu::RenderPipeline {
    let color = if std::env::var_os("LIBOVERLAY_VALIDATION_VISIBLE").is_some() {
        "vec4(0.04, 0.24, 0.12, 0.4)"
    } else {
        "vec4(0.0)"
    };
    let source = r#"
            @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                var vertices = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
                return vec4(vertices[i], 0.0, 1.0);
            }
            @fragment fn fs() -> @location(0) vec4<f32> {
                return COLOR;
            }
        "#.replace("COLOR", color);
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("stability shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
    context
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("stability pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
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
        })
}

fn draw(
    overlay: &OverlayWindow,
    pipeline: &Arc<wgpu::RenderPipeline>,
    callbacks: &Arc<AtomicU64>,
    index: u64,
) -> Result<()> {
    let pipeline = pipeline.clone();
    let callbacks = callbacks.clone();
    match overlay.draw_gpu(move |frame| {
        let mut pass = frame
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("stability draw"),
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
        let size = frame.info.physical_size;
        let width = ((index as u32 % 4 + 1) * size.width / 6).max(1);
        pass.set_pipeline(&pipeline);
        pass.set_scissor_rect(4, 4, width.min(size.width - 8), size.height - 8);
        pass.draw(0..3, 0..1);
        callbacks.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }) {
        Ok(()) | Err(OverlayError::QueueFull) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn submitted(overlay: &OverlayWindow) -> Result<u64> {
    let events = overlay.poll_events()?;
    if !overlay.is_running() {
        return Err(io::Error::other("GPU runtime stopped unexpectedly").into());
    }
    Ok(events
        .iter()
        .filter(|event| matches!(event, OverlayEvent::FrameSubmitted { .. }))
        .count() as u64)
}

fn wait_for_gpu(context: &GpuContext) -> Result<()> {
    let finished = Arc::new(AtomicBool::new(false));
    let marker = finished.clone();
    context
        .queue
        .on_submitted_work_done(move || marker.store(true, Ordering::Release));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !finished.load(Ordering::Acquire) {
        context.device.poll(wgpu::PollType::Poll)?;
        if Instant::now() > deadline {
            return Err(io::Error::other("GPU queue did not finish in five seconds").into());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

fn soak(seconds: u64, size: Size, metrics: &mut Metrics) -> Result<()> {
    let overlay = window(size)?;
    println!(
        "report={:#?}; info={:?}",
        overlay.backend_report(),
        overlay.info()?
    );
    let context = overlay.gpu_context()?;
    let pipeline = Arc::new(pipeline(&context));
    let callbacks = Arc::new(AtomicU64::new(0));
    let start = Instant::now();
    let mut last_submission = start;
    let mut next_sample = start;
    let mut count = 0;
    let mut attempts = 0;
    let mut paused = false;
    let mut pauses = 0;
    let mut longest_gap = Duration::ZERO;
    let mut samples = Vec::new();
    while start.elapsed() < Duration::from_secs(seconds) {
        let n = submitted(&overlay)?;
        count += n;
        if n > 0 {
            longest_gap = longest_gap.max(last_submission.elapsed());
            if paused {
                println!(
                    "presentation resumed after {} ms",
                    last_submission.elapsed().as_millis()
                );
            }
            paused = false;
            last_submission = Instant::now();
        }
        if last_submission.elapsed() > Duration::from_secs(5) && !paused {
            // Wayland may withhold callbacks while a surface/output is hidden.
            // Keep checking runtime/device health and record the pacing pause.
            pauses += 1;
            paused = true;
            println!("presentation paused; runtime is healthy, waiting for compositor pacing");
        }
        draw(&overlay, &pipeline, &callbacks, attempts)?;
        attempts += 1;
        if Instant::now() >= next_sample {
            samples.push(metrics.sample(
                if paused { "soak_paused" } else { "soak" },
                start.elapsed().as_secs(),
                count,
                callbacks.load(Ordering::Relaxed),
            )?);
            next_sample = Instant::now() + Duration::from_secs(10);
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    wait_for_gpu(&context)?;
    count += submitted(&overlay)?;
    metrics.sample(
        "soak_end",
        seconds,
        count,
        callbacks.load(Ordering::Relaxed),
    )?;
    overlay.close()?;
    // Application-owned pipelines/device clones deliberately outlive the window.
    wait_for_gpu(&context)?;
    drop(overlay);
    drop(pipeline);
    drop(context);
    metrics.sample(
        "soak_closed",
        seconds,
        count,
        callbacks.load(Ordering::Relaxed),
    )?;
    if count == 0 {
        return Err(io::Error::other("no frames were submitted").into());
    }
    println!(
        "PASS soak seconds={seconds} submitted={count} samples={} pauses={pauses} longest_submission_gap_ms={}",
        samples.len(), longest_gap.max(last_submission.elapsed()).as_millis()
    );
    Ok(())
}

fn cycles(count: u64, size: Size, metrics: &mut Metrics) -> Result<()> {
    let mut samples = Vec::new();
    let mut total_frames = 0;
    let mut total_callbacks = 0;
    for index in 1..=count {
        {
            let overlay = window(size)?;
            if index == 1 {
                println!("report={:#?}", overlay.backend_report());
            }
            let context = overlay.gpu_context()?;
            let pipeline = Arc::new(pipeline(&context));
            let callbacks = Arc::new(AtomicU64::new(0));
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut frames = 0;
            while frames < 3 {
                frames += submitted(&overlay)?;
                if Instant::now() > deadline {
                    return Err(io::Error::other(format!("cycle {index} stalled")).into());
                }
                draw(&overlay, &pipeline, &callbacks, frames)?;
                std::thread::sleep(Duration::from_millis(8));
            }
            wait_for_gpu(&context)?;
            total_frames += frames;
            total_callbacks += callbacks.load(Ordering::Relaxed);
            if index % 2 == 0 {
                let clone = overlay.clone();
                overlay.close()?;
                assert!(!clone.is_running(), "close did not close the shared window");
                drop(clone);
            }
            // Odd cycles exercise RAII closure. GPU resources survive either path.
            drop(overlay);
            wait_for_gpu(&context)?;
            drop(pipeline);
            drop(context);
        }
        samples.push(metrics.sample("cycle_closed", index, total_frames, total_callbacks)?);
    }
    if count >= 20 {
        let warm = &samples[9..19];
        let last = &samples[samples.len() - 10..];
        let warm_fds = warm.iter().map(|s| s.fds).max().unwrap();
        let warm_threads = warm.iter().map(|s| s.threads).max().unwrap();
        let final_fds = last.iter().map(|s| s.fds).max().unwrap();
        let final_threads = last.iter().map(|s| s.threads).max().unwrap();
        let warm_rss = warm.iter().map(|s| s.rss_kib).max().unwrap();
        let final_rss = last.iter().map(|s| s.rss_kib).max().unwrap();
        if final_fds > warm_fds + 2 || final_threads > warm_threads + 2 {
            return Err(io::Error::other(format!("resource growth after warmup: fds {warm_fds}->{final_fds}, threads {warm_threads}->{final_threads}")).into());
        }
        // A coarse regression bound, allowing allocator/driver caches. It is
        // not a proof that every allocation was returned to the operating system.
        if final_rss > warm_rss + 32 * 1024 {
            return Err(io::Error::other(format!(
                "RSS growth after warmup: {warm_rss}->{final_rss} KiB"
            ))
            .into());
        }
    }
    println!("PASS cycles count={count} submitted={total_frames} callbacks={total_callbacks}");
    Ok(())
}

fn instances(count: u64, metrics: &mut Metrics) -> Result<()> {
    for index in 1..=count {
        {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..Default::default()
            });
            let adapters = instance.enumerate_adapters(wgpu::Backends::VULKAN);
            println!(
                "instance_probe={index} adapters={:?}",
                adapters
                    .iter()
                    .map(|a| a.get_info().name)
                    .collect::<Vec<_>>()
            );
        }
        metrics.sample("instance_closed", index, 0, 0)?;
    }
    Ok(())
}

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args().collect();
    let mode = arguments.get(1).map(String::as_str).unwrap_or("soak");
    let count = arguments
        .get(2)
        .map(|s| s.parse::<u64>())
        .transpose()?
        .unwrap_or(if mode == "soak" { 300 } else { 100 });
    let limit = match mode {
        "soak" => 86_400,
        "cycles" | "instances" => 100_000,
        _ => return Err(io::Error::other("mode must be soak, cycles or instances").into()),
    };
    if count == 0 || count > limit {
        return Err(io::Error::other(format!("count/duration must be in 1..={limit}")).into());
    }
    let dimension = |index: usize, default: u32| -> Result<u32> {
        Ok(arguments
            .get(index)
            .map(|s| s.parse())
            .transpose()?
            .unwrap_or(default))
    };
    if arguments.len() > 6 || (arguments.len() == 5) {
        return Err(
            "usage: gpu_stability [soak|cycles|instances] [count] [metrics.tsv] [width height]"
                .into(),
        );
    }
    let size = Size::new(dimension(4, 160)?, dimension(5, 90)?);
    if size.width < 16 || size.height < 16 {
        return Err("surface dimensions must be at least 16 pixels".into());
    }
    let path = arguments
        .get(3)
        .map(String::as_str)
        .unwrap_or("gpu-stability.tsv");
    let mut metrics = Metrics::new(Path::new(path))?;
    metrics.sample("baseline", 0, 0, 0)?;
    match mode {
        "soak" => soak(count, size, &mut metrics),
        "cycles" => cycles(count, size, &mut metrics),
        "instances" => instances(count, &mut metrics),
        _ => Err(io::Error::other("mode must be soak, cycles or instances").into()),
    }
}
