use super::OverlayBackend;
mod input;
mod presentation;
use crate::{
    Anchor, BackendReport, BackendType, ButtonState, Capabilities, Frame, FramePacing,
    KeyboardMode, OutputSelector, OverlayBuilder, OverlayError, OverlayEvent, Result, ScaleFactor,
    Size, StackingSupport, Support, WindowInfo,
};
use memmap2::{MmapMut, MmapOptions};
use std::{
    collections::BTreeMap,
    fs::File,
    os::fd::{AsFd, BorrowedFd},
    time::{Duration, Instant},
};
use wayland_client::{
    delegate_noop,
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_region,
        wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface, wl_touch,
    },
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::wp::{
    fractional_scale::v1::client::{wp_fractional_scale_manager_v1, wp_fractional_scale_v1},
    viewporter::client::{wp_viewport, wp_viewporter},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1 as shell, zwlr_layer_surface_v1 as layer,
};

struct Output {
    proxy: wl_output::WlOutput,
    scale: u32,
    name: Option<String>,
}
#[derive(Clone, Copy)]
enum Callback {
    Sync,
    Frame(u64),
}
struct State {
    presentation: Option<presentation::Manager>,
    presentation_clock: Option<u32>,
    feedbacks: BTreeMap<u64, presentation::Feedback>,
    next_submission: u64,
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    shell: Option<shell::ZwlrLayerShellV1>,
    fractional_manager: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    viewport_active: bool,
    preferred_scale: Option<u32>,
    surface: Option<wl_surface::WlSurface>,
    outputs: BTreeMap<u32, Output>,
    entered: Vec<wl_output::WlOutput>,
    seats: BTreeMap<u32, wl_seat::WlSeat>,
    pointers: BTreeMap<u32, wl_pointer::WlPointer>,
    keyboards: BTreeMap<u32, wl_keyboard::WlKeyboard>,
    keyboard_states: BTreeMap<u32, crate::input::keyboard::Keyboard>,
    text_input: crate::TextInputState,
    text_manager: Option<input::TextManager>,
    text_inputs: BTreeMap<u32, input::TextInput>,
    input_error: Option<OverlayError>,
    touches: BTreeMap<u32, wl_touch::WlTouch>,
    touch_state: crate::input::touch::TouchTracker,
    keyboard_mode: KeyboardMode,
    has_argb: bool,
    sync_done: bool,
    configured: Option<Size>,
    requested: Size,
    closed: bool,
    frame_ready: bool,
    frame_token: u64,
    released: Vec<u64>,
    events: Vec<OverlayEvent>,
    selected_output: Option<u32>,
    shell_global: Option<u32>,
}
impl State {
    fn new(config: &OverlayBuilder) -> Self {
        Self {
            presentation: None,
            presentation_clock: None,
            feedbacks: BTreeMap::new(),
            next_submission: 0,
            compositor: None,
            shm: None,
            shell: None,
            fractional_manager: None,
            viewporter: None,
            viewport_active: false,
            preferred_scale: None,
            surface: None,
            outputs: BTreeMap::new(),
            entered: Vec::new(),
            seats: BTreeMap::new(),
            pointers: BTreeMap::new(),
            keyboards: BTreeMap::new(),
            keyboard_states: BTreeMap::new(),
            text_input: config.text_input.clone(),
            text_manager: None,
            text_inputs: BTreeMap::new(),
            input_error: None,
            touches: BTreeMap::new(),
            touch_state: crate::input::touch::TouchTracker::default(),
            keyboard_mode: config.keyboard,
            has_argb: false,
            sync_done: false,
            configured: None,
            requested: config.size,
            closed: false,
            frame_ready: true,
            frame_token: 0,
            released: Vec::new(),
            events: Vec::new(),
            selected_output: None,
            shell_global: None,
        }
    }
    fn reconfigure(&mut self) {
        // Resizes must not wait for a callback belonging to obsolete content.
        self.frame_token = self.frame_token.wrapping_add(1);
        self.frame_ready = true;
    }
    fn frame_done(&mut self, token: u64) {
        if token == self.frame_token {
            self.frame_ready = true;
        }
    }
    fn scale(&self) -> Result<ScaleFactor> {
        if self.viewport_active {
            if let Some(scale) = self.preferred_scale {
                return ScaleFactor::from_120ths(scale);
            }
        }
        let supports_scale = self.surface.as_ref().is_some_and(|s| s.version() >= 3);
        if !supports_scale {
            return Ok(ScaleFactor::ONE);
        }
        ScaleFactor::from_integer(
            self.outputs
                .iter()
                .filter(|(id, output)| {
                    self.entered.contains(&output.proxy) || self.selected_output == Some(**id)
                })
                .map(|(_, output)| output.scale)
                .max()
                .unwrap_or(1),
        )
    }
}

struct Slot {
    _file: File,
    map: MmapMut,
    buffer: wl_buffer::WlBuffer,
    id: u64,
    size: Size,
    busy: bool,
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.buffer.destroy();
    }
}

pub(crate) struct WaylandBackend {
    feedback_enabled: bool,
    damage: crate::damage::Tracker,
    last_presented_slot: Option<u64>,
    #[cfg(feature = "gpu")]
    gpu: Option<crate::gpu::GpuSurface>,
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    surface: wl_surface::WlSurface,
    viewport: Option<wp_viewport::WpViewport>,
    fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    layer: layer::ZwlrLayerSurfaceV1,
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    slots: Vec<Slot>,
    next_slot: u64,
    info: WindowInfo,
    limit: usize,
    write_blocked: bool,
    timeout: Duration,
    input_region: crate::InputRegion,
}
impl WaylandBackend {
    pub(crate) fn create(
        config: &OverlayBuilder,
        deadline: Instant,
    ) -> Result<(Self, BackendReport)> {
        let conn = Connection::connect_to_env()?;
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let mut state = State::new(config);
        conn.display().get_registry(&qh, ());
        roundtrip(&conn, &mut queue, &mut state, deadline)?;
        // Registry handlers issue binds. A second barrier receives their initial
        // SHM formats, output metadata, and seat capabilities.
        roundtrip(&conn, &mut queue, &mut state, deadline)?;
        let compositor = state
            .compositor
            .clone()
            .ok_or(OverlayError::Unsupported("wl_compositor"))?;
        let shm = state
            .shm
            .clone()
            .ok_or(OverlayError::Unsupported("wl_shm"))?;
        let shell = state
            .shell
            .clone()
            .ok_or(OverlayError::Unsupported("zwlr_layer_shell_v1"))?;
        if !state.has_argb {
            return Err(OverlayError::Unsupported("wl_shm ARGB8888"));
        }
        if config.keyboard == KeyboardMode::OnDemand && shell.version() < 4 {
            return Err(OverlayError::Unsupported(
                "layer-shell v4 keyboard on-demand",
            ));
        }
        state.selected_output = match &config.output {
            OutputSelector::Automatic => None,
            OutputSelector::Name(name) => Some(
                *state
                    .outputs
                    .iter()
                    .find(|(_, output)| output.name.as_deref() == Some(name.as_str()))
                    .ok_or(OverlayError::Unsupported(
                        "requested Wayland output name (requires wl_output v4)",
                    ))?
                    .0,
            ),
        };
        let output = state
            .selected_output
            .and_then(|id| state.outputs.get(&id))
            .map(|o| &o.proxy);
        let surface = compositor.create_surface(&qh, ());
        let (viewport, fractional) = match (&state.viewporter, &state.fractional_manager) {
            (Some(viewporter), Some(manager)) => {
                state.viewport_active = true;
                (
                    Some(viewporter.get_viewport(&surface, &qh, ())),
                    Some(manager.get_fractional_scale(&surface, &qh, ())),
                )
            }
            _ => (None, None),
        };
        let layer = shell.get_layer_surface(
            &surface,
            output,
            shell::Layer::Overlay,
            config.title.clone(),
            &qh,
            (),
        );
        layer.set_anchor(anchors(config.anchor));
        if config.anchor == Anchor::Fill {
            layer.set_size(0, 0);
        } else {
            layer.set_size(config.size.width, config.size.height);
        }
        layer.set_margin(
            config.margin as i32,
            config.margin as i32,
            config.margin as i32,
            config.margin as i32,
        );
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(match config.keyboard {
            KeyboardMode::None => layer::KeyboardInteractivity::None,
            KeyboardMode::OnDemand => layer::KeyboardInteractivity::OnDemand,
        });
        apply_region(
            &surface,
            &compositor,
            &qh,
            if config.click_through {
                &crate::InputRegion::Empty
            } else {
                &config.input_region
            },
        );
        state.surface = Some(surface.clone());
        surface.commit();
        while state.configured.is_none() && !state.closed {
            wait_dispatch(&conn, &mut queue, &mut state, deadline)?;
        }
        if state.closed {
            return Err(OverlayError::Unsupported(
                "layer surface was closed during creation",
            ));
        }
        let logical = state
            .configured
            .ok_or(OverlayError::Unsupported("initial layer configure"))?;
        let scale = state.scale()?;
        let physical = scale.physical_size(logical)?;
        physical.byte_len(config.max_frame_bytes)?;
        let info = WindowInfo {
            logical_size: logical,
            physical_size: physical,
            scale_factor: scale,
            generation: 0,
        };
        let shell_version = shell.version();
        let fractional_scaling = viewport.is_some();
        let mut backend = Self {
            feedback_enabled: false,
            damage: crate::damage::Tracker::new(config.cpu_damage_tracking),
            last_presented_slot: None,
            #[cfg(feature = "gpu")]
            gpu: None,
            conn,
            queue,
            state,
            surface,
            viewport,
            fractional,
            layer,
            compositor,
            shm,
            slots: Vec::new(),
            next_slot: 0,
            info,
            limit: config.max_frame_bytes,
            write_blocked: false,
            timeout: config.command_timeout,
            input_region: config.input_region.clone(),
        };
        let mut warnings = vec!["Layer-shell controls desktop layering; lock screens and compositor UI remain compositor policy.".into()];
        if !fractional_scaling {
            warnings.push(
                "Fractional scale/viewporter pair unavailable; using integer output scaling."
                    .into(),
            );
        }
        if config.renderer != crate::RendererPreference::Cpu {
            #[cfg(feature = "gpu")]
            if let Err(err) = backend.init_gpu(config, deadline) {
                if config.renderer == crate::RendererPreference::GpuRequired {
                    return Err(err);
                }
                warnings.push(format!("GPU initialization failed; using CPU: {err}"));
            }
            #[cfg(not(feature = "gpu"))]
            warnings.push("GPU Cargo feature is disabled; using CPU.".into());
        }
        #[cfg(feature = "gpu")]
        let gpu_info = backend.gpu.as_ref().map(|gpu| gpu.info());
        #[cfg(not(feature = "gpu"))]
        let gpu_info: Option<crate::GpuAdapterInfo> = None;
        let renderer = if gpu_info.is_some() {
            crate::RendererType::Gpu
        } else {
            crate::RendererType::Cpu
        };
        let transparency = if gpu_info
            .as_ref()
            .is_some_and(|gpu| gpu.alpha_mode == "Opaque")
        {
            warnings.push("GPU swapchain is opaque under Preferred transparency policy.".into());
            Support::Unsupported
        } else {
            Support::Supported
        };
        #[cfg(feature = "gpu")]
        if backend.gpu.is_some() {
            let mut clear: Option<crate::gpu::GpuDraw> = Some(Box::new(|_| Ok(())));
            backend.present_gpu(&mut clear, info)?;
        }
        if renderer == crate::RendererType::Cpu {
            let initial = Frame::new(physical, 0, 0, config.max_frame_bytes)?;
            backend.present(&initial)?;
        }
        backend.feedback_enabled = config.presentation_feedback;
        backend.flush()?;
        let translated = if backend
            .state
            .keyboard_states
            .values()
            .any(|k| k.table.is_some())
        {
            Support::Supported
        } else {
            Support::Unsupported
        };
        let input_method = if backend.state.text_manager.is_some() {
            Support::Supported
        } else {
            Support::Unsupported
        };
        let composed = if input_method == Support::Supported
            || backend
                .state
                .keyboard_states
                .values()
                .any(|k| k.has_compose())
        {
            Support::Supported
        } else {
            Support::Unsupported
        };
        let touch = if backend.state.touches.is_empty() {
            Support::Unsupported
        } else {
            Support::Supported
        };
        if config.text_input.enabled && input_method == Support::Unsupported {
            warnings.push(
                "text-input-v3 is unavailable; text input uses local XCompose tables.".into(),
            );
        }
        if config.keyboard == KeyboardMode::OnDemand && composed == Support::Unsupported {
            warnings.push(
                "XCompose tables are unavailable; only individual-key text is available.".into(),
            );
        }
        let presentation_feedback = if backend.state.presentation.is_some() {
            Support::Supported
        } else {
            Support::Unsupported
        };
        Ok((
            backend,
            BackendReport {
                gpu: gpu_info,
                backend: BackendType::Wayland,
                capabilities: Capabilities {
                    renderer,
                    transparency,
                    click_through: Support::Supported,
                    input_regions: Support::Supported,
                    keyboard_translation: translated,
                    text_composition: composed,
                    input_method,
                    surrounding_text: input_method,
                    touch,
                    keyboard_on_demand: if shell_version >= 4 {
                        Support::Supported
                    } else {
                        Support::Unsupported
                    },
                    stacking: StackingSupport::LayerShellOverlay,
                    frame_pacing: if renderer == crate::RendererType::Gpu {
                        FramePacing::GpuFifo
                    } else {
                        FramePacing::CompositorCallback
                    },
                    integer_scaling: true,
                    presentation_feedback,
                    fractional_scaling,
                },
                fallback_attempts: Vec::new(),
                warnings,
            },
        ))
    }

    #[cfg(feature = "gpu")]
    fn init_gpu(&mut self, config: &OverlayBuilder, deadline: Instant) -> Result<()> {
        use raw_window_handle::{
            RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
        };
        let display = std::ptr::NonNull::new(self.conn.backend().display_ptr().cast())
            .ok_or_else(|| OverlayError::Gpu("native Wayland display is null".into()))?;
        let window = std::ptr::NonNull::new(self.surface.id().as_ptr().cast())
            .ok_or_else(|| OverlayError::Gpu("native Wayland surface is null".into()))?;
        // SAFETY: retain the native connection/proxy and drop this renderer in
        // WaylandBackend::drop before sending wl_surface.destroy.
        self.gpu = Some(unsafe {
            crate::gpu::GpuSurface::create(
                RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display)),
                RawWindowHandle::Wayland(WaylandWindowHandle::new(window)),
                Box::new((self.conn.clone(), self.surface.clone())),
                config,
                self.info.physical_size,
                true,
                deadline,
            )
        }?);
        Ok(())
    }

    fn prepare_surface(&self) -> Result<()> {
        if let Some(viewport) = &self.viewport {
            if self.surface.version() >= 3 {
                self.surface.set_buffer_scale(1);
            }
            viewport.set_destination(
                self.info.logical_size.width as i32,
                self.info.logical_size.height as i32,
            );
        } else if self.surface.version() >= 3 {
            self.surface
                .set_buffer_scale(self.info.scale_factor.as_integer().ok_or(
                    OverlayError::Unsupported("fractional scale without viewport"),
                )? as i32);
        }
        Ok(())
    }

    fn request_frame(&mut self) {
        self.state.frame_token = self.state.frame_token.wrapping_add(1);
        self.surface.frame(
            &self.queue.handle(),
            Callback::Frame(self.state.frame_token),
        );
    }

    fn allocate_slot(&mut self) -> Result<usize> {
        let size = self.info.physical_size;
        let bytes = size.byte_len(self.limit)?;
        let fd = rustix::fs::memfd_create(
            "liboverlay-frame",
            rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
        )
        .map_err(std::io::Error::from)?;
        rustix::fs::ftruncate(&fd, bytes as u64).map_err(std::io::Error::from)?;
        rustix::fs::fcntl_add_seals(
            &fd,
            rustix::fs::SealFlags::SHRINK | rustix::fs::SealFlags::GROW,
        )
        .map_err(std::io::Error::from)?;
        let file = File::from(fd);
        // SAFETY: a private, correctly sized memfd is retained for the entire
        // mapping lifetime and sealed against shrinking. No other local mapping
        // aliases these bytes. After commit, writes are forbidden until release.
        let map = unsafe { MmapOptions::new().len(bytes).map_mut(&file) }?;
        let qh = self.queue.handle();
        let pool = self.shm.create_pool(file.as_fd(), bytes as i32, &qh, ());
        let id = self.next_slot;
        self.next_slot = self.next_slot.wrapping_add(1);
        let buffer = pool.create_buffer(
            0,
            size.width as i32,
            size.height as i32,
            (size.width * 4) as i32,
            wl_shm::Format::Argb8888,
            &qh,
            id,
        );
        pool.destroy();
        self.slots.push(Slot {
            _file: file,
            map,
            buffer,
            id,
            size,
            busy: false,
        });
        Ok(self.slots.len() - 1)
    }
}
impl OverlayBackend for WaylandBackend {
    fn fd(&self) -> BorrowedFd<'_> {
        self.conn.as_fd()
    }
    fn info(&self) -> WindowInfo {
        self.info
    }
    fn dispatch(&mut self) -> Result<Vec<OverlayEvent>> {
        #[cfg(feature = "gpu")]
        if let Some(gpu) = &self.gpu {
            gpu.check()?;
        }
        self.queue.dispatch_pending(&mut self.state)?;
        if let Some(guard) = self.queue.prepare_read() {
            use rustix::event::{poll, PollFd, PollFlags, Timespec};
            let mut fds = [PollFd::new(&self.conn, PollFlags::IN)];
            match poll(
                &mut fds,
                Some(&Timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                }),
            ) {
                Ok(n) if n > 0 => read_guard(guard)?,
                Ok(_) | Err(rustix::io::Errno::INTR) => drop(guard),
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
        }
        self.queue.dispatch_pending(&mut self.state)?;
        for id in self.state.released.drain(..) {
            if let Some(slot) = self.slots.iter_mut().find(|slot| slot.id == id) {
                slot.busy = false;
            }
        }
        if let Some(error) = self.state.input_error.take() {
            return Err(error);
        }
        for keyboard in self.state.keyboard_states.values_mut() {
            self.state
                .events
                .extend(keyboard.tick(self.state.text_input.enabled));
        }
        let mut events = std::mem::take(&mut self.state.events);
        if self.state.closed {
            events.push(OverlayEvent::Closed);
            return Ok(events);
        }
        if let Some(logical) = self.state.configured {
            let scale = self.state.scale()?;
            let physical = scale.physical_size(logical)?;
            physical.byte_len(self.limit)?;
            if logical != self.info.logical_size
                || physical != self.info.physical_size
                || scale != self.info.scale_factor
            {
                self.info = WindowInfo {
                    logical_size: logical,
                    physical_size: physical,
                    scale_factor: scale,
                    generation: self.info.generation.wrapping_add(1),
                };
                // Busy old-generation slots survive until the server releases
                // them; at most three mappings exist across all generations.
                self.state.reconfigure();
                self.slots.retain(|slot| slot.busy || slot.size == physical);
                events.push(OverlayEvent::Resized(self.info));
            }
        }
        Ok(events)
    }
    fn present(&mut self, frame: &Frame) -> Result<bool> {
        if !self.state.frame_ready {
            return Ok(false);
        }
        self.slots
            .retain(|slot| slot.busy || slot.size == frame.size);
        let index = match self
            .slots
            .iter()
            .position(|slot| !slot.busy && slot.size == frame.size)
        {
            Some(index) => index,
            None if self.slots.len() < 3 => self.allocate_slot()?,
            None => return Ok(false),
        };
        let damage = self.damage.update(frame)?;
        if damage.is_none() {
            // Flush an empty commit to preserve FrameSubmitted's transport
            // contract, without requesting a callback that may never repaint.
            self.skip_presentation();
            self.surface.commit();
            return Ok(true);
        }
        self.prepare_surface()?;
        self.request_frame();
        self.request_presentation();
        let slot = &mut self.slots[index];
        let copy = if self.last_presented_slot == Some(slot.id) {
            damage
        } else {
            Some(crate::damage::Rectangle::full(frame.size))
        };
        if let Some(rectangle) = copy {
            for y in rectangle.y..rectangle.y + rectangle.height {
                let first = (y * frame.size.width + rectangle.x) as usize;
                let last = first + rectangle.width as usize;
                for (destination, pixel) in slot.map[first * 4..last * 4]
                    .chunks_exact_mut(4)
                    .zip(&frame.pixels[first..last])
                {
                    destination.copy_from_slice(&pixel.to_ne_bytes());
                }
            }
        }
        self.surface.attach(Some(&slot.buffer), 0, 0);
        if let Some(rectangle) = damage {
            if self.surface.version() >= 4 {
                self.surface.damage_buffer(
                    rectangle.x as i32,
                    rectangle.y as i32,
                    rectangle.width as i32,
                    rectangle.height as i32,
                );
            } else {
                // Logical rounding must cover every changed physical pixel.
                self.surface.damage(
                    0,
                    0,
                    self.info.logical_size.width as i32,
                    self.info.logical_size.height as i32,
                );
            }
        }
        self.last_presented_slot = Some(slot.id);
        self.surface.commit();
        slot.busy = true;
        self.state.frame_ready = false;
        Ok(true)
    }
    fn set_click_through(&mut self, enabled: bool) -> Result<()> {
        apply_region(
            &self.surface,
            &self.compositor,
            &self.queue.handle(),
            if enabled {
                &crate::InputRegion::Empty
            } else {
                &self.input_region
            },
        );
        self.surface.commit();
        flush_until(&self.conn, Instant::now() + self.timeout)?;
        self.write_blocked = false;
        Ok(())
    }
    fn reset_input(&mut self) -> Result<()> {
        self.state
            .events
            .extend(self.state.touch_state.cancel(None, None));
        for keyboard in self.state.keyboard_states.values_mut() {
            keyboard.reset();
        }
        self.state.refresh_text_inputs(true);
        Ok(())
    }
    fn set_text_input(&mut self, text: crate::TextInputState) -> Result<()> {
        text.validate()?;
        if self.state.keyboard_mode != KeyboardMode::OnDemand {
            return Err(OverlayError::Unsupported(
                "text input without on-demand keyboard interaction",
            ));
        }
        let reset = !text.enabled || text.change_cause == crate::TextChangeCause::Other;
        self.state.text_input = text;
        if reset {
            for keyboard in self.state.keyboard_states.values_mut() {
                keyboard.reset_compose();
                self.state.events.push(OverlayEvent::TextPreedit {
                    source: keyboard.source,
                    text: String::new(),
                    cursor: None,
                });
            }
        }
        self.state.refresh_text_inputs(false);
        flush_until(&self.conn, Instant::now() + self.timeout)?;
        Ok(())
    }
    fn set_input_region(&mut self, region: crate::InputRegion) -> Result<()> {
        region.validate()?;
        apply_region(
            &self.surface,
            &self.compositor,
            &self.queue.handle(),
            &region,
        );
        self.surface.commit();
        flush_until(&self.conn, Instant::now() + self.timeout)?;
        self.write_blocked = false;
        self.input_region = region;
        Ok(())
    }
    fn set_keyboard_capture(&mut self, enabled: bool) -> Result<()> {
        if self.state.keyboard_mode == KeyboardMode::None {
            return Err(OverlayError::Unsupported(
                "keyboard interaction was not enabled at startup",
            ));
        }
        self.layer.set_keyboard_interactivity(if enabled {
            layer::KeyboardInteractivity::Exclusive
        } else {
            layer::KeyboardInteractivity::None
        });
        if !enabled {
            self.reset_input()?;
            self.state.events.push(OverlayEvent::InputReset {
                source: None,
                scope: crate::InputResetScope::All,
                reason: crate::InputResetReason::Cancelled,
            });
        }
        self.surface.commit();
        flush_until(&self.conn, Instant::now() + self.timeout)?;
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        match self.conn.flush() {
            Ok(()) => {
                self.write_blocked = false;
                Ok(())
            }
            Err(wayland_client::backend::WaylandError::Io(error))
                if error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                self.write_blocked = true;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }
    fn wants_write(&self) -> bool {
        self.write_blocked
    }
    #[cfg(feature = "gpu")]
    fn gpu_context(&self) -> Option<crate::gpu::GpuContext> {
        self.gpu.as_ref().map(|gpu| gpu.context())
    }
    #[cfg(feature = "gpu")]
    fn present_gpu(
        &mut self,
        draw: &mut Option<crate::gpu::GpuDraw>,
        info: WindowInfo,
    ) -> Result<bool> {
        if !self.state.frame_ready {
            return Ok(false);
        }
        let texture = self
            .gpu
            .as_mut()
            .ok_or(OverlayError::Unsupported("GPU renderer"))?
            .acquire(info.physical_size)?;
        let Some(texture) = texture else {
            return Ok(false);
        };
        self.prepare_surface()?;
        self.request_frame();
        self.request_presentation();
        self.gpu
            .as_ref()
            .ok_or(OverlayError::Unsupported("GPU renderer"))?
            .draw(texture, draw, info)?;
        self.state.frame_ready = false;
        Ok(true)
    }
    fn frame_wait(&self) -> Option<Duration> {
        #[cfg(feature = "gpu")]
        if self.gpu.is_some() && self.state.frame_ready {
            return Some(Duration::from_millis(2));
        }
        None
    }
    fn needs_dispatch(&self) -> bool {
        !self.state.events.is_empty()
    }
    fn idle_wait(&self) -> Option<Duration> {
        let repeat = self
            .state
            .keyboard_states
            .values()
            .filter_map(|k| k.repeat_wait())
            .min();
        #[cfg(feature = "gpu")]
        let gpu = self.gpu.is_some().then_some(Duration::from_millis(250));
        #[cfg(not(feature = "gpu"))]
        let gpu = None;
        repeat.into_iter().chain(gpu).min()
    }
}
impl Drop for WaylandBackend {
    fn drop(&mut self) {
        #[cfg(feature = "gpu")]
        drop(self.gpu.take());
        if let Some(fractional) = self.fractional.take() {
            fractional.destroy();
        }
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
        self.layer.destroy();
        self.surface.destroy();
        self.slots.clear();
        let _ = self.conn.flush();
    }
}

fn flush_until(conn: &Connection, deadline: Instant) -> Result<()> {
    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    loop {
        match conn.flush() {
            Ok(()) => return Ok(()),
            Err(wayland_client::backend::WaylandError::Io(error))
                if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(OverlayError::Timeout("flushing Wayland requests"))?;
        let timeout = Timespec::try_from(remaining).map_err(std::io::Error::other)?;
        let mut fds = [PollFd::new(conn, PollFlags::OUT)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => return Err(OverlayError::Timeout("flushing Wayland requests")),
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
    }
}
fn read_guard(guard: wayland_client::backend::ReadEventsGuard) -> Result<()> {
    match guard.read() {
        Ok(_) => Ok(()),
        Err(wayland_client::backend::WaylandError::Io(error))
            if error.kind() == std::io::ErrorKind::WouldBlock =>
        {
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}
fn wait_dispatch(
    conn: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<()> {
    if queue.dispatch_pending(state)? > 0 {
        return Ok(());
    }
    flush_until(conn, deadline)?;
    if let Some(guard) = queue.prepare_read() {
        super::wait_readable(conn.as_fd(), deadline, false)?;
        read_guard(guard)?;
    }
    queue.dispatch_pending(state)?;
    Ok(())
}
fn roundtrip(
    conn: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<()> {
    state.sync_done = false;
    conn.display().sync(&queue.handle(), Callback::Sync);
    while !state.sync_done {
        wait_dispatch(conn, queue, state, deadline)?;
    }
    Ok(())
}
fn anchors(anchor: Anchor) -> layer::Anchor {
    use layer::Anchor as A;
    match anchor {
        Anchor::TopLeft => A::Top | A::Left,
        Anchor::Top => A::Top,
        Anchor::TopRight => A::Top | A::Right,
        Anchor::Left => A::Left,
        Anchor::Center => A::empty(),
        Anchor::Right => A::Right,
        Anchor::BottomLeft => A::Bottom | A::Left,
        Anchor::Bottom => A::Bottom,
        Anchor::BottomRight => A::Bottom | A::Right,
        Anchor::Fill => A::Top | A::Bottom | A::Left | A::Right,
    }
}

fn apply_region(
    surface: &wl_surface::WlSurface,
    compositor: &wl_compositor::WlCompositor,
    qh: &QueueHandle<State>,
    input: &crate::InputRegion,
) {
    if matches!(input, crate::InputRegion::Full) {
        surface.set_input_region(None);
        return;
    }
    let region = compositor.create_region(qh, ());
    if let crate::InputRegion::Rectangles(rectangles) = input {
        for rect in rectangles {
            region.add(
                i32::from(rect.x),
                i32::from(rect.y),
                i32::from(rect.width),
                i32::from(rect.height),
            );
        }
    }
    surface.set_input_region(Some(&region));
    region.destroy();
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_compositor" if state.compositor.is_none() => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()))
                }
                "wp_presentation" if state.presentation.is_none() => {
                    state.presentation = Some(registry.bind(name, 1, qh, ()));
                }
                "wl_shm" if state.shm.is_none() => state.shm = Some(registry.bind(name, 1, qh, ())),
                "wp_viewporter" if state.viewporter.is_none() => {
                    state.viewporter = Some(registry.bind(name, 1, qh, ()))
                }
                "wp_fractional_scale_manager_v1" if state.fractional_manager.is_none() => {
                    state.fractional_manager = Some(registry.bind(name, 1, qh, ()))
                }
                "zwlr_layer_shell_v1" if state.shell.is_none() => {
                    state.shell = Some(registry.bind(name, version.min(5), qh, ()));
                    state.shell_global = Some(name);
                }
                "wl_output" => {
                    let proxy = registry.bind(name, version.min(4), qh, name);
                    state.outputs.insert(
                        name,
                        Output {
                            proxy,
                            scale: 1,
                            name: None,
                        },
                    );
                }
                "wl_seat" => {
                    let seat = registry.bind(name, version.min(5), qh, name);
                    state.seats.insert(name, seat);
                }
                "zwp_text_input_manager_v3"
                    if state.keyboard_mode == KeyboardMode::OnDemand
                        && state.text_manager.is_none() =>
                {
                    state.text_manager = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if state.shell_global == Some(name) || state.selected_output == Some(name) {
                    state.closed = true;
                }
                if let Some(output) = state.outputs.remove(&name) {
                    state.entered.retain(|o| o != &output.proxy);
                    if output.proxy.version() >= 3 {
                        output.proxy.release();
                    }
                }
                if let Some(pointer) = state.pointers.remove(&name) {
                    if pointer.version() >= 3 {
                        pointer.release();
                    }
                }
                if let Some(keyboard) = state.keyboards.remove(&name) {
                    if keyboard.version() >= 3 {
                        keyboard.release();
                    }
                }
                if let Some(mut keyboard) = state.keyboard_states.remove(&name) {
                    state.events.extend(keyboard.focus(false));
                }
                if let Some(text) = state.text_inputs.remove(&name) {
                    text.proxy.destroy();
                }
                if let Some(seat) = state.seats.remove(&name) {
                    if seat.version() >= 5 {
                        seat.release();
                    }
                }
                if let Some(touch) = state.touches.remove(&name) {
                    state
                        .events
                        .extend(state.touch_state.cancel(Some(name), None));
                    if touch.version() >= 3 {
                        touch.release();
                    }
                }
            }
            _ => {}
        }
        state.ensure_text_inputs(qh);
    }
}
impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let Some(output) = state.outputs.get_mut(id) {
            match event {
                wl_output::Event::Scale { factor } => output.scale = factor.max(1) as u32,
                wl_output::Event::Name { name } => output.name = Some(name),
                _ => {}
            }
        }
    }
}
impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_shm::WlShm,
        event: wl_shm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_shm::Event::Format {
            format: WEnum::Value(wl_shm::Format::Argb8888),
        } = event
        {
            state.has_argb = true;
        }
    }
}
impl Dispatch<layer::ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &layer::ZwlrLayerSurfaceV1,
        event: layer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            layer::Event::Configure {
                serial,
                width,
                height,
            } => {
                proxy.ack_configure(serial);
                state.reconfigure();
                state.configured = Some(Size::new(
                    if width == 0 {
                        state.requested.width
                    } else {
                        width
                    },
                    if height == 0 {
                        state.requested.height
                    } else {
                        height
                    },
                ));
            }
            layer::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}
impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::Enter { output } => {
                if !state.entered.contains(&output) {
                    state.entered.push(output);
                }
            }
            wl_surface::Event::Leave { output } => state.entered.retain(|o| o != &output),
            _ => {}
        }
    }
}
impl Dispatch<wl_callback::WlCallback, Callback> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        kind: &Callback,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match kind {
            Callback::Sync => state.sync_done = true,
            Callback::Frame(token) => state.frame_done(*token),
        }
    }
}
impl Dispatch<wl_buffer::WlBuffer, u64> for State {
    fn event(
        state: &mut Self,
        _: &wl_buffer::WlBuffer,
        event: wl_buffer::Event,
        id: &u64,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.released.push(*id);
        }
    }
}
delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_region::WlRegion);
delegate_noop!(State: ignore shell::ZwlrLayerShellV1);
delegate_noop!(State: ignore wp_viewporter::WpViewporter);
delegate_noop!(State: ignore wp_viewport::WpViewport);
delegate_noop!(State: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            if state.preferred_scale != Some(scale) {
                state.preferred_scale = Some(scale);
                state.reconfigure();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fractional_scale_requires_both_protocols_and_rejects_zero() {
        let mut state = State::new(&OverlayBuilder::new());
        state.preferred_scale = Some(150);
        assert_eq!(state.scale().unwrap(), ScaleFactor::ONE);
        state.viewport_active = true;
        assert_eq!(
            state.scale().unwrap(),
            ScaleFactor::from_120ths(150).unwrap()
        );
        state.preferred_scale = Some(0);
        assert!(state.scale().is_err());
    }
    #[test]
    fn resize_unblocks_rendering_and_obsolete_callbacks_cannot_unthrottle_it() {
        let mut state = State::new(&OverlayBuilder::new());
        state.frame_token = 4;
        state.frame_ready = false;
        state.reconfigure();
        assert!(state.frame_ready);
        state.frame_token += 1; // new-size buffer committed
        state.frame_ready = false;
        state.frame_done(4); // delayed callback from before resize
        assert!(!state.frame_ready);
        state.frame_done(state.frame_token);
        assert!(state.frame_ready);
    }
}
