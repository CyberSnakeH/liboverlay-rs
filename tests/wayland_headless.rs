//! Private-compositor checks driven by scripts/check_wayland.py.
#![cfg(feature = "wayland")]
use liboverlay::{
    Anchor, BackendPreference, OutputSelector, OverlayBuilder, OverlayError, OverlayEvent,
    OverlayWindow, ScaleFactor,
};
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

fn builder() -> OverlayBuilder {
    OverlayBuilder::new()
        .with_backend(BackendPreference::WaylandOnly)
        .with_output(OutputSelector::Name("HEADLESS-2".into()))
        .with_size(102, 101)
}

fn signal(stage: &str) -> Result<String, Box<dyn std::error::Error>> {
    println!("READY {stage}");
    io::stdout().flush()?;
    let mut reply = String::new();
    io::stdin().read_line(&mut reply)?;
    if reply.is_empty() {
        return Err("runner disconnected".into());
    }
    Ok(reply.trim().into())
}

#[test]
#[ignore = "controlled touch messages on the runner's private Wayland proxy"]
fn touch_contacts_seat_removal_and_stalled_input_regions() -> Result<(), Box<dyn std::error::Error>>
{
    use liboverlay::{InputRectangle, InputRegion, TouchPhase};
    fn until(
        window: &OverlayWindow,
        events: &mut Vec<OverlayEvent>,
        predicate: impl Fn(&[OverlayEvent]) -> bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate(events) {
            events.extend(window.poll_events()?);
            if Instant::now() >= deadline {
                return Err(format!("touch fixture timed out: {events:?}").into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
    let region = InputRegion::Rectangles(vec![InputRectangle::new(0, 0, 40, 40)]);
    let overlay = builder().with_input_region(region.clone()).build()?;
    assert_eq!(
        overlay.backend_report().capabilities.touch,
        liboverlay::Support::Supported
    );
    overlay.set_click_through(true)?;
    overlay.set_click_through(false)?;
    overlay.set_input_region(region)?;
    let mut events = Vec::new();
    signal("touch-start")?;
    until(&overlay, &mut events, |events| {
        events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    OverlayEvent::Touch {
                        phase: TouchPhase::Started,
                        ..
                    }
                )
            })
            .count()
            == 2
    })?;
    let sources: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let OverlayEvent::Touch {
                source,
                id: 7,
                phase: TouchPhase::Started,
                x,
                y,
            } = e
            {
                assert_eq!((*x, *y), (12.5, 20.25));
                Some(*source)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(sources.len(), 2);
    assert_ne!(sources[0], sources[1]);
    let frame_count = |events: &[OverlayEvent], source| {
        events
            .iter()
            .filter(|e| matches!(e,OverlayEvent::TouchFrame {source:found} if *found==source))
            .count()
    };
    until(&overlay, &mut events, |e| {
        frame_count(e, sources[0]) >= 1 && frame_count(e, sources[1]) >= 1
    })?;
    signal("touch-move")?;
    until(&overlay, &mut events, |e| frame_count(e, sources[0]) >= 2)?;
    assert!(events.iter().any(|e|matches!(e,OverlayEvent::Touch {source,phase:TouchPhase::Moved,x,y,..} if *source==sources[0] && *x== -3.5 && *y==22.25)));
    signal("touch-cancel")?;
    until(&overlay, &mut events, |e| frame_count(e, sources[0]) >= 3)?;
    assert!(events.iter().any(|e|matches!(e,OverlayEvent::Touch {source,phase:TouchPhase::Cancelled,..} if *source==sources[0])));
    assert!(!events.iter().any(|e|matches!(e,OverlayEvent::Touch {source,phase:TouchPhase::Cancelled,..} if *source==sources[1])));
    signal("touch-orphan")?;
    until(&overlay, &mut events, |e| frame_count(e, sources[0]) >= 4)?;
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(
                e,
                OverlayEvent::Touch {
                    phase: TouchPhase::Moved,
                    ..
                }
            ))
            .count(),
        1
    );
    signal("touch-remove")?;
    until(&overlay, &mut events, |events| {
        events.iter().any(|e|matches!(e,OverlayEvent::Touch {source,phase:TouchPhase::Cancelled,..} if *source==sources[1]))
    })?;
    assert!(overlay.is_running());
    overlay.close()?;
    Ok(())
}

fn wait_for_close(window: &OverlayWindow) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while window.is_running() {
        assert!(Instant::now() < deadline, "runtime did not close");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
#[ignore = "requires the private Sway/protocol-filter runner"]
fn protocol_negotiation_and_frames() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os("LIBOVERLAY_TEST_NO_LAYER_SHELL").is_some() {
        let error = builder().build().unwrap_err();
        assert!(matches!(error, OverlayError::NoBackend { .. }));
        assert!(error.to_string().contains("zwlr_layer_shell_v1"));
        return Ok(());
    }
    let overlay = builder().with_presentation_feedback(true).build()?;
    assert_eq!(
        overlay.backend_report().capabilities.presentation_feedback,
        if std::env::var_os("LIBOVERLAY_TEST_NO_PRESENTATION").is_some() {
            liboverlay::Support::Unsupported
        } else {
            liboverlay::Support::Supported
        }
    );
    let fractional = std::env::var_os("LIBOVERLAY_TEST_NO_FRACTIONAL").is_none();
    assert_eq!(
        overlay.backend_report().capabilities.fractional_scaling,
        fractional
    );
    let info = overlay.info()?;
    let scale = if fractional {
        ScaleFactor::from_120ths(150)?
    } else {
        ScaleFactor::from_integer(2)?
    };
    assert_eq!(info.scale_factor, scale);
    assert_eq!(info.physical_size, scale.physical_size(info.logical_size)?);
    for _ in 0..3 {
        overlay.draw(|pixels, _, _| pixels.fill(0))?;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if overlay
                .poll_events()?
                .iter()
                .any(|event| matches!(event, OverlayEvent::FrameSubmitted { .. }))
            {
                break;
            }
            assert!(Instant::now() < deadline, "CPU rendering stopped");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    overlay.close()?;
    Ok(())
}

#[test]
#[ignore = "runner withholds frame callbacks on a private Wayland socket"]
fn withheld_callbacks_do_not_block_input_or_close() -> Result<(), Box<dyn std::error::Error>> {
    let overlay = builder().build()?;
    let mut stalled = false;
    // Initial configure/scale events may legitimately unthrottle obsolete
    // geometry once. Wait for that negotiation before testing withheld pacing.
    for _ in 0..10 {
        overlay.poll_events()?;
        overlay.draw(|pixels, _, _| pixels.fill(0))?;
        std::thread::sleep(Duration::from_millis(50));
        if !overlay
            .poll_events()?
            .iter()
            .any(|event| matches!(event, OverlayEvent::FrameSubmitted { .. }))
        {
            stalled = true;
            break;
        }
    }
    assert!(stalled, "fixture did not withhold render pacing");
    overlay.set_click_through(false)?;
    overlay.set_click_through(true)?;
    overlay.close()?;
    assert!(!overlay.is_running());
    Ok(())
}

#[cfg(feature = "gpu")]
#[test]
#[ignore = "runner withholds callbacks and selects software Vulkan on a private socket"]
fn device_loss_while_waiting_for_a_callback_is_terminal() -> Result<(), Box<dyn std::error::Error>>
{
    let overlay = builder()
        .with_renderer(liboverlay::RendererPreference::GpuRequired)
        .with_software_gpu(true)
        .build()?;
    overlay.draw_gpu(|_| Ok(()))?;
    overlay.gpu_context()?.device.destroy();
    wait_for_close(&overlay);
    assert!(matches!(overlay.poll_events(), Err(OverlayError::Gpu(_))));
    overlay.close()?;
    Ok(())
}

#[test]
#[ignore = "runner changes scales, dimensions and removes an owned virtual output"]
fn geometry_and_output_removal() -> Result<(), Box<dyn std::error::Error>> {
    let config = builder().with_anchor(Anchor::Fill).with_margin(8);
    #[cfg(feature = "gpu")]
    let config = if std::env::var_os("LIBOVERLAY_TEST_GPU_GEOMETRY").is_some() {
        config
            .with_renderer(liboverlay::RendererPreference::GpuRequired)
            .with_software_gpu(true)
    } else {
        config
    };
    let overlay = config.build()?;
    let cpu = overlay.backend_report().capabilities.renderer == liboverlay::RendererType::Cpu;
    for stage in 0..3 {
        let before = overlay.info()?;
        let stale = if cpu { Some(overlay.frame()?) } else { None };
        let expected: u32 = signal(&format!("scale-{stage}"))?.parse()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let info = overlay.info()?;
            if info.scale_factor.numerator() == expected && info.generation > before.generation {
                assert_eq!(
                    info.physical_size,
                    info.scale_factor.physical_size(info.logical_size)?
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "latest scale/geometry was not applied"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        if let Some(stale) = stale {
            assert!(matches!(
                overlay.present(stale),
                Err(OverlayError::OutdatedFrame { .. })
            ));
        }
        overlay.poll_events()?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let result = if cpu {
                overlay.draw(|pixels, _, _| pixels.fill(0))
            } else {
                #[cfg(feature = "gpu")]
                {
                    overlay.draw_gpu(|_| Ok(()))
                }
                #[cfg(not(feature = "gpu"))]
                {
                    unreachable!("GPU renderer without GPU support")
                }
            };
            match result {
                Ok(()) | Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. }) => {}
                Err(error) => return Err(error.into()),
            }
            let generation = overlay.info()?.generation;
            if overlay.poll_events()?.iter().any(|event| {
                matches!(event,
                OverlayEvent::FrameSubmitted { generation: submitted } if *submitted == generation)
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "new geometry was not presented");
            std::thread::sleep(Duration::from_millis(2));
        }
        overlay.set_click_through(true)?;
    }
    assert_eq!(signal("remove-output")?, "removed");
    wait_for_close(&overlay);
    assert!(overlay
        .poll_events()?
        .iter()
        .any(|event| matches!(event, OverlayEvent::Closed)));
    overlay.close()?;
    Ok(())
}

#[test]
#[ignore = "runner terminates its own Sway after READY"]
fn server_disconnect_is_terminal() -> Result<(), Box<dyn std::error::Error>> {
    let overlay = builder().build()?;
    overlay.draw(|pixels, _, _| pixels.fill(0))?;
    assert_eq!(signal("disconnect")?, "disconnected");
    wait_for_close(&overlay);
    assert!(matches!(
        overlay.poll_events(),
        Err(OverlayError::Backend { .. }) | Err(OverlayError::Io(_))
    ));
    overlay.close()?;
    assert!(matches!(overlay.frame(), Err(OverlayError::Closed)));
    Ok(())
}

#[cfg(feature = "gpu")]
#[test]
#[ignore = "runner selects missing or software-only Vulkan drivers"]
fn gpu_adapter_policies() -> Result<(), Box<dyn std::error::Error>> {
    use liboverlay::{RendererPreference, RendererType};
    let software = std::env::var("LIBOVERLAY_TEST_VULKAN")? == "software";
    let error = builder()
        .with_renderer(RendererPreference::GpuRequired)
        .build()
        .unwrap_err();
    assert!(matches!(error, OverlayError::NoBackend { .. }));
    if software {
        assert!(error.to_string().contains("software"), "{error}");
        let overlay = builder()
            .with_renderer(RendererPreference::GpuRequired)
            .with_software_gpu(true)
            .build()?;
        assert_eq!(
            overlay.backend_report().capabilities.renderer,
            RendererType::Gpu
        );
        assert!(overlay.backend_report().gpu.as_ref().unwrap().software);
        let limit = overlay
            .gpu_context()?
            .device
            .limits()
            .max_texture_dimension_2d;
        // The selected headless output requests 125% scaling. Exercise the
        // checked configuration failure while remaining within CPU protocol bounds.
        if limit < 32767 {
            let width = limit * 120 / 150 + 2;
            let error = builder()
                .with_size(width, 16)
                .with_renderer(RendererPreference::GpuRequired)
                .with_software_gpu(true)
                .build()
                .unwrap_err();
            assert!(
                error.to_string().contains("GPU texture dimension limit"),
                "{error}"
            );
            let fallback = builder()
                .with_size(width, 16)
                .with_renderer(RendererPreference::GpuPreferred)
                .with_software_gpu(true)
                .build()?;
            assert_eq!(
                fallback.backend_report().capabilities.renderer,
                RendererType::Cpu
            );
            fallback.close()?;
        }
        overlay.draw_gpu(|_| Ok(()))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if overlay
                .poll_events()?
                .iter()
                .any(|event| matches!(event, OverlayEvent::FrameSubmitted { .. }))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "software Vulkan did not submit a frame"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        overlay.close()?;
    } else {
        let overlay = builder()
            .with_renderer(RendererPreference::GpuPreferred)
            .build()?;
        assert_eq!(
            overlay.backend_report().capabilities.renderer,
            RendererType::Cpu
        );
        assert!(overlay.backend_report().gpu.is_none());
        assert!(overlay
            .backend_report()
            .warnings
            .iter()
            .any(|warning| warning.contains("GPU initialization failed")));
        overlay.draw(|pixels, _, _| pixels.fill(0))?;
        overlay.close()?;
    }
    Ok(())
}
