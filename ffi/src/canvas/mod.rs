//! Generic command-buffer C API. All foreign data is copied before building UI.
use super::{boundary, failure, BUSY, CLOSED, FAILED, INVALID, OK, WRONG_THREAD};
use crate::desktop::{self, Desktop, Shortcuts};
use liboverlay::{
    egui::{egui, EguiOverlay},
    Anchor, BackendPreference, BackendType, InputRegion, KeyboardMode, OutputSelector,
    OverlayBuilder, OverlayError, RendererPreference,
};
use std::{
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

pub const UNSUPPORTED: i32 = -5;
pub const CONFLICT: i32 = -6;
pub const MAX_COMMANDS: usize = 8192;
pub const MAX_WIDGETS: usize = 128;
pub const MAX_EVENTS: usize = 256;
type Result<T> = std::result::Result<T, (i32, String)>;
mod commands;
mod types;
use commands::{copy_commands, show};
use types::text;
pub use types::*;
struct Settings {
    width: u32,
    height: u32,
    backend: BackendPreference,
    software: bool,
    title: String,
    output: String,
}
pub struct OverlayCanvas {
    owner: ThreadId,
    ui: EguiOverlay,
    poisoned: bool,
    settings: Settings,
    output: String,
    input: u32,
    pid: u32,
    target: Target,
    next_query: Instant,
    desktop: Desktop,
    shortcuts: Shortcuts,
}
impl OverlayCanvas {
    fn check(&self) -> Result<()> {
        if self.owner != thread::current().id() {
            Err((WRONG_THREAD, "canvas belongs to another thread".into()))
        } else if self.poisoned {
            Err((FAILED, "canvas is poisoned; destroy it".into()))
        } else {
            Ok(())
        }
    }
    fn build(s: &Settings, output: &str, input: u32) -> Result<EguiOverlay> {
        let mut b = OverlayBuilder::new()
            .with_title(&s.title)
            .with_backend(s.backend)
            .with_renderer(RendererPreference::GpuRequired)
            .with_software_gpu(s.software)
            .with_keyboard_mode(KeyboardMode::OnDemand)
            .with_input_region(InputRegion::Empty);
        b = if s.width == 0 {
            b.with_anchor(Anchor::Fill)
        } else {
            b.with_size(s.width, s.height)
        };
        if !output.is_empty() {
            b = b.with_output(OutputSelector::Name(output.into()));
        }
        let window = b.build().map_err(failure)?;
        // Explicit release is important when opening above a pointer-locked game.
        match window.set_keyboard_capture(false) {
            Ok(()) | Err(OverlayError::Unsupported(_)) => {}
            Err(e) => return Err(failure(e)),
        }
        let ui = EguiOverlay::new(window).map_err(failure)?;
        ui.context().set_visuals(egui::Visuals::dark());
        Self::apply_input(&ui, input)?;
        Ok(ui)
    }
    fn apply_input(ui: &EguiOverlay, mode: u32) -> Result<()> {
        if mode > 2 {
            return Err((INVALID, "invalid input mode".into()));
        }
        if mode == 2 && ui.window().backend_type() != BackendType::Wayland {
            return Err((
                UNSUPPORTED,
                "modal keyboard capture requires Wayland layer-shell".into(),
            ));
        }
        ui.window()
            .set_input_region(if mode == 0 {
                InputRegion::Empty
            } else {
                InputRegion::Full
            })
            .map_err(failure)?;
        match ui.window().set_keyboard_capture(mode == 2) {
            Ok(()) | Err(OverlayError::Unsupported(_)) if mode != 2 => {}
            Ok(()) => {}
            Err(e) => return Err(failure(e)),
        }
        Ok(())
    }
    fn refresh(&mut self) -> Result<()> {
        if self.pid == 0 {
            self.target = Target {
                visible: 1,
                ..Default::default()
            };
            return Ok(());
        }
        if Instant::now() < self.next_query {
            return Ok(());
        }
        self.next_query = Instant::now() + Duration::from_millis(100);
        self.target = match self.desktop.query(self.pid, &self.settings.output) {
            Ok(t) => t,
            Err((UNSUPPORTED, _)) => Target::default(),
            Err(e) => return Err(e),
        };
        let output = self.target.output();
        if !output.is_empty() && output != self.output && self.target.visible != 0 {
            let replacement = Self::build(&self.settings, &output, self.input)?;
            self.ui.window().close().map_err(failure)?;
            self.ui = replacement;
            self.output = output;
        }
        Ok(())
    }
}
// SAFETY: the public API requires a live uniquely borrowed handle on the creator thread.
unsafe fn handle<'a>(raw: *mut OverlayCanvas) -> Result<&'a mut OverlayCanvas> {
    if raw.is_null() {
        return Err((INVALID, "null canvas".into()));
    }
    // SAFETY: validity/alignment/exclusive ownership are caller preconditions.
    let value = unsafe { &mut *raw };
    value.check()?;
    Ok(value)
}
#[unsafe(no_mangle)]
pub extern "C" fn overlay_canvas_default_options() -> Options {
    Options::default()
}
/// # Safety
/// options/out are valid aligned disjoint storage; output is writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_canvas_create(
    options: *const Options,
    out: *mut *mut OverlayCanvas,
) -> i32 {
    boundary(|| {
        if out.is_null() {
            return Err((INVALID, "null output".into()));
        }
        // SAFETY: caller guarantees writable output.
        unsafe {
            out.write(std::ptr::null_mut());
        }
        if options.is_null() {
            return Err((INVALID, "null options".into()));
        }
        // SAFETY: caller guarantees readable options.
        let o = unsafe { options.read() };
        if o.abi_version != 1
            || o.struct_size as usize != std::mem::size_of::<Options>()
            || o.flags & !2 != 0
            || o.input_mode > 2
            || (o.width == 0) != (o.height == 0)
            || o.width > 65535
            || o.height > 65535
            || o.target_pid > i32::MAX as u32
        {
            return Err((INVALID, "invalid canvas options/ABI/size".into()));
        }
        let backend = match o.backend {
            0 => BackendPreference::Auto,
            1 => BackendPreference::WaylandOnly,
            2 => BackendPreference::X11Only,
            _ => return Err((INVALID, "invalid backend".into())),
        };
        // SAFETY: strings follow options' readable-storage contract.
        let (title, output) = unsafe { (text(o.title, 4096)?, text(o.output, 127)?) };
        let settings = Settings {
            width: o.width,
            height: o.height,
            backend,
            software: o.flags & 2 != 0,
            title: if title.is_empty() {
                "liboverlay".into()
            } else {
                title
            },
            output: output.clone(),
        };
        let mut desktop = Desktop::default();
        let target = if o.target_pid == 0 {
            Target {
                visible: 1,
                ..Default::default()
            }
        } else {
            desktop.query(o.target_pid, &output).unwrap_or_default()
        };
        let selected = if output.is_empty() {
            target.output()
        } else {
            output
        };
        let ui = OverlayCanvas::build(&settings, &selected, o.input_mode)?;
        let value = Box::new(OverlayCanvas {
            owner: thread::current().id(),
            ui,
            poisoned: false,
            settings,
            output: selected,
            input: o.input_mode,
            pid: o.target_pid,
            target,
            next_query: Instant::now() + Duration::from_millis(100),
            desktop,
            shortcuts: Shortcuts::default(),
        });
        // SAFETY: transfer unique allocation to caller.
        unsafe {
            out.write(Box::into_raw(value));
        }
        Ok(OK)
    })
}
/// # Safety
/// canvas is null or a uniquely owned live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_canvas_destroy(canvas: *mut OverlayCanvas) -> i32 {
    boundary(|| {
        if canvas.is_null() {
            return Ok(OK);
        }
        // SAFETY: read only the live handle's owner; wrong-thread calls retain ownership.
        if unsafe { (*canvas).owner } != thread::current().id() {
            return Err((WRONG_THREAD, "canvas belongs to another thread".into()));
        }
        // SAFETY: caller transfers unique allocation, including poisoned handles.
        let canvas = unsafe { Box::from_raw(canvas) };
        canvas.ui.window().close().map_err(failure)?;
        Ok(OK)
    })
}
/// # Safety
/// canvas is live; out is valid writable non-overlapping storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_canvas_info(canvas: *mut OverlayCanvas, out: *mut Info) -> i32 {
    boundary(|| {
        if out.is_null() {
            return Err((INVALID, "null info".into()));
        }
        // SAFETY: valid handle and writable output are API preconditions.
        let c = unsafe { handle(canvas) }?;
        c.refresh()?;
        let w = c.ui.window().info().map_err(failure)?;
        let mut info = Info {
            backend: match c.ui.window().backend_type() {
                BackendType::Wayland => 1,
                BackendType::X11 => 2,
            },
            width: w.logical_size.width,
            height: w.logical_size.height,
            input_mode: c.input,
            keyboard_capture: if c.ui.window().backend_type() == BackendType::Wayland {
                1
            } else {
                2
            },
            global_shortcuts: if desktop::hyprland_path().is_some() {
                0
            } else {
                2
            },
            target: c.target,
            ..Default::default()
        };
        if c.shortcuts.is_active() {
            info.global_shortcuts = 1;
        }
        let n = c.output.len().min(127);
        info.output[..n].copy_from_slice(&c.output.as_bytes()[..n]);
        // SAFETY: caller guarantees writable info.
        unsafe {
            out.write(info);
        }
        Ok(if c.ui.window().is_running() {
            OK
        } else {
            CLOSED
        })
    })
}
/// # Safety
/// canvas is a live uniquely borrowed handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_canvas_set_input(canvas: *mut OverlayCanvas, mode: u32) -> i32 {
    boundary(|| {
        // SAFETY: API handle contract.
        let c = unsafe { handle(canvas) }?;
        if c.input != mode {
            OverlayCanvas::apply_input(&c.ui, mode)?;
            c.input = mode;
        }
        Ok(OK)
    })
}
/// # Safety
/// canvas is a live uniquely borrowed handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_canvas_follow(canvas: *mut OverlayCanvas, pid: u32) -> i32 {
    boundary(|| {
        if pid > i32::MAX as u32 {
            return Err((INVALID, "invalid target PID".into()));
        }
        // SAFETY: API handle contract.
        let c = unsafe { handle(canvas) }?;
        if c.pid != pid {
            c.pid = pid;
            c.next_query = Instant::now();
        }
        c.refresh()?;
        Ok(OK)
    })
}
/// # Safety
/// All input/output storage is valid, aligned and non-overlapping for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_canvas_submit(
    canvas: *mut OverlayCanvas,
    frame: *const Frame,
    events: *mut Event,
    capacity: usize,
    count: *mut usize,
) -> i32 {
    boundary(|| {
        if count.is_null() {
            return Err((INVALID, "null event count".into()));
        }
        // SAFETY: writable count is an API precondition.
        unsafe {
            count.write(0);
        }
        if frame.is_null() || events.is_null() || capacity < MAX_EVENTS {
            return Err((
                INVALID,
                "frame/events invalid; reserve OVERLAY_MAX_EVENTS".into(),
            ));
        }
        // SAFETY: valid readable frame/command buffers and live unique canvas.
        let commands = unsafe { copy_commands(frame.read()) }?;
        // SAFETY: handle contract.
        let c = unsafe { handle(canvas) }?;
        if !c.ui.window().is_running() {
            return Ok(CLOSED);
        }
        let mut result = c.shortcuts.poll();
        c.poisoned = true;
        let drawn = c.ui.run(|ctx| show(ctx, &commands, &mut result));
        c.poisoned = false;
        debug_assert!(result.len() <= MAX_EVENTS);
        // SAFETY: output has at least MAX_EVENTS entries, count is disjoint/writable.
        unsafe {
            std::ptr::copy_nonoverlapping(result.as_ptr(), events, result.len());
            count.write(result.len());
        }
        match drawn {
            Ok(_) => Ok(OK),
            Err(OverlayError::Closed) => Ok(CLOSED),
            Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. }) => Ok(BUSY),
            Err(e) => Err(failure(e)),
        }
    })
}
/// # Safety
/// canvas/key follow the handle and readable-string contracts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_canvas_shortcut(
    canvas: *mut OverlayCanvas,
    key: Text,
    modifiers: u32,
    id: u32,
) -> i32 {
    boundary(|| {
        // SAFETY: caller provides live handle and readable key bytes.
        let (c, key) = unsafe { (handle(canvas)?, text(key, 64)?) };
        c.shortcuts.register(&key, modifiers, id)?;
        Ok(OK)
    })
}
/// # Safety
/// canvas is live and uniquely borrowed on its creator thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_canvas_unbind(canvas: *mut OverlayCanvas, id: u32) -> i32 {
    boundary(|| {
        // SAFETY: handle contract.
        unsafe { handle(canvas) }?.shortcuts.remove(id)?;
        Ok(OK)
    })
}
/// # Safety
/// output is writable; filter is readable; storage does not overlap.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_target_query(pid: u32, filter: Text, out: *mut Target) -> i32 {
    boundary(|| {
        if out.is_null() || pid == 0 || pid > i32::MAX as u32 {
            return Err((INVALID, "invalid target query arguments".into()));
        }
        // SAFETY: caller guarantees output and string storage.
        unsafe {
            out.write(Target::default());
        }
        // SAFETY: caller guarantees readable filter bytes.
        let filter = unsafe { text(filter, 127) }?;
        let target = Desktop::default().query(pid, &filter)?;
        // SAFETY: caller guarantees writable output.
        unsafe {
            out.write(target);
        }
        Ok(OK)
    })
}
/// # Safety
/// out points to writable u32 storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_active_pid(out: *mut u32) -> i32 {
    boundary(|| {
        if out.is_null() {
            return Err((INVALID, "null active PID output".into()));
        }
        // SAFETY: caller guarantees writable output.
        unsafe {
            out.write(0);
        }
        let pid = desktop::active_pid()?;
        // SAFETY: caller guarantees writable output.
        unsafe {
            out.write(pid);
        }
        Ok(OK)
    })
}
