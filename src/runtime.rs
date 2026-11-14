use crate::{
    backend::{self, OverlayBackend},
    BackendReport, BackendType, Frame, OverlayBuilder, OverlayError, OverlayEvent, Result,
    WindowInfo,
};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError, TrySendError};
use std::{
    collections::VecDeque,
    os::fd::OwnedFd,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

const EVENT_CAPACITY: usize = 256;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct State {
    #[cfg(feature = "gpu")]
    gpu: Option<crate::gpu::GpuContext>,
    info: WindowInfo,
    events: VecDeque<OverlayEvent>,
    terminal: Option<OverlayError>,
}
struct Shared {
    running: AtomicBool,
    state: Mutex<State>,
}
impl Shared {
    fn push(&self, event: OverlayEvent) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| OverlayError::Poisoned)?;
        if matches!(event, OverlayEvent::PointerMoved { .. })
            && matches!(state.events.back(), Some(OverlayEvent::PointerMoved { .. }))
        {
            state.events.pop_back();
        }
        if state.events.len() == EVENT_CAPACITY {
            state.events.pop_front();
        }
        state.events.push_back(event);
        Ok(())
    }
}

enum Command {
    Present(Frame),
    #[cfg(feature = "gpu")]
    Gpu(WindowInfo, crate::gpu::GpuDraw),
    ClickThrough(bool, Sender<Result<()>>),
    Close,
}
enum FrameJob {
    Cpu(Frame),
    #[cfg(feature = "gpu")]
    Gpu {
        info: WindowInfo,
        draw: Option<crate::gpu::GpuDraw>,
    },
}
impl FrameJob {
    fn geometry(&self) -> (crate::Size, u64) {
        match self {
            Self::Cpu(frame) => (frame.size, frame.generation),
            #[cfg(feature = "gpu")]
            Self::Gpu { info, .. } => (info.physical_size, info.generation),
        }
    }
    fn present(&mut self, backend: &mut dyn OverlayBackend) -> Result<bool> {
        match self {
            Self::Cpu(frame) => backend.present(frame),
            #[cfg(feature = "gpu")]
            Self::Gpu { info, draw } => backend.present_gpu(draw, *info),
        }
    }
}
struct Inner {
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    id: u64,
    tx: Sender<Command>,
    wake: OwnedFd,
    shared: Arc<Shared>,
    report: BackendReport,
    timeout: Duration,
    limit: usize,
}

/// Cloneable, `Send + Sync` handle to an overlay's private display runtime.
#[derive(Clone)]
pub struct OverlayWindow {
    inner: Arc<Inner>,
}
impl std::fmt::Debug for OverlayWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OverlayWindow")
            .field("id", &self.inner.id)
            .field("backend", &self.backend_type())
            .field("running", &self.is_running())
            .finish()
    }
}
impl OverlayWindow {
    pub fn backend_type(&self) -> BackendType {
        self.inner.report.backend
    }
    pub fn backend_report(&self) -> &BackendReport {
        &self.inner.report
    }
    pub fn is_running(&self) -> bool {
        self.inner.shared.running.load(Ordering::Acquire)
    }
    pub fn info(&self) -> Result<WindowInfo> {
        Ok(self
            .inner
            .shared
            .state
            .lock()
            .map_err(|_| OverlayError::Poisoned)?
            .info)
    }
    /// Allocates a transparent frame at the current physical size.
    pub fn frame(&self) -> Result<Frame> {
        if !self.is_running() {
            return Err(OverlayError::Closed);
        }
        if self.inner.report.capabilities.renderer != crate::RendererType::Cpu {
            return Err(OverlayError::Unsupported(
                "CPU frames on a GPU overlay; use draw_gpu",
            ));
        }
        let info = self.info()?;
        Frame::new(
            info.physical_size,
            info.generation,
            self.inner.id,
            self.inner.limit,
        )
    }
    /// Queues a frame without blocking. Newer frames may replace pending frames.
    /// A successful return means queued, not displayed. Resize races produce
    /// `FrameDiscarded`; the application should redraw using the latest `info()`.
    pub fn present(&self, frame: Frame) -> Result<()> {
        if frame.owner != self.inner.id {
            return Err(OverlayError::ForeignFrame);
        }
        let info = self.info()?;
        if frame.size != info.physical_size || frame.generation != info.generation {
            return Err(OverlayError::OutdatedFrame {
                expected: info.physical_size,
                actual: frame.size,
            });
        }
        self.send(Command::Present(frame))
    }
    pub fn draw(&self, draw: impl FnOnce(&mut [u32], u32, u32)) -> Result<()> {
        let mut frame = self.frame()?;
        let size = frame.size;
        draw(&mut frame.pixels, size.width, size.height);
        self.present(frame)
    }
    #[cfg(feature = "gpu")]
    pub fn gpu_context(&self) -> Result<crate::gpu::GpuContext> {
        if !self.is_running() {
            return Err(OverlayError::Closed);
        }
        self.inner
            .shared
            .state
            .lock()
            .map_err(|_| OverlayError::Poisoned)?
            .gpu
            .clone()
            .ok_or(OverlayError::Unsupported("GPU context on a CPU overlay"))
    }
    /// Queues a GPU encoding callback. It runs on the display thread; prepare
    /// pipelines and resources beforehand, and avoid blocking inside callbacks.
    #[cfg(feature = "gpu")]
    pub fn draw_gpu(
        &self,
        draw: impl for<'a> FnOnce(crate::gpu::GpuFrame<'a>) -> Result<()> + Send + 'static,
    ) -> Result<()> {
        if self.inner.report.capabilities.renderer != crate::RendererType::Gpu {
            return Err(OverlayError::Unsupported("GPU commands on a CPU overlay"));
        }
        self.send(Command::Gpu(self.info()?, Box::new(draw)))
    }
    /// Applies the input region independently of render pacing. The reply
    /// confirms a flushed request, not that the next physical click occurred.
    pub fn set_click_through(&self, enabled: bool) -> Result<()> {
        if on_worker_thread(&self.inner.worker)? {
            return Err(OverlayError::Unsupported(
                "synchronous input commands from a GPU callback",
            ));
        }
        let (tx, rx) = bounded(1);
        self.send(Command::ClickThrough(enabled, tx))?;
        rx.recv_timeout(self.inner.timeout)
            .map_err(|error| match error {
                crossbeam_channel::RecvTimeoutError::Timeout => {
                    OverlayError::Timeout("setting the input region")
                }
                crossbeam_channel::RecvTimeoutError::Disconnected => OverlayError::Closed,
            })?
    }
    pub fn poll_events(&self) -> Result<Vec<OverlayEvent>> {
        let mut state = self
            .inner
            .shared
            .state
            .lock()
            .map_err(|_| OverlayError::Poisoned)?;
        if let Some(error) = state.terminal.take() {
            return Err(error);
        }
        Ok(state.events.drain(..).collect())
    }
    pub fn close(&self) -> Result<()> {
        if on_worker_thread(&self.inner.worker)? {
            return Err(OverlayError::Unsupported(
                "synchronous close from a GPU callback",
            ));
        }
        if self.is_running() {
            match self.send(Command::Close) {
                Ok(()) | Err(OverlayError::Closed) => {}
                Err(error) => return Err(error),
            }
        }
        join_worker(&self.inner.worker, self.inner.timeout)
    }
    fn send(&self, command: Command) -> Result<()> {
        if !self.is_running() {
            return Err(OverlayError::Closed);
        }
        self.inner
            .tx
            .try_send(command)
            .map_err(|error| match error {
                TrySendError::Full(_) => OverlayError::QueueFull,
                TrySendError::Disconnected(_) => OverlayError::Closed,
            })?;
        wake(&self.inner.wake)
    }
}
fn wake(fd: &OwnedFd) -> Result<()> {
    match rustix::io::write(fd, &[1]) {
        Ok(_) | Err(rustix::io::Errno::AGAIN) => Ok(()),
        Err(error) => Err(std::io::Error::from(error).into()),
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::Release);
        let _ = wake(&self.wake);
        let _ = join_worker(&self.worker, self.timeout);
    }
}

fn on_worker_thread(worker: &Mutex<Option<std::thread::JoinHandle<()>>>) -> Result<bool> {
    Ok(worker
        .lock()
        .map_err(|_| OverlayError::Poisoned)?
        .as_ref()
        .is_some_and(|handle| handle.thread().id() == std::thread::current().id()))
}

fn join_worker(
    worker: &Mutex<Option<std::thread::JoinHandle<()>>>,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        {
            let mut slot = worker.lock().map_err(|_| OverlayError::Poisoned)?;
            let Some(handle) = slot.as_ref() else {
                return Ok(());
            };
            if handle.thread().id() == std::thread::current().id() {
                return Err(OverlayError::Unsupported(
                    "joining the display thread from itself",
                ));
            }
            if handle.is_finished() {
                // Join also waits for native thread-local destruction, beyond
                // our Rust cleanup guard, before the caller can exit main.
                let finished = slot.take();
                drop(slot);
                if let Some(handle) = finished {
                    handle.join().map_err(|_| OverlayError::RuntimePanic)?;
                }
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(OverlayError::Timeout("waiting for display cleanup"));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub(crate) fn spawn(config: OverlayBuilder) -> Result<OverlayWindow> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let initial = WindowInfo {
        logical_size: config.size,
        physical_size: config.size,
        scale_factor: crate::ScaleFactor::ONE,
        generation: 0,
    };
    let shared = Arc::new(Shared {
        running: AtomicBool::new(true),
        state: Mutex::new(State {
            #[cfg(feature = "gpu")]
            gpu: None,
            info: initial,
            events: VecDeque::new(),
            terminal: None,
        }),
    });
    let (read, write) = rustix::pipe::pipe_with(
        rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
    )
    .map_err(std::io::Error::from)?;
    let (tx, rx) = bounded(8);
    let (ready_tx, ready_rx) = bounded(1);
    let timeout = config.startup_timeout;
    let command_timeout = config.command_timeout;
    let limit = config.max_frame_bytes;
    let worker_shared = shared.clone();
    let worker = std::thread::Builder::new()
        .name(format!("liboverlay-{id}"))
        .spawn(move || {
            // Also marks closure if a dependency unexpectedly unwinds.
            struct RunningGuard(Arc<Shared>);
            impl Drop for RunningGuard {
                fn drop(&mut self) {
                    self.0.running.store(false, Ordering::Release);
                    let _ = self.0.push(OverlayEvent::Closed);
                }
            }
            let _guard = RunningGuard(worker_shared.clone());
            match backend::create(&config, Instant::now() + timeout) {
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                }
                Ok((mut backend, report)) => {
                    if let Ok(mut state) = worker_shared.state.lock() {
                        state.info = backend.info();
                        #[cfg(feature = "gpu")]
                        {
                            state.gpu = backend.gpu_context();
                        }
                    }
                    if ready_tx.send(Ok(report)).is_err() {
                        return;
                    }
                    let result = run(&mut *backend, rx, read, &worker_shared);
                    // Release the runtime's exported device clone before the
                    // backend tears down its native display connection.
                    #[cfg(feature = "gpu")]
                    if let Ok(mut state) = worker_shared.state.lock() {
                        state.gpu = None;
                    }
                    if let Err(error) = result {
                        if let Ok(mut state) = worker_shared.state.lock() {
                            state.terminal = Some(error);
                        }
                    }
                }
            }
        })?;
    let worker = Mutex::new(Some(worker));
    let report = match ready_rx.recv_timeout(timeout) {
        Ok(Ok(report)) => report,
        result => {
            shared.running.store(false, Ordering::Release);
            let _ = wake(&write);
            let _ = join_worker(&worker, command_timeout);
            return Err(match result {
                Ok(Err(error)) => error,
                _ => OverlayError::Timeout("creating the overlay"),
            });
        }
    };
    Ok(OverlayWindow {
        inner: Arc::new(Inner {
            worker,
            id,
            tx,
            wake: write,
            shared,
            report,
            timeout: command_timeout,
            limit,
        }),
    })
}

fn dispatch_events(
    backend: &mut dyn OverlayBackend,
    shared: &Shared,
) -> Result<Option<WindowInfo>> {
    for event in backend.dispatch()? {
        if matches!(event, OverlayEvent::Closed) {
            return Ok(None);
        }
        shared.push(event)?;
    }
    let info = backend.info();
    shared
        .state
        .lock()
        .map_err(|_| OverlayError::Poisoned)?
        .info = info;
    Ok(Some(info))
}

fn run(
    backend: &mut dyn OverlayBackend,
    rx: Receiver<Command>,
    read: OwnedFd,
    shared: &Shared,
) -> Result<()> {
    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    let mut pending: Option<FrameJob> = None;
    let mut submitted = None;
    while shared.running.load(Ordering::Acquire) {
        let Some(mut info) = dispatch_events(backend, shared)? else {
            return Ok(());
        };
        let mut buffered_events_possible = false;
        for _ in 0..64 {
            match rx.try_recv() {
                Ok(Command::Present(frame)) => {
                    if pending.replace(FrameJob::Cpu(frame)).is_some() {
                        shared.push(OverlayEvent::FrameDiscarded)?;
                    }
                }
                #[cfg(feature = "gpu")]
                Ok(Command::Gpu(info, draw)) => {
                    if pending
                        .replace(FrameJob::Gpu {
                            info,
                            draw: Some(draw),
                        })
                        .is_some()
                    {
                        shared.push(OverlayEvent::FrameDiscarded)?;
                    }
                }
                Ok(Command::ClickThrough(enabled, reply)) => {
                    buffered_events_possible = true;
                    let result = backend
                        .set_click_through(enabled)
                        .and_then(|()| backend.flush());
                    let fatal = result
                        .as_ref()
                        .err()
                        .is_some_and(|e| !matches!(e, OverlayError::Unsupported(_)));
                    // Preserve the actual error for the synchronous caller.
                    let _ = reply.send(result);
                    if fatal {
                        return Err(OverlayError::Closed);
                    }
                }
                Ok(Command::Close) => {
                    shared.running.store(false, Ordering::Release);
                    return Ok(());
                }
                Err(TryRecvError::Disconnected) => return Ok(()),
                Err(TryRecvError::Empty) => break,
            }
        }
        // A synchronous X11 reply may have read Present notifications into
        // x11rb's event queue, leaving the socket unreadable. Dispatch that queue
        // before waiting for the next frame, or a single pending frame can stall.
        if buffered_events_possible {
            let Some(latest) = dispatch_events(backend, shared)? else {
                return Ok(());
            };
            info = latest;
        }
        if let Some(frame) = pending.as_mut() {
            let (size, generation) = frame.geometry();
            if generation != info.generation || size != info.physical_size {
                pending = None;
                shared.push(OverlayEvent::FrameDiscarded)?;
            } else if frame.present(backend)? {
                submitted = Some(generation);
                pending = None;
            }
        }
        backend.flush()?;
        if !backend.wants_write() {
            if let Some(generation) = submitted.take() {
                shared.push(OverlayEvent::FrameSubmitted { generation })?;
            }
        }
        // Never sleep while commands remain after the bounded drain above.
        if !rx.is_empty() {
            continue;
        }
        let timeout = if pending.is_some() {
            backend.frame_wait()
        } else {
            None
        };
        let timespec = timeout
            .map(Timespec::try_from)
            .transpose()
            .map_err(|_| OverlayError::InvalidConfiguration("timeout overflow".into()))?;
        let mut flags = PollFlags::IN;
        if backend.wants_write() {
            flags |= PollFlags::OUT;
        }
        let mut fds = [
            PollFd::from_borrowed_fd(backend.fd(), flags),
            PollFd::new(&read, PollFlags::IN),
        ];
        match poll(&mut fds, timespec.as_ref()) {
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
        let mut bytes = [0u8; 128];
        while rustix::io::read(&read, &mut bytes).is_ok_and(|n| n > 0) {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsFd;

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
        let (window, rx, _read) = mock_window();
        let cleaned = Arc::new(AtomicBool::new(false));
        let marker = cleaned.clone();
        let shared = window.inner.shared.clone();
        let worker = std::thread::spawn(move || {
            CLEANUP.with(|slot| {
                *slot.borrow_mut() = Some(Cleanup(marker));
            });
            assert!(matches!(rx.recv().unwrap(), Command::Close));
            shared.running.store(false, Ordering::Release);
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
            }),
        });
        let (read, write) = rustix::pipe::pipe_with(
            rustix::pipe::PipeFlags::CLOEXEC | rustix::pipe::PipeFlags::NONBLOCK,
        )
        .unwrap();
        let (tx, rx) = bounded(8);
        let report = BackendReport {
            gpu: None,
            backend: BackendType::Wayland,
            capabilities: crate::Capabilities {
                renderer: crate::RendererType::Cpu,
                transparency: crate::Support::Supported,
                click_through: crate::Support::Supported,
                keyboard_on_demand: crate::Support::Unsupported,
                stacking: crate::StackingSupport::LayerShellOverlay,
                frame_pacing: crate::FramePacing::CompositorCallback,
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
        assert_eq!(state.events.len(), EVENT_CAPACITY);
        assert!(matches!(state.events.back(), Some(OverlayEvent::Closed)));
    }
}
