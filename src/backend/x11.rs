use super::OverlayBackend;
use crate::{
    Anchor, BackendReport, BackendType, ButtonState, Capabilities, Frame, FramePacing,
    KeyboardMode, OutputSelector, OverlayBuilder, OverlayError, OverlayEvent, Requirement, Result,
    Size, StackingPolicy, StackingSupport, Support, WindowInfo,
};
use std::{
    cell::Cell,
    io,
    os::fd::{AsFd, BorrowedFd},
    time::{Duration, Instant},
};
use x11rb::{
    connection::{Connection, RequestConnection},
    properties::WmHints,
    protocol::{
        present::{self, ConnectionExt as _},
        randr::{self, ConnectionExt as _},
        render::{self, ConnectionExt as _},
        shape::{self, ConnectionExt as _},
        xproto::*,
        Event,
    },
    reexports::x11rb_protocol::{parse_display, xauth},
    rust_connection::{DefaultStream, PollMode, RustConnection, Stream},
    utils::RawFdContainer,
    wrapper::ConnectionExt as _,
};

struct TimedStream {
    inner: DefaultStream,
    deadline: Cell<Option<Instant>>,
    timeout: Duration,
}
impl AsFd for TimedStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}
impl Stream for TimedStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        use rustix::event::{poll, PollFd, PollFlags, Timespec};
        let deadline = self
            .deadline
            .get()
            .unwrap_or_else(|| Instant::now() + self.timeout);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::TimedOut, "X11 server did not respond")
                })?;
            let mut flags = PollFlags::empty();
            if mode.readable() {
                flags |= PollFlags::IN;
            }
            if mode.writable() {
                flags |= PollFlags::OUT;
            }
            let mut fds = [PollFd::new(self, flags)];
            let timeout = Timespec::try_from(remaining).map_err(io::Error::other)?;
            match poll(&mut fds, Some(&timeout)) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "X11 server did not respond",
                    ))
                }
                Ok(_) => return Ok(()),
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn read(&self, bytes: &mut [u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.read(bytes, fds)
    }
    fn write(&self, bytes: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.write(bytes, fds)
    }
    fn write_vectored(
        &self,
        bytes: &[io::IoSlice<'_>],
        fds: &mut Vec<RawFdContainer>,
    ) -> io::Result<usize> {
        self.inner.write_vectored(bytes, fds)
    }
}
type XConnection = RustConnection<TimedStream>;

// COPY prevents a flip from keeping our single upload pixmap busy until a
// subsequent frame. Both notifications are required before its next upload:
// Idle permits reuse, Complete supplies the next vertical-blank target.
struct PresentState {
    event: present::Event,
    serial: u32,
    idle: bool,
    complete: bool,
    msc: u64,
}
impl PresentState {
    fn new(event: present::Event) -> Self {
        Self {
            event,
            serial: 0,
            idle: true,
            complete: true,
            msc: 0,
        }
    }
    fn ready(&self) -> bool {
        self.idle && self.complete
    }
    fn submitted(&mut self) {
        self.serial = self.serial.wrapping_add(1);
        self.idle = false;
        self.complete = false;
    }
    fn completed(&mut self, serial: u32, msc: u64) {
        if serial == self.serial {
            self.complete = true;
            self.msc = msc;
        }
    }
    fn released(&mut self, serial: u32) {
        if serial == self.serial {
            self.idle = true;
        }
    }
}

fn connect(config: &OverlayBuilder, deadline: Instant) -> Result<(XConnection, usize, String)> {
    let display_name =
        std::env::var("DISPLAY").map_err(|e| crate::error::backend_error("X11", e))?;
    let display = parse_display::parse_display(Some(&display_name))
        .map_err(|e| crate::error::backend_error("X11", e))?;
    // The address iterator computes an optional TCP port as u16, even when
    // only its Unix-socket addresses are used below.
    if display.display > u16::MAX - 6000 {
        return Err(OverlayError::InvalidConfiguration(
            "X11 display number is out of range".into(),
        ));
    }
    let screen = usize::from(display.screen);
    let mut last = None;
    for address in display.connect_instruction() {
        // The initial release deliberately supports local Unix sockets only,
        // avoiding unbounded DNS/TCP connection attempts in the display worker.
        if !matches!(address, parse_display::ConnectAddress::Socket(_)) {
            continue;
        }
        match DefaultStream::connect(&address) {
            Ok((stream, (family, address))) => {
                let auth = match xauth::get_auth(family, &address, display.display) {
                    Ok(auth) => auth,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error.into()),
                };
                let (auth_name, auth_data) = auth.unwrap_or_default();
                let stream = TimedStream {
                    inner: stream,
                    deadline: Cell::new(Some(deadline)),
                    timeout: config.command_timeout,
                };
                return Ok((
                    RustConnection::connect_to_stream_with_auth_info(
                        stream, screen, auth_name, auth_data,
                    )?,
                    screen,
                    display_name,
                ));
            }
            Err(error) => last = Some(error),
        }
    }
    Err(last
        .map(OverlayError::Io)
        .unwrap_or(OverlayError::Unsupported("local X11 Unix socket")))
}

pub(crate) struct X11Backend {
    #[cfg(feature = "gpu")]
    gpu: Option<crate::gpu::GpuSurface>,
    conn: XConnection,
    window: Window,
    pixmap: Pixmap,
    gc: Gcontext,
    colormap: Colormap,
    depth: u8,
    shape: bool,
    info: WindowInfo,
    limit: usize,
    delete_atom: Atom,
    protocols_atom: Atom,
    last_present: Option<Instant>,
    interval: Duration,
    present: Option<PresentState>,
    repaint: bool,
}

impl X11Backend {
    pub(crate) fn create(
        config: &OverlayBuilder,
        deadline: Instant,
    ) -> Result<(Self, BackendReport)> {
        if config.stacking == StackingPolicy::RequireOverlayLayer {
            return Err(OverlayError::Unsupported(
                "layer-shell overlay stacking on X11",
            ));
        }
        let (conn, screen_number, _display_name) = connect(config, deadline)?;
        let screen = &conn.setup().roots[screen_number];
        let root = screen.root;
        let shape = if conn
            .extension_information(shape::X11_EXTENSION_NAME)?
            .is_some()
        {
            let version = conn.shape_query_version()?.reply()?;
            (version.major_version, version.minor_version) >= (1, 1)
        } else {
            false
        };
        if config.click_through && !shape {
            return Err(OverlayError::Unsupported("SHAPE 1.1 input regions"));
        }
        let mut warnings = vec!["X11 stacking is a compositor/window-manager policy; fullscreen coverage is not guaranteed.".into()];

        let mut argb_visual = None;
        if conn
            .extension_information(render::X11_EXTENSION_NAME)?
            .is_some()
        {
            let formats = conn.render_query_pict_formats()?.reply()?;
            if let Some(pict_screen) = formats.screens.get(screen_number) {
                for depth in &pict_screen.depths {
                    if depth.depth != 32 {
                        continue;
                    }
                    for visual in &depth.visuals {
                        let matches = formats.formats.iter().any(|f| {
                            f.id == visual.format
                                && f.type_ == render::PictType::DIRECT
                                && f.depth == 32
                                && f.direct.alpha_shift == 24
                                && f.direct.alpha_mask == 255
                                && f.direct.red_shift == 16
                                && f.direct.red_mask == 255
                                && f.direct.green_shift == 8
                                && f.direct.green_mask == 255
                                && f.direct.blue_shift == 0
                                && f.direct.blue_mask == 255
                        });
                        if matches
                            && screen
                                .allowed_depths
                                .iter()
                                .filter(|d| d.depth == 32)
                                .flat_map(|d| &d.visuals)
                                .any(|v| {
                                    v.visual_id == visual.visual
                                        && v.class == VisualClass::TRUE_COLOR
                                })
                        {
                            argb_visual = Some(visual.visual);
                            break;
                        }
                    }
                    if argb_visual.is_some() {
                        break;
                    }
                }
            }
        }
        let cm_atom = atom(&conn, format!("_NET_WM_CM_S{screen_number}").as_bytes())?;
        let has_compositor = conn.get_selection_owner(cm_atom)?.reply()?.owner != x11rb::NONE;
        let is_xwayland = conn.query_extension(b"XWAYLAND")?.reply()?.present;
        let transparent = argb_visual.is_some() && (has_compositor || is_xwayland);
        if !transparent && config.transparency == Requirement::Required {
            return Err(OverlayError::Unsupported(
                "ARGB visual and an X11 compositor (or XWayland)",
            ));
        }
        if is_xwayland {
            warnings.push("Connected to XWayland; the Wayland compositor controls native/X11 window stacking.".into());
        }
        if !transparent {
            warnings.push(
                "Transparency unavailable: explicit Preferred policy permits an opaque window."
                    .into(),
            );
        }
        let (visual, depth) = if transparent {
            (
                argb_visual.ok_or(OverlayError::Unsupported("ARGB visual"))?,
                32,
            )
        } else {
            (screen.root_visual, screen.root_depth)
        };
        // CPU upload currently accepts standard 32-bit RGB/ARGB storage only.
        if !conn
            .setup()
            .pixmap_formats
            .iter()
            .any(|f| f.depth == depth && f.bits_per_pixel == 32 && f.scanline_pad == 32)
        {
            return Err(OverlayError::Unsupported("32-bit X11 image storage"));
        }
        let visual_info = screen
            .allowed_depths
            .iter()
            .flat_map(|d| &d.visuals)
            .find(|v| v.visual_id == visual)
            .ok_or(OverlayError::Unsupported("X11 visual"))?;
        if visual_info.red_mask != 0xff0000
            || visual_info.green_mask != 0xff00
            || visual_info.blue_mask != 0xff
        {
            return Err(OverlayError::Unsupported("standard X11 RGB channel masks"));
        }
        let (mx, my, mw, mh) = monitor_geometry(
            &conn,
            root,
            screen.width_in_pixels,
            screen.height_in_pixels,
            &config.output,
            &mut warnings,
        )?;
        let size = if config.anchor == Anchor::Fill {
            Size::new(
                mw.checked_sub(config.margin * 2)
                    .ok_or(OverlayError::InvalidSize(config.size))?,
                mh.checked_sub(config.margin * 2)
                    .ok_or(OverlayError::InvalidSize(config.size))?,
            )
        } else {
            config.size
        };
        size.byte_len(config.max_frame_bytes)?;
        let (x, y) = placement(config.anchor, config.margin, size, (mx, my, mw, mh))?;
        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, root, visual)?
            .check()?;
        let window = conn.generate_id()?;
        let managed = config.keyboard == KeyboardMode::OnDemand;
        conn.create_window(
            depth,
            window,
            root,
            x,
            y,
            size.width as u16,
            size.height as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .colormap(colormap)
                .border_pixel(0)
                .background_pixel(0)
                .override_redirect(u32::from(!managed))
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::STRUCTURE_NOTIFY
                        | EventMask::POINTER_MOTION
                        | EventMask::BUTTON_PRESS
                        | EventMask::BUTTON_RELEASE
                        | EventMask::ENTER_WINDOW
                        | EventMask::LEAVE_WINDOW
                        | EventMask::KEY_PRESS
                        | EventMask::KEY_RELEASE,
                ),
        )?
        .check()?;
        let pixmap = conn.generate_id()?;
        conn.create_pixmap(depth, pixmap, window, size.width as u16, size.height as u16)?
            .check()?;
        let gc = conn.generate_id()?;
        conn.create_gc(
            gc,
            pixmap,
            &CreateGCAux::new().foreground(0).graphics_exposures(0),
        )?
        .check()?;
        conn.poly_fill_rectangle(
            pixmap,
            gc,
            &[Rectangle {
                x: 0,
                y: 0,
                width: size.width as u16,
                height: size.height as u16,
            }],
        )?
        .check()?;
        conn.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().background_pixmap(pixmap),
        )?
        .check()?;
        let utf8 = atom(&conn, b"UTF8_STRING")?;
        let name = atom(&conn, b"_NET_WM_NAME")?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            name,
            utf8,
            config.title.as_bytes(),
        )?
        .check()?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            config.title.as_bytes(),
        )?
        .check()?;
        conn.change_property8(
            PropMode::REPLACE,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            b"liboverlay\0liboverlay\0",
        )?
        .check()?;
        let delete_atom = atom(&conn, b"WM_DELETE_WINDOW")?;
        let protocols_atom = atom(&conn, b"WM_PROTOCOLS")?;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            protocols_atom,
            AtomEnum::ATOM,
            &[delete_atom],
        )?
        .check()?;
        let mut hints = WmHints::new();
        hints.input = Some(managed);
        hints.set(&conn, window)?.check()?;
        let mut stacking = if managed {
            StackingSupport::WindowManagerControlled
        } else {
            StackingSupport::UnmanagedRaise
        };
        if managed {
            let above = atom(&conn, b"_NET_WM_STATE_ABOVE")?;
            let state = atom(&conn, b"_NET_WM_STATE")?;
            let supported = atom(&conn, b"_NET_SUPPORTED")?;
            let reply = conn
                .get_property(false, root, supported, AtomEnum::ATOM, 0, 4096)?
                .reply()?;
            if reply
                .value32()
                .is_some_and(|mut values| values.any(|v| v == above))
            {
                conn.change_property32(PropMode::REPLACE, window, state, AtomEnum::ATOM, &[above])?
                    .check()?;
                stacking = StackingSupport::WindowManagerHint;
            } else {
                warnings.push("Window manager does not advertise _NET_WM_STATE_ABOVE.".into());
            }
        }
        let info = WindowInfo {
            logical_size: size,
            physical_size: size,
            scale_factor: crate::ScaleFactor::ONE,
            generation: 0,
        };
        let mut backend = Self {
            #[cfg(feature = "gpu")]
            gpu: None,
            conn,
            window,
            pixmap,
            gc,
            colormap,
            depth,
            shape,
            info,
            limit: config.max_frame_bytes,
            delete_atom,
            protocols_atom,
            last_present: None,
            interval: config.frame_interval,
            present: None,
            repaint: false,
        };
        backend.set_click_through(config.click_through)?;
        backend.conn.map_window(window)?.check()?;
        if !managed {
            backend
                .conn
                .configure_window(
                    window,
                    &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
                )?
                .check()?;
        }
        backend.conn.flush()?;
        backend.conn.stream().deadline.set(None);
        if config.renderer != crate::RendererPreference::Cpu {
            #[cfg(feature = "gpu")]
            if let Err(err) = backend.init_gpu(
                config,
                &_display_name,
                visual,
                screen_number,
                transparent,
                deadline,
            ) {
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
        if renderer == crate::RendererType::Gpu {
            // GPU swapchain images replace the CPU pixmap. Do not retain a
            // full-size unused backing pixmap or overwrite GPU images on Expose.
            backend
                .conn
                .change_window_attributes(
                    window,
                    &ChangeWindowAttributesAux::new().background_pixel(0),
                )?
                .check()?;
            backend.conn.free_pixmap(backend.pixmap)?.check()?;
            backend.pixmap = x11rb::NONE;
            backend.conn.free_gc(backend.gc)?.check()?;
            backend.gc = x11rb::NONE;
            #[cfg(feature = "gpu")]
            {
                let mut clear: Option<crate::gpu::GpuDraw> = Some(Box::new(|_| Ok(())));
                backend.present_gpu(&mut clear, info)?;
            }
        } else {
            match backend.enable_present() {
                Ok(true) => {},
                Ok(false) => warnings.push("X11 Present unavailable; pacing uses a timer without vertical synchronization.".into()),
                Err(error) => warnings.push(format!("X11 Present initialization failed; using timer: {error}")),
            }
        }
        let frame_pacing = if renderer == crate::RendererType::Gpu {
            FramePacing::GpuFifo
        } else if backend.present.is_some() {
            FramePacing::XPresent
        } else {
            FramePacing::Timer
        };
        let transparent = transparent
            && gpu_info
                .as_ref()
                .is_none_or(|gpu| gpu.alpha_mode != "Opaque");
        Ok((
            backend,
            BackendReport {
                gpu: gpu_info,
                backend: BackendType::X11,
                capabilities: Capabilities {
                    renderer,
                    transparency: if transparent {
                        Support::Supported
                    } else {
                        Support::Unsupported
                    },
                    click_through: if shape {
                        Support::Supported
                    } else {
                        Support::Unsupported
                    },
                    keyboard_on_demand: Support::Supported,
                    stacking,
                    frame_pacing,
                    integer_scaling: false,
                    fractional_scaling: false,
                },
                fallback_attempts: Vec::new(),
                warnings,
            },
        ))
    }

    fn uses_gpu(&self) -> bool {
        #[cfg(feature = "gpu")]
        {
            self.gpu.is_some()
        }
        #[cfg(not(feature = "gpu"))]
        {
            false
        }
    }

    #[cfg(feature = "gpu")]
    fn init_gpu(
        &mut self,
        config: &OverlayBuilder,
        display_name: &str,
        visual: Visualid,
        screen: usize,
        transparent: bool,
        deadline: Instant,
    ) -> Result<()> {
        use raw_window_handle::{
            RawDisplayHandle, RawWindowHandle, XcbDisplayHandle, XcbWindowHandle,
        };
        let native = crate::gpu::xcb::NativeXcb::connect(display_name)?;
        if usize::try_from(native.screen).ok() != Some(screen) {
            return Err(OverlayError::Gpu(
                "native XCB screen differs from the window's screen".into(),
            ));
        }
        let display =
            RawDisplayHandle::Xcb(XcbDisplayHandle::new(Some(native.pointer), native.screen));
        let mut window = XcbWindowHandle::new(
            std::num::NonZeroU32::new(self.window)
                .ok_or_else(|| OverlayError::Gpu("invalid X11 window id".into()))?,
        );
        window.visual_id = std::num::NonZeroU32::new(visual);
        // SAFETY: both connections address the same captured DISPLAY/screen.
        // The Rust connection owns this window; the native connection is kept
        // in GpuSurface, which is dropped before the window is destroyed.
        self.gpu = Some(unsafe {
            crate::gpu::GpuSurface::create(
                display,
                RawWindowHandle::Xcb(window),
                Box::new(native),
                config,
                self.info.physical_size,
                transparent,
                deadline,
            )
        }?);
        Ok(())
    }

    fn enable_present(&mut self) -> Result<bool> {
        if self
            .conn
            .extension_information(present::X11_EXTENSION_NAME)?
            .is_none()
        {
            return Ok(false);
        }
        let version = self.conn.present_query_version(1, 0)?.reply()?;
        if version.major_version < 1 {
            return Ok(false);
        }
        let event = self.conn.generate_id()?;
        self.conn
            .present_select_input(
                event,
                self.window,
                present::EventMask::COMPLETE_NOTIFY | present::EventMask::IDLE_NOTIFY,
            )?
            .check()?;
        // Automatic background-pixmap restoration would bypass Present timing.
        self.conn
            .change_window_attributes(
                self.window,
                &ChangeWindowAttributesAux::new().background_pixel(0),
            )?
            .check()?;
        self.present = Some(PresentState::new(event));
        Ok(true)
    }

    fn schedule_present(&mut self) -> Result<()> {
        let state = self.present.as_mut().expect("Present negotiated");
        debug_assert!(state.ready());
        self.conn.present_pixmap(
            self.window,
            self.pixmap,
            state.serial.wrapping_add(1),
            x11rb::NONE,
            x11rb::NONE,
            0,
            0,
            x11rb::NONE,
            x11rb::NONE,
            x11rb::NONE,
            u32::from(present::Option::COPY),
            state.msc.saturating_add(1),
            0,
            0,
            &[],
        )?;
        state.submitted();
        self.repaint = false;
        Ok(())
    }

    fn copy_to_window(&self) -> Result<()> {
        if self.uses_gpu() || self.present.is_some() {
            return Ok(());
        }
        self.conn.copy_area(
            self.pixmap,
            self.window,
            self.gc,
            0,
            0,
            0,
            0,
            self.info.physical_size.width as u16,
            self.info.physical_size.height as u16,
        )?;
        Ok(())
    }
}
impl OverlayBackend for X11Backend {
    fn fd(&self) -> BorrowedFd<'_> {
        self.conn.stream().as_fd()
    }
    fn info(&self) -> WindowInfo {
        self.info
    }
    fn dispatch(&mut self) -> Result<Vec<OverlayEvent>> {
        #[cfg(feature = "gpu")]
        if let Some(gpu) = &self.gpu {
            gpu.check()?;
        }
        let mut events = Vec::new();
        for _ in 0..256 {
            let Some(event) = self.conn.poll_for_event()? else {
                break;
            };
            match event {
                Event::Error(error) => {
                    return Err(crate::error::backend_error("X11", format!("{error:?}")))
                }
                Event::Expose(e) if e.window == self.window && e.count == 0 => {
                    if self.present.is_some() {
                        self.repaint = true;
                    } else {
                        self.copy_to_window()?;
                    }
                }
                Event::PresentCompleteNotify(e)
                    if e.window == self.window && e.kind == present::CompleteKind::PIXMAP =>
                {
                    if let Some(state) = &mut self.present {
                        if state.event == e.event {
                            state.completed(e.serial, e.msc);
                        }
                    }
                }
                Event::PresentIdleNotify(e) if e.window == self.window => {
                    if let Some(state) = &mut self.present {
                        if state.event == e.event {
                            state.released(e.serial);
                        }
                    }
                }
                Event::ConfigureNotify(e) if e.window == self.window => {
                    let size = Size::new(u32::from(e.width), u32::from(e.height));
                    if size != self.info.physical_size {
                        size.byte_len(self.limit)?;
                        if !self.uses_gpu() {
                            let pixmap = self.conn.generate_id()?;
                            self.conn
                                .create_pixmap(self.depth, pixmap, self.window, e.width, e.height)?
                                .check()?;
                            self.conn
                                .poly_fill_rectangle(
                                    pixmap,
                                    self.gc,
                                    &[Rectangle {
                                        x: 0,
                                        y: 0,
                                        width: e.width,
                                        height: e.height,
                                    }],
                                )?
                                .check()?;
                            self.conn
                                .change_window_attributes(
                                    self.window,
                                    &if self.present.is_some() {
                                        ChangeWindowAttributesAux::new().background_pixel(0)
                                    } else {
                                        ChangeWindowAttributesAux::new().background_pixmap(pixmap)
                                    },
                                )?
                                .check()?;
                            self.conn.free_pixmap(self.pixmap)?;
                            self.pixmap = pixmap;
                            self.repaint = self.present.is_some();
                        }
                        self.info = WindowInfo {
                            logical_size: size,
                            physical_size: size,
                            scale_factor: crate::ScaleFactor::ONE,
                            generation: self.info.generation.wrapping_add(1),
                        };
                        self.copy_to_window()?;
                        events.push(OverlayEvent::Resized(self.info));
                    }
                }
                Event::MotionNotify(e) if e.event == self.window => {
                    events.push(OverlayEvent::PointerMoved {
                        x: f64::from(e.event_x),
                        y: f64::from(e.event_y),
                    })
                }
                Event::EnterNotify(e) if e.event == self.window => {
                    events.push(OverlayEvent::PointerEntered)
                }
                Event::LeaveNotify(e) if e.event == self.window => {
                    events.push(OverlayEvent::PointerLeft)
                }
                Event::ButtonPress(e)
                    if e.event == self.window && !matches!(e.detail, 0 | 4..=7) =>
                {
                    events.push(OverlayEvent::PointerButton {
                        button: button_code(e.detail),
                        state: ButtonState::Pressed,
                    })
                }
                Event::ButtonRelease(e)
                    if e.event == self.window && !matches!(e.detail, 0 | 4..=7) =>
                {
                    events.push(OverlayEvent::PointerButton {
                        button: button_code(e.detail),
                        state: ButtonState::Released,
                    })
                }
                Event::KeyPress(e) if e.event == self.window => events.push(OverlayEvent::Key {
                    code: u32::from(e.detail.saturating_sub(8)),
                    state: ButtonState::Pressed,
                }),
                Event::KeyRelease(e) if e.event == self.window => events.push(OverlayEvent::Key {
                    code: u32::from(e.detail.saturating_sub(8)),
                    state: ButtonState::Released,
                }),
                Event::DestroyNotify(e) if e.window == self.window => {
                    events.push(OverlayEvent::Closed)
                }
                Event::ClientMessage(e)
                    if e.window == self.window
                        && e.type_ == self.protocols_atom
                        && e.format == 32
                        && e.data.as_data32()[0] == self.delete_atom =>
                {
                    events.push(OverlayEvent::Closed)
                }
                _ => {}
            }
        }
        if self.repaint && self.present.as_ref().is_some_and(PresentState::ready) {
            self.schedule_present()?;
        }
        Ok(events)
    }
    fn present(&mut self, frame: &Frame) -> Result<bool> {
        let blocked = self
            .present
            .as_ref()
            .map(|state| !state.ready())
            .unwrap_or_else(|| {
                self.last_present
                    .is_some_and(|time| time.elapsed() < self.interval)
            });
        if blocked {
            return Ok(false);
        }
        let size = frame.size;
        let row_bytes = size.width as usize * 4;
        let max_data = self.conn.maximum_request_bytes().saturating_sub(28);
        let rows = max_data / row_bytes;
        if rows == 0 {
            return Err(OverlayError::Unsupported(
                "X11 maximum request size for this row width",
            ));
        }
        let chunk_rows = rows.min(size.height as usize);
        let mut data = Vec::new();
        data.try_reserve_exact(chunk_rows * row_bytes)
            .map_err(|_| OverlayError::Allocation {
                bytes: chunk_rows * row_bytes,
            })?;
        let little_endian = self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
        for start in (0..size.height as usize).step_by(chunk_rows) {
            let count = chunk_rows.min(size.height as usize - start);
            data.clear();
            for pixel in
                &frame.pixels[start * size.width as usize..(start + count) * size.width as usize]
            {
                data.extend_from_slice(&if little_endian {
                    pixel.to_le_bytes()
                } else {
                    pixel.to_be_bytes()
                });
            }
            self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                self.pixmap,
                self.gc,
                size.width as u16,
                count as u16,
                0,
                start as i16,
                0,
                self.depth,
                &data,
            )?;
        }
        if self.present.is_some() {
            self.schedule_present()?;
        } else {
            self.copy_to_window()?;
        }
        self.last_present = Some(Instant::now());
        Ok(true)
    }
    fn set_click_through(&mut self, enabled: bool) -> Result<()> {
        if !self.shape {
            return if enabled {
                Err(OverlayError::Unsupported("SHAPE 1.1 input regions"))
            } else {
                Ok(())
            };
        }
        if enabled {
            self.conn
                .shape_rectangles(
                    shape::SO::SET,
                    shape::SK::INPUT,
                    ClipOrdering::UNSORTED,
                    self.window,
                    0,
                    0,
                    &[],
                )?
                .check()?;
        } else {
            self.conn
                .shape_mask(
                    shape::SO::SET,
                    shape::SK::INPUT,
                    self.window,
                    0,
                    0,
                    x11rb::NONE,
                )?
                .check()?;
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        self.conn.flush()?;
        Ok(())
    }
    fn frame_wait(&self) -> Option<Duration> {
        if self.uses_gpu() {
            return Some(Duration::from_millis(2));
        }
        if let Some(state) = &self.present {
            return state.ready().then_some(Duration::ZERO);
        }
        Some(
            self.last_present
                .map(|time| self.interval.saturating_sub(time.elapsed()))
                .unwrap_or(Duration::ZERO),
        )
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
        let gpu = self
            .gpu
            .as_mut()
            .ok_or(OverlayError::Unsupported("GPU renderer"))?;
        let Some(texture) = gpu.acquire(info.physical_size)? else {
            return Ok(false);
        };
        gpu.draw(texture, draw, info)?;
        Ok(true)
    }
}
impl Drop for X11Backend {
    fn drop(&mut self) {
        #[cfg(feature = "gpu")]
        drop(self.gpu.take());
        let _ = self.conn.destroy_window(self.window);
        if self.pixmap != x11rb::NONE {
            let _ = self.conn.free_pixmap(self.pixmap);
        }
        if self.gc != x11rb::NONE {
            let _ = self.conn.free_gc(self.gc);
        }
        let _ = self.conn.free_colormap(self.colormap);
        let _ = self.conn.flush();
    }
}
fn atom(conn: &XConnection, name: &[u8]) -> Result<Atom> {
    Ok(conn.intern_atom(false, name)?.reply()?.atom)
}
fn button_code(detail: u8) -> u32 {
    match detail {
        1 => 272,
        2 => 274,
        3 => 273,
        8..=u8::MAX => 275 + u32::from(detail - 8),
        _ => u32::from(detail),
    }
}
fn monitor_geometry(
    conn: &XConnection,
    root: Window,
    width: u16,
    height: u16,
    selector: &OutputSelector,
    warnings: &mut Vec<String>,
) -> Result<(i32, i32, u32, u32)> {
    if conn
        .extension_information(randr::X11_EXTENSION_NAME)?
        .is_some()
    {
        let version = conn.randr_query_version(1, 5)?.reply()?;
        if (version.major_version, version.minor_version) >= (1, 5) {
            let monitors = conn.randr_get_monitors(root, true)?.reply()?.monitors;
            for monitor in &monitors {
                let chosen = match selector {
                    OutputSelector::Automatic => {
                        monitor.primary
                            || (!monitors.iter().any(|m| m.primary)
                                && monitors.first().is_some_and(|m| m.name == monitor.name))
                    }
                    OutputSelector::Name(name) => {
                        conn.get_atom_name(monitor.name)?.reply()?.name == name.as_bytes()
                    }
                };
                if chosen {
                    return Ok((
                        i32::from(monitor.x),
                        i32::from(monitor.y),
                        u32::from(monitor.width),
                        u32::from(monitor.height),
                    ));
                }
            }
        }
    }
    if matches!(selector, OutputSelector::Name(_)) {
        return Err(OverlayError::Unsupported("requested X11 monitor name"));
    }
    warnings.push("No RandR monitor geometry; placement uses the complete X11 screen.".into());
    Ok((0, 0, u32::from(width), u32::from(height)))
}
fn placement(
    anchor: Anchor,
    margin: u32,
    size: Size,
    monitor: (i32, i32, u32, u32),
) -> Result<(i16, i16)> {
    let (x, y, w, h) = monitor;
    let dx = i64::from(w) - i64::from(size.width);
    let dy = i64::from(h) - i64::from(size.height);
    let margin = i64::from(margin);
    let px = match anchor {
        Anchor::TopLeft | Anchor::Left | Anchor::BottomLeft => margin,
        Anchor::TopRight | Anchor::Right | Anchor::BottomRight => dx - margin,
        Anchor::Fill => margin,
        _ => dx / 2,
    };
    let py = match anchor {
        Anchor::TopLeft | Anchor::Top | Anchor::TopRight => margin,
        Anchor::BottomLeft | Anchor::Bottom | Anchor::BottomRight => dy - margin,
        Anchor::Fill => margin,
        _ => dy / 2,
    };
    Ok((
        (i64::from(x) + px)
            .try_into()
            .map_err(|_| OverlayError::Unsupported("X11 horizontal coordinate range"))?,
        (i64::from(y) + py)
            .try_into()
            .map_err(|_| OverlayError::Unsupported("X11 vertical coordinate range"))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn present_waits_for_both_notifications_and_ignores_obsolete_serials() {
        for idle_first in [true, false] {
            let mut state = PresentState::new(42);
            state.submitted();
            if idle_first {
                state.released(1);
            } else {
                state.completed(1, 100);
            }
            assert!(!state.ready());
            state.released(0);
            state.completed(0, 50);
            assert!(!state.ready());
            if idle_first {
                state.completed(1, 100);
            } else {
                state.released(1);
            }
            assert!(state.ready());
            assert_eq!(state.msc, 100);
            state.submitted();
            state.completed(1, 100);
            state.released(1);
            assert!(!state.ready());
        }
    }
    #[test]
    fn anchors_respect_negative_monitor_origins_and_margins() {
        assert_eq!(
            placement(
                Anchor::Center,
                0,
                Size::new(800, 600),
                (-1920, 0, 1920, 1080)
            )
            .unwrap(),
            (-1360, 240)
        );
        assert_eq!(
            placement(
                Anchor::BottomRight,
                10,
                Size::new(800, 600),
                (0, 0, 1920, 1080)
            )
            .unwrap(),
            (1110, 470)
        );
        assert!(placement(Anchor::Right, 0, Size::new(1, 1), (32767, 0, 100, 100)).is_err());
    }
}
