//! Capture only the private Sway output created by the validation runner.
#![cfg(feature = "wayland")]
use liboverlay::{
    Anchor, BackendPreference, OutputSelector, OverlayBuilder, OverlayEvent, Support,
};
use std::{
    fs::File,
    os::fd::AsFd,
    time::{Duration, Instant},
};
use wayland_client::{
    delegate_noop,
    protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1 as frame, zwlr_screencopy_manager_v1 as manager,
};
type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Default)]
struct Capture {
    shm: Option<wl_shm::WlShm>,
    manager: Option<manager::ZwlrScreencopyManagerV1>,
    outputs: Vec<(wl_output::WlOutput, String)>,
    file: Option<File>,
    buffer: Option<wl_buffer::WlBuffer>,
    width: u32,
    height: u32,
    stride: u32,
    format: Option<wl_shm::Format>,
    ready: bool,
    failed: bool,
    inverted: bool,
}
impl Dispatch<wl_registry::WlRegistry, ()> for Capture {
    fn event(
        s: &mut Self,
        r: &wl_registry::WlRegistry,
        e: wl_registry::Event,
        _: &(),
        _: &Connection,
        q: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = e
        {
            match interface.as_str() {
                "wl_shm" => s.shm = Some(r.bind(name, 1, q, ())),
                "zwlr_screencopy_manager_v1" => s.manager = Some(r.bind(name, 1, q, ())),
                "wl_output" => s
                    .outputs
                    .push((r.bind(name, version.min(4), q, ()), String::new())),
                _ => {}
            }
        }
    }
}
impl Dispatch<wl_output::WlOutput, ()> for Capture {
    fn event(
        s: &mut Self,
        p: &wl_output::WlOutput,
        e: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = e {
            if let Some(v) = s.outputs.iter_mut().find(|v| &v.0 == p) {
                v.1 = name;
            }
        }
    }
}
impl Dispatch<frame::ZwlrScreencopyFrameV1, ()> for Capture {
    fn event(
        s: &mut Self,
        p: &frame::ZwlrScreencopyFrameV1,
        e: frame::Event,
        _: &(),
        _: &Connection,
        q: &QueueHandle<Self>,
    ) {
        match e {
            frame::Event::Buffer {
                format: wayland_client::WEnum::Value(format),
                width,
                height,
                stride,
            } => {
                if !matches!(format, wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888)
                    || stride < width * 4
                    || u64::from(stride) * u64::from(height) > 64 * 1024 * 1024
                {
                    s.failed = true;
                    return;
                }
                let file = File::from(
                    rustix::fs::memfd_create(c"overlay-capture", rustix::fs::MemfdFlags::CLOEXEC)
                        .unwrap(),
                );
                let bytes = stride * height;
                file.set_len(u64::from(bytes)).unwrap();
                let pool = s
                    .shm
                    .as_ref()
                    .unwrap()
                    .create_pool(file.as_fd(), bytes as i32, q, ());
                let buffer = pool.create_buffer(
                    0,
                    width as i32,
                    height as i32,
                    stride as i32,
                    format,
                    q,
                    (),
                );
                pool.destroy();
                p.copy(&buffer);
                s.file = Some(file);
                s.buffer = Some(buffer);
                s.width = width;
                s.height = height;
                s.stride = stride;
                s.format = Some(format);
            }
            frame::Event::Flags {
                flags: wayland_client::WEnum::Value(flags),
            } => s.inverted = flags.contains(frame::Flags::YInvert),
            frame::Event::Ready { .. } => s.ready = true,
            frame::Event::Failed => s.failed = true,
            _ => {}
        }
    }
}
delegate_noop!(Capture: ignore wl_shm::WlShm);
delegate_noop!(Capture: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Capture: ignore wl_buffer::WlBuffer);
delegate_noop!(Capture: ignore manager::ZwlrScreencopyManagerV1);

#[test]
#[ignore = "captures only the runner-owned private output"]
fn partial_updates_and_presentation_preserve_composed_pixels() -> Result {
    if std::env::var_os("LIBOVERLAY_TEST_PRIVATE_CAPTURE").is_none() {
        return Err("private capture runner required".into());
    }
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue();
    let mut capture = Capture::default();
    connection.display().get_registry(&queue.handle(), ());
    queue.roundtrip(&mut capture)?;
    queue.roundtrip(&mut capture)?;
    let output = capture
        .outputs
        .iter()
        .find(|v| v.1 == "HEADLESS-1")
        .ok_or("private output not found")?
        .0
        .clone();
    let overlay = OverlayBuilder::new()
        .with_backend(BackendPreference::WaylandOnly)
        .with_output(OutputSelector::Name("HEADLESS-1".into()))
        .with_anchor(Anchor::Fill)
        .with_cpu_damage_tracking(true)
        .with_presentation_feedback(true)
        .build()?;
    assert_eq!(
        overlay.backend_report().capabilities.presentation_feedback,
        Support::Supported
    );
    for index in 0..8 {
        if matches!(index, 3 | 5 | 7) {
            let generation = overlay.info()?.generation;
            println!("READY pixels-{index}");
            use std::io::Write as _;
            std::io::stdout().flush()?;
            let mut reply = String::new();
            std::io::stdin().read_line(&mut reply)?;
            assert_eq!(reply.trim(), "changed");
            let deadline = Instant::now() + Duration::from_secs(3);
            while overlay.info()?.generation == generation {
                assert!(Instant::now() < deadline, "private output did not resize");
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        let info = overlay.info()?;
        let width = info.physical_size.width;
        let height = info.physical_size.height;
        let mut expected = vec![0xff000000; (width * height) as usize];
        let x = if index < 2 { 5 } else { (index * 41) % width };
        for y in 4..18 {
            expected[(y * width + x) as usize] = 0xff1256ab;
        }
        if index == 4 {
            expected[(width * height - 1) as usize] = 0xffcd8721;
        }
        overlay.draw(|p, _, _| p.copy_from_slice(&expected))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let events = overlay.poll_events()?;
            if events.iter().any(|e| {
                matches!(
                    e,
                    OverlayEvent::FramePresented(_) | OverlayEvent::PresentationDiscarded { .. }
                )
            }) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "presentation feedback did not arrive"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        capture.ready = false;
        capture.failed = false;
        let frame =
            capture
                .manager
                .as_ref()
                .unwrap()
                .capture_output(0, &output, &queue.handle(), ());
        while !capture.ready && !capture.failed {
            queue.roundtrip(&mut capture)?;
            assert!(Instant::now() < deadline);
        }
        assert!(!capture.failed, "private screencopy failed");
        assert_eq!((capture.width, capture.height), (width, height));
        use std::os::unix::fs::FileExt;
        let mut bytes = vec![0; (capture.stride * capture.height) as usize];
        capture
            .file
            .as_ref()
            .unwrap()
            .read_exact_at(&mut bytes, 0)?;
        for y in 0..height {
            let source_y = if capture.inverted { height - y - 1 } else { y };
            for x in 0..width {
                let offset = (source_y * capture.stride + x * 4) as usize;
                let actual = u32::from_ne_bytes(bytes[offset..offset + 4].try_into()?);
                assert_eq!(
                    actual & 0xffffff,
                    expected[(y * width + x) as usize] & 0xffffff,
                    "composed pixels differ at ({x},{y}), frame {index}"
                );
            }
        }
        frame.destroy();
        capture.buffer.take().unwrap().destroy();
        capture.file = None;
    }
    overlay.close()?;
    Ok(())
}
