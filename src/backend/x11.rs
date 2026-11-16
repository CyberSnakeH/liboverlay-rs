use super::OverlayBackend;
use crate::{
    Anchor, BackendReport, BackendType, ButtonState, Capabilities, Frame, FramePacing,
    KeyboardMode, OutputSelector, OverlayBuilder, OverlayError, OverlayEvent, Requirement, Result,
    Size, StackingPolicy, StackingSupport, Support, WindowInfo,
};
use std::{
    os::fd::{AsFd, BorrowedFd},
    rc::Rc,
    time::{Duration, Instant},
};
use x11rb::{
    connection::{Connection, RequestConnection},
    properties::{WmHints, WmSizeHints, WmSizeHintsSpecification},
    protocol::{
        present::{self, ConnectionExt as _},
        randr::{self, ConnectionExt as _},
        render::{self, ConnectionExt as _},
        shape::{self, ConnectionExt as _},
        xproto::*,
        Event,
    },
    wrapper::ConnectionExt as _,
};

mod connection;
mod geometry;
mod ime;
mod keyboard;
mod pacing;
mod shm;
mod touch;
use connection::{connect, XConnection};
use geometry::{anchored_size, monitor_geometry, placement};
use pacing::PresentState;

pub(crate) struct X11Backend {
    #[cfg(feature = "gpu")]
    gpu: Option<crate::gpu::GpuSurface>,
    conn: Rc<XConnection>,
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
    user_time_atom: Option<Atom>,
    last_present: Option<Instant>,
    feedback_enabled: bool,
    next_submission: u64,
    pending_feedback: Option<(u32, u64, u64)>,
    damage: crate::damage::Tracker,
    interval: Duration,
    present: Option<PresentState>,
    repaint: bool,
    root: Window,
    output: OutputSelector,
    anchor: Anchor,
    margin: u32,
    requested_size: Size,
    dispatch_again: bool,
    upload: Option<shm::Upload>,
    input_region: crate::InputRegion,
    touch: Option<touch::TouchInput>,
    pending_input: Vec<OverlayEvent>,
    keyboard: Option<keyboard::KeyboardInput>,
    text_input: crate::TextInputState,
    ime: Option<ime::Ime>,
    retired_ime_windows: std::collections::BTreeSet<u32>,
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
        let conn = Rc::new(conn);
        let screen = &conn.setup().roots[screen_number];
        let root = screen.root;
        // RandR monitor definitions also change through core root ConfigureNotify
        // events, including changes that leave the overall screen size intact.
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
        )?
        .check()?;
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
        let (mx, my, mw, mh) = monitor_geometry(&conn, root, &config.output, &mut warnings)?
            .ok_or(OverlayError::Unsupported("requested X11 monitor name"))?;
        if conn
            .extension_information(randr::X11_EXTENSION_NAME)?
            .is_some()
        {
            let version = conn.randr_query_version(1, 5)?.reply()?;
            let mut mask = randr::NotifyMask::SCREEN_CHANGE;
            if (version.major_version, version.minor_version) >= (1, 5) {
                mask |= randr::NotifyMask::CRTC_CHANGE
                    | randr::NotifyMask::OUTPUT_CHANGE
                    | randr::NotifyMask::RESOURCE_CHANGE;
            }
            conn.randr_select_input(root, mask)?.check()?;
        }
        let size = anchored_size(config.anchor, config.margin, config.size, (mx, my, mw, mh))?;
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
                        | EventMask::KEY_RELEASE
                        | EventMask::FOCUS_CHANGE,
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
        let mut user_time_atom = None;
        if managed {
            // A keyboard-capable overlay may receive focus after user interaction,
            // but mapping it must not activate it over the current application.
            let user_time = atom(&conn, b"_NET_WM_USER_TIME")?;
            user_time_atom = Some(user_time);
            conn.change_property32(
                PropMode::REPLACE,
                window,
                user_time,
                AtomEnum::CARDINAL,
                &[0],
            )?
            .check()?;
            let motif = atom(&conn, b"_MOTIF_WM_HINTS")?;
            // MWM_HINTS_DECORATIONS; an empty decorations field requests no frame.
            conn.change_property32(PropMode::REPLACE, window, motif, motif, &[2, 0, 0, 0, 0])?
                .check()?;
            let mut size_hints = WmSizeHints::new();
            size_hints.position = Some((
                WmSizeHintsSpecification::ProgramSpecified,
                x.into(),
                y.into(),
            ));
            size_hints.size = Some((
                WmSizeHintsSpecification::ProgramSpecified,
                size.width as i32,
                size.height as i32,
            ));
            size_hints.set_normal_hints(&conn, window)?.check()?;
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
        let touch = touch::TouchInput::new(&conn, root, window)?;
        let keyboard = if config.keyboard == KeyboardMode::OnDemand {
            match keyboard::KeyboardInput::new(&conn) {
                Ok(keyboard) => Some(keyboard),
                Err(error) if !config.text_input.enabled => {
                    warnings.push(format!(
                        "XKB unavailable; raw keyboard events only: {error}"
                    ));
                    None
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let ime = if let Some(keyboard) = &keyboard {
            match ime::Ime::new(
                conn.clone(),
                screen_number,
                window,
                keyboard.keyboard.source,
                config.text_input.clone(),
            ) {
                Ok(ime) => ime,
                Err(error) => {
                    warnings.push(format!("XIM unavailable; using local composition: {error}"));
                    None
                }
            }
        } else {
            None
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
            user_time_atom,
            last_present: None,
            feedback_enabled: config.presentation_feedback,
            next_submission: 0,
            pending_feedback: None,
            damage: crate::damage::Tracker::new(config.cpu_damage_tracking),
            interval: config.frame_interval,
            present: None,
            repaint: false,
            root,
            output: config.output.clone(),
            anchor: config.anchor,
            margin: config.margin,
            requested_size: config.size,
            dispatch_again: false,
            upload: None,
            input_region: config.input_region.clone(),
            touch,
            pending_input: Vec::new(),
            keyboard,
            text_input: config.text_input.clone(),
            ime,
            retired_ime_windows: std::collections::BTreeSet::new(),
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
        backend.conn.stream().finish_startup();
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
            match shm::Upload::supported(&backend.conn).and_then(|supported| {
                if supported {
                    shm::Upload::create(&backend.conn, size, backend.limit).map(Some)
                } else {
                    Ok(None)
                }
            }) {
                Ok(upload) => backend.upload = upload,
                Err(error) => warnings.push(format!(
                    "MIT-SHM initialization failed; using PutImage: {error}"
                )),
            }
            if backend.upload.is_none() {
                warnings.push("MIT-SHM FD transport unavailable; CPU uploads use PutImage.".into());
            }
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
        let translated = if backend.keyboard.is_some() {
            Support::Supported
        } else {
            Support::Unsupported
        };
        let input_method = if backend.ime.is_some() {
            Support::Supported
        } else {
            Support::Unsupported
        };
        let composed = if backend.ime.is_some()
            || backend
                .keyboard
                .as_ref()
                .is_some_and(|k| k.keyboard.has_compose())
        {
            Support::Supported
        } else {
            Support::Unsupported
        };
        let touch = if backend.touch.is_some() {
            Support::Supported
        } else {
            Support::Unsupported
        };
        if config.text_input.enabled && backend.ime.is_none() {
            warnings.push("No XIM server was found; text input uses local XCompose tables.".into());
        }
        if backend.keyboard.is_some() && composed == Support::Unsupported {
            warnings.push(
                "XCompose tables are unavailable; only individual-key text is available.".into(),
            );
        }
        let presentation_feedback =
            if renderer == crate::RendererType::Cpu && backend.present.is_some() {
                Support::Supported
            } else {
                Support::Unsupported
            };
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
                    input_regions: if shape {
                        Support::Supported
                    } else {
                        Support::Unsupported
                    },
                    keyboard_translation: translated,
                    text_composition: composed,
                    input_method,
                    surrounding_text: Support::Unsupported,
                    touch,
                    stacking,
                    frame_pacing,
                    presentation_feedback,
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

    fn relocate(&self) -> Result<bool> {
        let Some(monitor) = monitor_geometry(&self.conn, self.root, &self.output, &mut Vec::new())?
        else {
            // A named output is a binding, not a preference for another monitor.
            return Ok(false);
        };
        let size = anchored_size(self.anchor, self.margin, self.requested_size, monitor)?;
        size.byte_len(self.limit)?;
        let (x, y) = placement(self.anchor, self.margin, size, monitor)?;
        self.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new()
                .x(i32::from(x))
                .y(i32::from(y))
                .width(size.width)
                .height(size.height),
        )?;
        Ok(true)
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
        let mut events = std::mem::take(&mut self.pending_input);
        if self.ime.as_ref().is_some_and(ime::Ime::expired) {
            self.ime_failed(
                "XIM did not acknowledge its request within five seconds",
                &mut events,
            );
        }
        let mut topology_changed = false;
        self.dispatch_again = true;
        for _ in 0..256 {
            let Some(event) = self.conn.poll_for_event()? else {
                self.dispatch_again = false;
                break;
            };
            if let Event::Error(error) = &event {
                if error.error_kind == x11rb::protocol::ErrorKind::Window
                    && self.retired_ime_windows.contains(&error.bad_value)
                {
                    continue;
                }
            }
            if let Some(touch) = &mut self.touch {
                events.extend(touch.event(&event));
            }
            if let Some(keyboard) = &mut self.keyboard {
                let input_events = keyboard.event(&self.conn, &event)?;
                if !input_events.is_empty()
                    && input_events
                        .iter()
                        .any(|e| matches!(e, OverlayEvent::InputReset { .. }))
                {
                    if let Some(ime) = &mut self.ime {
                        ime.set_source(keyboard.keyboard.source);
                        if let Err(error) = ime.reset() {
                            self.ime_failed(error, &mut events);
                        }
                    }
                }
                events.extend(input_events);
            }
            if let Some(ime) = &mut self.ime {
                match ime.filter(&event) {
                    Ok(true) => {
                        self.drain_ime(&mut events);
                        continue;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        self.ime_failed(error, &mut events);
                        continue;
                    }
                }
            }
            self.drain_ime(&mut events);
            let activity = match &event {
                Event::ButtonPress(e) if e.event == self.window && e.response_type & 0x80 == 0 => {
                    Some(e.time)
                }
                Event::KeyPress(e) if e.event == self.window && e.response_type & 0x80 == 0 => {
                    Some(e.time)
                }
                _ => None,
            };
            if let (Some(atom), Some(time)) = (self.user_time_atom, activity) {
                self.conn.change_property32(
                    PropMode::REPLACE,
                    self.window,
                    atom,
                    AtomEnum::CARDINAL,
                    &[time],
                )?;
            }
            match event {
                Event::ConfigureNotify(e) if e.window == self.root => {
                    topology_changed = true;
                }
                Event::ShmCompletion(e) => {
                    if let Some(upload) = &mut self.upload {
                        upload.completed(e.shmseg);
                    }
                }
                Event::RandrNotify(_) | Event::RandrScreenChangeNotify(_) => {
                    topology_changed = true;
                }
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
                            if let Some((serial, submission, generation)) = self.pending_feedback {
                                if serial == e.serial {
                                    self.pending_feedback = None;
                                    if e.mode == present::CompleteMode::SKIP {
                                        events.push(OverlayEvent::PresentationDiscarded {
                                            submission,
                                            generation,
                                        });
                                    } else {
                                        events.push(OverlayEvent::FramePresented(
                                            crate::PresentationFeedback {
                                                submission,
                                                generation,
                                                source: crate::PresentationSource::X11,
                                                timestamp: Duration::from_micros(e.ust),
                                                clock_id: None,
                                                refresh_interval: None,
                                                sequence: e.msc,
                                                vsync: false,
                                                hardware_clock: false,
                                                hardware_completion: false,
                                                zero_copy: false,
                                            },
                                        ));
                                    }
                                }
                            }
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
                Event::ButtonPress(e) if e.event == self.window && matches!(e.detail, 4..=7) => {
                    let (axis, value) = match e.detail {
                        4 => (crate::ScrollAxis::Vertical, -1.0),
                        5 => (crate::ScrollAxis::Vertical, 1.0),
                        6 => (crate::ScrollAxis::Horizontal, -1.0),
                        _ => (crate::ScrollAxis::Horizontal, 1.0),
                    };
                    events.push(OverlayEvent::PointerAxis {
                        axis,
                        value,
                        unit: crate::ScrollUnit::Steps,
                    });
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
                Event::KeyPress(e) if e.event == self.window => {
                    self.handle_key(e, ButtonState::Pressed, &mut events);
                }
                Event::KeyRelease(e) if e.event == self.window => {
                    self.handle_key(e, ButtonState::Released, &mut events);
                }
                Event::FocusIn(e)
                    if e.event == self.window
                        && e.mode != NotifyMode::GRAB
                        && e.mode != NotifyMode::UNGRAB
                        && e.detail != NotifyDetail::INFERIOR =>
                {
                    if let Some(keyboard) = &mut self.keyboard {
                        events.extend(keyboard.keyboard.focus(true));
                    }
                    if let Some(ime) = &mut self.ime {
                        if let Err(error) = ime.focus(true) {
                            self.ime_failed(error, &mut events);
                        }
                    }
                    self.drain_ime(&mut events);
                }
                Event::FocusOut(e)
                    if e.event == self.window
                        && e.mode != NotifyMode::GRAB
                        && e.mode != NotifyMode::UNGRAB
                        && e.detail != NotifyDetail::INFERIOR =>
                {
                    if let Some(keyboard) = &mut self.keyboard {
                        events.extend(keyboard.keyboard.focus(false));
                    }
                    if let Some(ime) = &mut self.ime {
                        if let Err(error) = ime.focus(false) {
                            self.ime_failed(error, &mut events);
                        }
                    }
                    self.drain_ime(&mut events);
                }
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
        if topology_changed {
            if !self.relocate()? {
                events.push(OverlayEvent::Closed);
            }
            // Monitor queries can move events from the socket into x11rb's queue.
            self.dispatch_again = true;
        }
        if self.repaint && self.present.as_ref().is_some_and(PresentState::ready) {
            self.schedule_present()?;
        }
        Ok(events)
    }
    fn present(&mut self, frame: &Frame) -> Result<bool> {
        if self.upload.as_ref().is_some_and(shm::Upload::busy) {
            return Ok(false);
        }
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
        if self
            .upload
            .as_ref()
            .is_some_and(|upload| upload.size() != size)
        {
            if let Some(upload) = self.upload.take() {
                upload.detach(&self.conn)?;
            }
            self.upload = Some(shm::Upload::create(&self.conn, size, self.limit)?);
            self.dispatch_again = true;
        }
        if let Some(rectangle) = self.damage.update(frame)? {
            if let Some(upload) = &mut self.upload {
                upload.put(
                    &self.conn,
                    self.pixmap,
                    self.gc,
                    self.depth,
                    frame,
                    rectangle,
                )?;
            } else {
                let row_bytes = rectangle.width as usize * 4;
                let max_data = self.conn.maximum_request_bytes().saturating_sub(28);
                let rows = max_data / row_bytes;
                if rows == 0 {
                    return Err(OverlayError::Unsupported(
                        "X11 maximum request size for this row width",
                    ));
                }
                let chunk_rows = rows.min(rectangle.height as usize);
                let mut data = Vec::new();
                data.try_reserve_exact(chunk_rows * row_bytes)
                    .map_err(|_| OverlayError::Allocation {
                        bytes: chunk_rows * row_bytes,
                    })?;
                let little = self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
                for start in (0..rectangle.height as usize).step_by(chunk_rows) {
                    let count = chunk_rows.min(rectangle.height as usize - start);
                    data.clear();
                    for y in rectangle.y as usize + start..rectangle.y as usize + start + count {
                        let first = y * size.width as usize + rectangle.x as usize;
                        for pixel in &frame.pixels[first..first + rectangle.width as usize] {
                            data.extend_from_slice(&if little {
                                pixel.to_le_bytes()
                            } else {
                                pixel.to_be_bytes()
                            });
                        }
                    }
                    self.conn.put_image(
                        ImageFormat::Z_PIXMAP,
                        self.pixmap,
                        self.gc,
                        rectangle.width as u16,
                        count as u16,
                        rectangle.x as i16,
                        (rectangle.y as usize + start) as i16,
                        0,
                        self.depth,
                        &data,
                    )?;
                }
            }
        }
        if self.present.is_some() {
            self.schedule_present()?;
            if self.feedback_enabled {
                self.next_submission = self.next_submission.wrapping_add(1);
                self.pending_feedback = Some((
                    self.present.as_ref().expect("Present negotiated").serial,
                    self.next_submission,
                    frame.generation,
                ));
            }
        } else {
            self.copy_to_window()?;
        }
        self.last_present = Some(Instant::now());
        Ok(true)
    }
    fn set_click_through(&mut self, enabled: bool) -> Result<()> {
        self.apply_region(if enabled {
            &crate::InputRegion::Empty
        } else {
            &self.input_region
        })
    }
    fn reset_input(&mut self) -> Result<()> {
        if let Some(touch) = &mut self.touch {
            self.pending_input.extend(touch.reset());
        }
        if let Some(keyboard) = &mut self.keyboard {
            keyboard.keyboard.reset();
        }
        if let Some(ime) = &mut self.ime {
            if let Err(error) = ime.reset() {
                let mut events = Vec::new();
                self.ime_failed(error, &mut events);
                self.pending_input.extend(events);
            }
        }
        Ok(())
    }
    fn set_text_input(&mut self, text: crate::TextInputState) -> Result<()> {
        text.validate()?;
        let keyboard = self
            .keyboard
            .as_mut()
            .ok_or(OverlayError::Unsupported("XKB text translation"))?;
        if !text.enabled || text.change_cause == crate::TextChangeCause::Other {
            keyboard.keyboard.reset_compose();
            self.pending_input.push(OverlayEvent::TextPreedit {
                source: keyboard.keyboard.source,
                text: String::new(),
                cursor: None,
            });
        }
        self.text_input = text.clone();
        if let Some(ime) = &mut self.ime {
            if let Err(error) = ime.configure(text) {
                let mut events = Vec::new();
                self.ime_failed(error, &mut events);
                self.pending_input.extend(events);
            }
        }
        Ok(())
    }
    fn set_input_region(&mut self, region: crate::InputRegion) -> Result<()> {
        region.validate()?;
        self.apply_region(&region)?;
        self.input_region = region;
        Ok(())
    }
    fn flush(&mut self) -> Result<()> {
        self.conn.flush()?;
        Ok(())
    }
    fn needs_dispatch(&self) -> bool {
        self.dispatch_again
    }
    fn idle_wait(&self) -> Option<Duration> {
        self.uses_gpu()
            .then_some(Duration::from_millis(250))
            .into_iter()
            .chain(self.ime.as_ref().and_then(ime::Ime::wait))
            .min()
    }
    fn frame_wait(&self) -> Option<Duration> {
        if self.upload.as_ref().is_some_and(shm::Upload::busy) {
            return None;
        }
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
impl X11Backend {
    fn ime_failed(&mut self, error: impl std::fmt::Display, events: &mut Vec<OverlayEvent>) {
        if let Some(ime) = self.ime.take() {
            self.retired_ime_windows.extend(ime.windows());
        }
        if let Some(keyboard) = &mut self.keyboard {
            keyboard.keyboard.reset_compose();
            let source = keyboard.keyboard.source;
            events.push(OverlayEvent::TextPreedit {
                source,
                text: String::new(),
                cursor: None,
            });
            events.push(OverlayEvent::InputMethod {
                source,
                active: false,
            });
            events.push(OverlayEvent::InputMethodError {
                source,
                message: format!("{error}").chars().take(2048).collect(),
            });
            events.push(OverlayEvent::InputReset {
                source: Some(source),
                scope: crate::InputResetScope::Keyboard,
                reason: crate::InputResetReason::Cancelled,
            });
        }
    }
    fn drain_ime(&mut self, events: &mut Vec<OverlayEvent>) {
        let Some(ime) = &mut self.ime else {
            return;
        };
        let ime_events = ime.events();
        if ime_events
            .iter()
            .any(|e| matches!(e, OverlayEvent::InputMethod { .. }))
        {
            if let Some(keyboard) = &mut self.keyboard {
                keyboard.keyboard.reset_compose();
            }
        }
        events.extend(ime_events);
        for translated in ime.unhandled.drain(..) {
            if let Some(keyboard) = &mut self.keyboard {
                if keyboard.keyboard.focused {
                    let composed = if self.text_input.enabled {
                        keyboard.keyboard.compose_input(&translated)
                    } else {
                        Vec::new()
                    };
                    events.push(OverlayEvent::KeyboardInput(translated));
                    events.extend(composed);
                }
            }
        }
    }
    fn handle_key(
        &mut self,
        event: KeyPressEvent,
        state: ButtonState,
        events: &mut Vec<OverlayEvent>,
    ) {
        let Some(keyboard) = &mut self.keyboard else {
            events.push(OverlayEvent::Key {
                code: u32::from(event.detail.saturating_sub(8)),
                state,
            });
            return;
        };
        keyboard.core_modifiers(u16::from(event.state));
        let mut key_events = keyboard.keyboard.key(
            u32::from(event.detail.saturating_sub(8)),
            state,
            false,
            self.text_input.enabled,
            false,
        );
        let translated = key_events.iter().find_map(|e| {
            if let OverlayEvent::KeyboardInput(input) = e {
                Some(input.clone())
            } else {
                None
            }
        });
        let mut forwarded = false;
        if let Some(ime) = &mut self.ime {
            match ime.forward(event, translated.clone()) {
                Ok(result) => forwarded = result,
                Err(error) => self.ime_failed(error, events),
            }
        }
        if forwarded {
            key_events.retain(|e| !matches!(e, OverlayEvent::KeyboardInput(_)));
        }
        events.extend(key_events);
        if !forwarded && self.text_input.enabled {
            if let (Some(keyboard), Some(translated)) = (&mut self.keyboard, translated) {
                events.extend(keyboard.keyboard.compose_input(&translated));
            }
        }
    }
    fn apply_region(&self, region: &crate::InputRegion) -> Result<()> {
        if !self.shape {
            return if !matches!(region, crate::InputRegion::Full) {
                Err(OverlayError::Unsupported("SHAPE 1.1 input regions"))
            } else {
                Ok(())
            };
        }
        if !matches!(region, crate::InputRegion::Full) {
            let rectangles: Vec<_> = match region {
                crate::InputRegion::Rectangles(rects) => rects
                    .iter()
                    .map(|r| Rectangle {
                        x: r.x,
                        y: r.y,
                        width: r.width,
                        height: r.height,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            self.conn
                .shape_rectangles(
                    shape::SO::SET,
                    shape::SK::INPUT,
                    ClipOrdering::UNSORTED,
                    self.window,
                    0,
                    0,
                    &rectangles,
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
}
impl Drop for X11Backend {
    fn drop(&mut self) {
        #[cfg(feature = "gpu")]
        drop(self.gpu.take());
        drop(self.ime.take());
        if let Some(upload) = self.upload.take() {
            let _ = upload.detach(&self.conn);
        }
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
