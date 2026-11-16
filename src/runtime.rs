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
        Arc, Mutex, Weak,
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
    frames: Vec<Frame>,
    input_reset: bool,
}
struct Shared {
    running: AtomicBool,
    state: Mutex<State>,
}
impl Shared {
    fn recycle(&self, job: FrameJob) -> Result<()> {
        #[allow(irrefutable_let_patterns)]
        if let FrameJob::Cpu(frame) = job {
            let mut state = self.state.lock().map_err(|_| OverlayError::Poisoned)?;
            if state.frames.len() < 2 && frame.size == state.info.physical_size {
                state.frames.push(frame);
            }
        }
        Ok(())
    }
    fn push(&self, event: OverlayEvent) -> Result<()> {
        let mut state = self.state.lock().map_err(|_| OverlayError::Poisoned)?;
        if matches!(event, OverlayEvent::PointerMoved { .. })
            && matches!(state.events.back(), Some(OverlayEvent::PointerMoved { .. }))
        {
            state.events.pop_back();
        }
        if state.events.len() == EVENT_CAPACITY {
            state.events.clear();
            state.events.push_back(OverlayEvent::InputReset {
                source: None,
                scope: crate::InputResetScope::All,
                reason: crate::InputResetReason::QueueOverflow,
            });
            state.input_reset = true;
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
    InputRegion(crate::InputRegion, Sender<Result<()>>),
    KeyboardCapture(bool, Sender<Result<()>>),
    TextInput(crate::TextInputState, Sender<Result<()>>),
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
    keyboard_enabled: bool,
}

/// Cloneable, `Send + Sync` handle to an overlay's private display runtime.
#[derive(Clone)]
pub struct OverlayWindow {
    inner: Arc<Inner>,
}

/// Non-owning window reference for callbacks and other lifetime-sensitive state.
/// It does not keep the display runtime alive.
#[derive(Debug, Clone)]
pub struct WeakOverlayWindow {
    inner: Weak<Inner>,
}

impl WeakOverlayWindow {
    /// Obtains an owning handle while one still exists. The window may already
    /// be closed; inspect `OverlayWindow::is_running` before issuing commands.
    pub fn upgrade(&self) -> Option<OverlayWindow> {
        self.inner.upgrade().map(|inner| OverlayWindow { inner })
    }
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
    /// Creates a non-owning reference suitable for a queued GPU callback.
    /// Capturing a strong clone in a stalled callback can prolong window lifetime.
    pub fn downgrade(&self) -> WeakOverlayWindow {
        WeakOverlayWindow {
            inner: Arc::downgrade(&self.inner),
        }
    }
    /// Returns the display backend selected during startup.
    pub fn backend_type(&self) -> BackendType {
        self.inner.report.backend
    }
    /// Returns the immutable startup capabilities, fallback attempts and warnings.
    pub fn backend_report(&self) -> &BackendReport {
        &self.inner.report
    }
    /// Returns whether the runtime is accepting commands.
    pub fn is_running(&self) -> bool {
        self.inner.shared.running.load(Ordering::Acquire)
    }
    /// Returns the latest geometry snapshot, also available after closure.
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
        let (info, recycled) = {
            let mut state = self
                .inner
                .shared
                .state
                .lock()
                .map_err(|_| OverlayError::Poisoned)?;
            (state.info, state.frames.pop())
        };
        if let Some(mut frame) = recycled.filter(|frame| frame.size == info.physical_size) {
            frame.generation = info.generation;
            frame.pixels.fill(0);
            return Ok(frame);
        }
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
    /// Obtains a transparent CPU frame, draws on the calling thread and queues it.
    /// The callback receives physical dimensions; its panic propagates to the caller.
    pub fn draw(&self, draw: impl FnOnce(&mut [u32], u32, u32)) -> Result<()> {
        let mut frame = self.frame()?;
        let size = frame.size;
        draw(&mut frame.pixels, size.width, size.height);
        self.present(frame)
    }
    #[cfg(feature = "gpu")]
    /// Returns cloneable GPU resources for preparing pipelines before drawing.
    /// Device resources may outlive the window. Destroying the device closes its runtime.
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
    /// Use `downgrade()` when the callback needs to reference this window without
    /// keeping it alive while presentation is stalled.
    /// Callback panics become `RuntimePanic` with `panic=unwind`; a `panic=abort`
    /// executable retains its process-aborting behavior.
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
    /// Applies a pointer/touch region and disables click-through. A later
    /// `set_click_through(true)` masks it until `set_click_through(false)`.
    /// The reply confirms flushing, independently of render pacing; it does not
    /// confirm when the server routes a physical input event through the region.
    pub fn set_input_region(&self, region: crate::InputRegion) -> Result<()> {
        region.validate()?;
        if on_worker_thread(&self.inner.worker)? {
            return Err(OverlayError::Unsupported(
                "synchronous input commands from a GPU callback",
            ));
        }
        let (tx, rx) = bounded(1);
        self.send(Command::InputRegion(region, tx))?;
        rx.recv_timeout(self.inner.timeout)
            .map_err(|error| match error {
                crossbeam_channel::RecvTimeoutError::Timeout => {
                    OverlayError::Timeout("setting the input region")
                }
                crossbeam_channel::RecvTimeoutError::Disconnected => OverlayError::Closed,
            })?
    }
    /// Captures keyboard focus for an interactive Wayland layer, or yields it.
    /// Requires `KeyboardMode::OnDemand` at startup. Enabled requests exclusive
    /// compositor focus; disabled requests no keyboard focus, returning control
    /// to the underlying application. Pointer input is configured independently.
    /// Other backends return `Unsupported`. Never call from a GPU callback.
    pub fn set_keyboard_capture(&self, enabled: bool) -> Result<()> {
        if !self.inner.keyboard_enabled {
            return Err(OverlayError::Unsupported(
                "keyboard interaction was not enabled at startup",
            ));
        }
        if on_worker_thread(&self.inner.worker)? {
            return Err(OverlayError::Unsupported(
                "synchronous keyboard capture from a GPU callback",
            ));
        }
        let (tx, rx) = bounded(1);
        self.send(Command::KeyboardCapture(enabled, tx))?;
        rx.recv_timeout(self.inner.timeout)
            .map_err(|error| match error {
                crossbeam_channel::RecvTimeoutError::Timeout => {
                    OverlayError::Timeout("changing keyboard capture")
                }
                crossbeam_channel::RecvTimeoutError::Disconnected => OverlayError::Closed,
            })?
    }
    /// Updates or disables the focused text field. This requires on-demand
    /// keyboard interaction at startup. Surrounding text uses UTF-8 byte offsets;
    /// `TextCommitted` and `TextDeleteSurrounding` must be applied by the caller.
    pub fn set_text_input(&self, state: crate::TextInputState) -> Result<()> {
        state.validate()?;
        if !self.inner.keyboard_enabled {
            return Err(OverlayError::Unsupported(
                "text input on an overlay without keyboard interaction",
            ));
        }
        if on_worker_thread(&self.inner.worker)? {
            return Err(OverlayError::Unsupported(
                "synchronous input commands from a GPU callback",
            ));
        }
        let (tx, rx) = bounded(1);
        self.send(Command::TextInput(state, tx))?;
        rx.recv_timeout(self.inner.timeout)
            .map_err(|error| match error {
                crossbeam_channel::RecvTimeoutError::Timeout => {
                    OverlayError::Timeout("updating text input")
                }
                crossbeam_channel::RecvTimeoutError::Disconnected => OverlayError::Closed,
            })?
    }
    /// Drains the shared bounded event queue. Clones consume the same queue;
    /// events are not broadcast. A fatal error is returned once, before queued events.
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
    /// Stops rendering for every clone and waits for display resource cleanup.
    /// Closure bypasses the bounded drawing queue, including when it is full.
    pub fn close(&self) -> Result<()> {
        if on_worker_thread(&self.inner.worker)? {
            return Err(OverlayError::Unsupported(
                "synchronous close from a GPU callback",
            ));
        }
        self.inner.shared.running.store(false, Ordering::Release);
        // The reader may already have closed after observing the stop flag.
        let _ = wake(&self.inner.wake);
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
    match rustix::io::write(fd, &1u64.to_ne_bytes()) {
        Ok(_) | Err(rustix::io::Errno::AGAIN) => Ok(()),
        Err(error) => Err(std::io::Error::from(error).into()),
    }
}

fn wake_pair() -> std::io::Result<(OwnedFd, OwnedFd)> {
    // An eventfd has no disconnected-reader SIGPIPE. Rust executables normally
    // ignore that signal, but C/C++ hosts may keep its default termination action.
    let read = rustix::event::eventfd(
        0,
        rustix::event::EventfdFlags::CLOEXEC | rustix::event::EventfdFlags::NONBLOCK,
    )?;
    let write = rustix::io::fcntl_dupfd_cloexec(&read, 0)?;
    Ok((read, write))
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
            frames: Vec::new(),
            input_reset: false,
        }),
    });
    let (read, write) = wake_pair()?;
    let (tx, rx) = bounded(8);
    let (ready_tx, ready_rx) = bounded(1);
    let timeout = config.startup_timeout;
    let command_timeout = config.command_timeout;
    let limit = config.max_frame_bytes;
    let keyboard_enabled = config.keyboard == crate::KeyboardMode::OnDemand;
    let worker_shared = shared.clone();
    let worker = std::thread::Builder::new()
        .name(format!("liboverlay-{id}"))
        .spawn(move || {
            // Also marks closure if a dependency unexpectedly unwinds.
            struct RunningGuard(Arc<Shared>);
            impl Drop for RunningGuard {
                fn drop(&mut self) {
                    if let Ok(mut state) = self.0.state.lock() {
                        state.frames.clear();
                    }
                    let _ = self.0.push(OverlayEvent::InputReset {
                        source: None,
                        scope: crate::InputResetScope::All,
                        reason: crate::InputResetReason::Closed,
                    });
                    let _ = self.0.push(OverlayEvent::Closed);
                    self.0.running.store(false, Ordering::Release);
                }
            }
            let _guard = RunningGuard(worker_shared.clone());
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
            }));
            if result.is_err() {
                // Preserve a typed error even when a rendering callback or a
                // graphics dependency unwinds. The normal panic hook still runs.
                if let Ok(mut state) = worker_shared.state.lock() {
                    #[cfg(feature = "gpu")]
                    {
                        state.gpu = None;
                    }
                    state.terminal = Some(OverlayError::RuntimePanic);
                }
                let _ = ready_tx.try_send(Err(OverlayError::RuntimePanic));
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
            keyboard_enabled,
        }),
    })
}

fn dispatch_events(
    backend: &mut dyn OverlayBackend,
    shared: &Shared,
) -> Result<Option<WindowInfo>> {
    let reset = {
        let mut state = shared.state.lock().map_err(|_| OverlayError::Poisoned)?;
        std::mem::take(&mut state.input_reset)
    };
    if reset {
        backend.reset_input()?;
    }
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
                    if let Some(replaced) = pending.replace(FrameJob::Cpu(frame)) {
                        shared.recycle(replaced)?;
                        shared.push(OverlayEvent::FrameDiscarded)?;
                    }
                }
                #[cfg(feature = "gpu")]
                Ok(Command::Gpu(info, draw)) => {
                    if let Some(replaced) = pending.replace(FrameJob::Gpu {
                        info,
                        draw: Some(draw),
                    }) {
                        shared.recycle(replaced)?;
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
                Ok(Command::InputRegion(region, reply)) => {
                    buffered_events_possible = true;
                    let result = backend
                        .set_input_region(region)
                        .and_then(|()| backend.flush());
                    let fatal = result
                        .as_ref()
                        .err()
                        .is_some_and(|e| !matches!(e, OverlayError::Unsupported(_)));
                    let _ = reply.send(result);
                    if fatal {
                        return Err(OverlayError::Closed);
                    }
                }
                Ok(Command::KeyboardCapture(enabled, reply)) => {
                    buffered_events_possible = true;
                    let result = backend
                        .set_keyboard_capture(enabled)
                        .and_then(|()| backend.flush());
                    let fatal = result
                        .as_ref()
                        .err()
                        .is_some_and(|e| !matches!(e, OverlayError::Unsupported(_)));
                    let _ = reply.send(result);
                    if fatal {
                        return Err(OverlayError::Closed);
                    }
                }
                Ok(Command::TextInput(state, reply)) => {
                    buffered_events_possible = true;
                    let result = backend.set_text_input(state).and_then(|()| backend.flush());
                    let fatal = result
                        .as_ref()
                        .err()
                        .is_some_and(|e| !matches!(e, OverlayError::Unsupported(_)));
                    let _ = reply.send(result);
                    if fatal {
                        return Err(OverlayError::Closed);
                    }
                }
                Err(TryRecvError::Disconnected) => return Ok(()),
                Err(TryRecvError::Empty) => break,
            }
        }
        if !shared.running.load(Ordering::Acquire) {
            return Ok(());
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
                if let Some(discarded) = pending.take() {
                    shared.recycle(discarded)?;
                }
                shared.push(OverlayEvent::FrameDiscarded)?;
            } else if frame.present(backend)? {
                submitted = Some(generation);
                if let Some(done) = pending.take() {
                    shared.recycle(done)?;
                }
            }
        }
        backend.flush()?;
        if !backend.wants_write() {
            if let Some(generation) = submitted.take() {
                shared.push(OverlayEvent::FrameSubmitted { generation })?;
            }
        }
        // Never sleep while commands remain after the bounded drain above.
        if !rx.is_empty() || backend.needs_dispatch() {
            continue;
        }
        let timeout = if pending.is_some() {
            backend.frame_wait()
        } else {
            None
        };
        // Device loss must also be detected while a compositor withholds frame
        // callbacks, or while no drawing command is pending.
        let timeout = timeout.into_iter().chain(backend.idle_wait()).min();
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
mod tests;
