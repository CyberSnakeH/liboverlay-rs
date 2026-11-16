use super::*;
use std::os::fd::AsFd;

#[test]
fn wake_remains_valid_after_the_worker_releases_its_descriptor() {
    let (read, write) = wake_pair().unwrap();
    drop(read);
    // This used to return EPIPE and terminate C++ hosts with default SIGPIPE.
    wake(&write).unwrap();
}

#[test]
fn close_waits_for_thread_local_cleanup() {
    struct Cleanup(Arc<AtomicBool>);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            std::thread::sleep(Duration::from_millis(30));
            self.0.store(true, Ordering::Release);
        }
    }
    std::thread_local! { static CLEANUP: std::cell::RefCell<Option<Cleanup>> = const { std::cell::RefCell::new(None) }; }
    let (window, _rx, _read) = mock_window();
    let cleaned = Arc::new(AtomicBool::new(false));
    let marker = cleaned.clone();
    let shared = window.inner.shared.clone();
    let worker = std::thread::spawn(move || {
        CLEANUP.with(|slot| {
            *slot.borrow_mut() = Some(Cleanup(marker));
        });
        while shared.running.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
    });
    *window.inner.worker.lock().unwrap() = Some(worker);
    window.close().unwrap();
    assert!(cleaned.load(Ordering::Acquire));
    assert!(!window.is_running());
}

fn mock_window() -> (OverlayWindow, Receiver<Command>, OwnedFd) {
    let size = crate::Size::new(4, 4);
    let shared = Arc::new(Shared {
        running: AtomicBool::new(true),
        state: Mutex::new(State {
            #[cfg(feature = "gpu")]
            gpu: None,
            info: WindowInfo {
                logical_size: size,
                physical_size: size,
                scale_factor: crate::ScaleFactor::ONE,
                generation: 0,
            },
            events: VecDeque::new(),
            terminal: None,
            frames: Vec::new(),
            input_reset: false,
        }),
    });
    let (read, write) = wake_pair().unwrap();
    let (tx, rx) = bounded(8);
    let report = BackendReport {
        gpu: None,
        backend: BackendType::Wayland,
        capabilities: crate::Capabilities {
            renderer: crate::RendererType::Cpu,
            transparency: crate::Support::Supported,
            click_through: crate::Support::Supported,
            keyboard_on_demand: crate::Support::Unsupported,
            input_regions: crate::Support::Supported,
            keyboard_translation: crate::Support::Unsupported,
            text_composition: crate::Support::Unsupported,
            input_method: crate::Support::Unsupported,
            surrounding_text: crate::Support::Unsupported,
            touch: crate::Support::Unsupported,
            stacking: crate::StackingSupport::LayerShellOverlay,
            frame_pacing: crate::FramePacing::CompositorCallback,
            presentation_feedback: crate::Support::Unsupported,
            integer_scaling: true,
            fractional_scaling: false,
        },
        fallback_attempts: Vec::new(),
        warnings: Vec::new(),
    };
    let window = OverlayWindow {
        inner: Arc::new(Inner {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            worker: Mutex::new(None),
            tx,
            wake: write,
            shared,
            report,
            timeout: Duration::from_secs(1),
            limit: 1024,
            keyboard_enabled: false,
        }),
    };
    (window, rx, read)
}

#[test]
fn stale_foreign_and_excess_frames_are_rejected() {
    let (window, _rx, _read) = mock_window();
    let frame = window.frame().unwrap();
    window.inner.shared.state.lock().unwrap().info.generation += 1;
    assert!(matches!(
        window.present(frame),
        Err(OverlayError::OutdatedFrame { .. })
    ));
    let (other, _other_rx, _other_read) = mock_window();
    assert!(matches!(
        window.present(other.frame().unwrap()),
        Err(OverlayError::ForeignFrame)
    ));
    for _ in 0..8 {
        window.draw(|_, _, _| {}).unwrap();
    }
    assert!(matches!(
        window.draw(|_, _, _| {}),
        Err(OverlayError::QueueFull)
    ));
}

#[test]
fn recycled_frames_reuse_storage_and_start_transparent() {
    let (window, _rx, _read) = mock_window();
    let mut frame = window.frame().unwrap();
    frame.pixels.fill(0xff12_3456);
    let storage = frame.pixels.as_ptr();
    window.inner.shared.recycle(FrameJob::Cpu(frame)).unwrap();
    window.inner.shared.state.lock().unwrap().info.generation += 1;
    let frame = window.frame().unwrap();
    assert_eq!(frame.pixels.as_ptr(), storage);
    assert!(frame.pixels.iter().all(|pixel| *pixel == 0));
    assert_eq!(frame.generation, window.info().unwrap().generation);
    window.present(frame).unwrap();
}

#[test]
fn concurrent_close_bypasses_a_full_drawing_queue() {
    let (window, _rx, _read) = mock_window();
    for _ in 0..8 {
        window.draw(|_, _, _| {}).unwrap();
    }
    let cleaned = Arc::new(AtomicBool::new(false));
    let marker = cleaned.clone();
    let shared = window.inner.shared.clone();
    *window.inner.worker.lock().unwrap() = Some(std::thread::spawn(move || {
        while shared.running.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        std::thread::sleep(Duration::from_millis(20));
        marker.store(true, Ordering::Release);
    }));
    let barrier = Arc::new(std::sync::Barrier::new(9));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let handle = window.clone();
            let barrier = barrier.clone();
            let cleaned = cleaned.clone();
            scope.spawn(move || {
                barrier.wait();
                handle.close().unwrap();
                assert!(cleaned.load(Ordering::Acquire));
                assert!(matches!(handle.frame(), Err(OverlayError::Closed)));
            });
        }
        barrier.wait();
    });
    window.close().unwrap();
}

#[test]
fn concurrent_drawing_input_and_closure_finish_without_deadlock() {
    struct ReadyBackend {
        socket: std::os::unix::net::UnixStream,
        info: WindowInfo,
    }
    impl OverlayBackend for ReadyBackend {
        fn fd(&self) -> std::os::fd::BorrowedFd<'_> {
            self.socket.as_fd()
        }
        fn info(&self) -> WindowInfo {
            self.info
        }
        fn dispatch(&mut self) -> Result<Vec<OverlayEvent>> {
            Ok(Vec::new())
        }
        fn present(&mut self, _: &Frame) -> Result<bool> {
            Ok(true)
        }
        fn set_click_through(&mut self, _: bool) -> Result<()> {
            Ok(())
        }
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
    }
    let (window, rx, read) = mock_window();
    let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut backend = ReadyBackend {
        socket,
        info: window.info().unwrap(),
    };
    let shared = window.inner.shared.clone();
    *window.inner.worker.lock().unwrap() = Some(std::thread::spawn(move || {
        run(&mut backend, rx, read, &shared).unwrap();
    }));
    let barrier = Arc::new(std::sync::Barrier::new(8));
    std::thread::scope(|scope| {
        for index in 0..8 {
            let window = window.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                if index >= 6 {
                    std::thread::sleep(Duration::from_millis(5));
                    window.close().unwrap();
                } else {
                    for _ in 0..1000 {
                        let result = if index < 4 {
                            window.draw(|pixels, _, _| pixels.fill(0))
                        } else {
                            window.set_click_through(index == 4)
                        };
                        match result {
                            Ok(()) | Err(OverlayError::QueueFull) => {}
                            Err(OverlayError::Closed) => break,
                            Err(error) => {
                                panic!("unexpected concurrent command error: {error}")
                            }
                        }
                    }
                }
            });
        }
    });
    assert!(!window.is_running());
    window.close().unwrap();
}

#[test]
fn input_commands_wake_stalled_rendering_and_dispatch_buffered_events() {
    struct BlockedBackend {
        socket: std::os::unix::net::UnixStream,
        info: WindowInfo,
        input: Arc<AtomicBool>,
        attempted: Sender<()>,
        submitted: Sender<()>,
        buffered_event: bool,
        ready: bool,
    }
    impl OverlayBackend for BlockedBackend {
        fn fd(&self) -> std::os::fd::BorrowedFd<'_> {
            self.socket.as_fd()
        }
        fn info(&self) -> WindowInfo {
            self.info
        }
        fn dispatch(&mut self) -> Result<Vec<OverlayEvent>> {
            if self.buffered_event {
                self.buffered_event = false;
                self.ready = true;
            }
            Ok(Vec::new())
        }
        fn present(&mut self, _: &Frame) -> Result<bool> {
            let _ = self.attempted.try_send(());
            if self.ready {
                let _ = self.submitted.try_send(());
            }
            Ok(self.ready)
        }
        fn set_click_through(&mut self, enabled: bool) -> Result<()> {
            self.input.store(enabled, Ordering::Release);
            self.buffered_event = true;
            Ok(())
        }
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
    }
    let (window, rx, read) = mock_window();
    let shared = window.inner.shared.clone();
    let input = Arc::new(AtomicBool::new(true));
    let worker_input = input.clone();
    let (attempted_tx, attempted_rx) = bounded(1);
    let (submitted_tx, submitted_rx) = bounded(1);
    let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let info = window.info().unwrap();
    let worker = std::thread::spawn(move || {
        let mut backend = BlockedBackend {
            socket,
            info,
            input: worker_input,
            attempted: attempted_tx,
            submitted: submitted_tx,
            buffered_event: false,
            ready: false,
        };
        run(&mut backend, rx, read, &shared)
    });
    window.draw(|_, _, _| {}).unwrap();
    attempted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    window.set_click_through(false).unwrap();
    assert!(!input.load(Ordering::Acquire));
    submitted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    window.close().unwrap();
    worker.join().unwrap().unwrap();
}

#[test]
fn public_handle_is_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<OverlayWindow>();
    check::<WeakOverlayWindow>();
}

#[cfg(feature = "gpu")]
#[test]
fn weak_callback_references_allow_raii_cleanup_while_pacing_is_stalled() {
    struct PendingBackend {
        socket: std::os::unix::net::UnixStream,
        info: WindowInfo,
        attempted: Sender<()>,
    }
    impl OverlayBackend for PendingBackend {
        fn fd(&self) -> std::os::fd::BorrowedFd<'_> {
            self.socket.as_fd()
        }
        fn info(&self) -> WindowInfo {
            self.info
        }
        fn dispatch(&mut self) -> Result<Vec<OverlayEvent>> {
            Ok(Vec::new())
        }
        fn present(&mut self, _: &Frame) -> Result<bool> {
            unreachable!()
        }
        fn present_gpu(
            &mut self,
            _: &mut Option<crate::gpu::GpuDraw>,
            _: WindowInfo,
        ) -> Result<bool> {
            let _ = self.attempted.try_send(());
            Ok(false)
        }
        fn set_click_through(&mut self, _: bool) -> Result<()> {
            Ok(())
        }
        fn flush(&mut self) -> Result<()> {
            Ok(())
        }
    }
    struct CallbackDrop(Arc<AtomicBool>);
    impl Drop for CallbackDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let (mut window, rx, read) = mock_window();
    Arc::get_mut(&mut window.inner)
        .unwrap()
        .report
        .capabilities
        .renderer = crate::RendererType::Gpu;
    let weak = window.downgrade();
    let reference = weak.clone();
    let dropped = Arc::new(AtomicBool::new(false));
    let marker = CallbackDrop(dropped.clone());
    let (attempted_tx, attempted_rx) = bounded(1);
    let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let mut backend = PendingBackend {
        socket,
        info: window.info().unwrap(),
        attempted: attempted_tx,
    };
    let shared = window.inner.shared.clone();
    *window.inner.worker.lock().unwrap() = Some(std::thread::spawn(move || {
        run(&mut backend, rx, read, &shared).unwrap();
    }));
    window
        .draw_gpu(move |_| {
            let _marker = marker;
            let _handle = reference.upgrade();
            panic!("stalled callback must not execute");
        })
        .unwrap();
    attempted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    drop(window);
    assert!(
        dropped.load(Ordering::Acquire),
        "pending callback survived final handle drop"
    );
    assert!(weak.upgrade().is_none());
}
#[test]
fn event_queue_bounds_motion_and_preserves_terminal_event() {
    let size = crate::Size::new(1, 1);
    let shared = Shared {
        running: AtomicBool::new(true),
        state: Mutex::new(State {
            #[cfg(feature = "gpu")]
            gpu: None,
            info: WindowInfo {
                logical_size: size,
                physical_size: size,
                scale_factor: crate::ScaleFactor::ONE,
                generation: 0,
            },
            events: VecDeque::new(),
            terminal: None,
            frames: Vec::new(),
            input_reset: false,
        }),
    };
    for n in 0..1000 {
        shared
            .push(OverlayEvent::PointerMoved {
                x: n as f64,
                y: 0.0,
            })
            .unwrap();
    }
    assert_eq!(shared.state.lock().unwrap().events.len(), 1);
    for _ in 0..1000 {
        shared.push(OverlayEvent::FrameDiscarded).unwrap();
    }
    shared.push(OverlayEvent::Closed).unwrap();
    let state = shared.state.lock().unwrap();
    assert!(state.events.len() <= EVENT_CAPACITY);
    assert!(state.events.iter().any(|event| matches!(
        event,
        OverlayEvent::InputReset {
            scope: crate::InputResetScope::All,
            reason: crate::InputResetReason::QueueOverflow,
            ..
        }
    )));
    assert!(state.input_reset);
    assert!(matches!(state.events.back(), Some(OverlayEvent::Closed)));
}
