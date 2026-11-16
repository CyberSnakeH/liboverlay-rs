//! Generic C/C++ canvas API and the compatibility CPU dashboard ABI.
//! No foreign pointer or string is retained after a call. See include/liboverlay.h.
mod canvas;
mod dashboard;
mod desktop;

use liboverlay::{
    egui::EguiOverlay, BackendPreference, KeyboardMode, OverlayBuilder, OverlayError,
    RendererPreference,
};
use std::{
    cell::RefCell,
    ffi::c_char,
    panic::{catch_unwind, AssertUnwindSafe},
    thread::{self, ThreadId},
};

const ABI_VERSION: u32 = 1;
const OK: i32 = 0;
const CLOSED: i32 = 1;
const INVALID: i32 = -1;
const FAILED: i32 = -2;
const PANIC: i32 = -3;
const WRONG_THREAD: i32 = -4;
const BUSY: i32 = 2;

thread_local! { static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) }; }

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OverlayConfig {
    pub abi_version: u32,
    pub width: u32,
    pub height: u32,
    pub backend: u32,
    pub flags: u32,
}
impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            abi_version: ABI_VERSION,
            width: 420,
            height: 220,
            backend: 0,
            flags: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct OverlayState {
    pub cpu_percent: f32,
    pub alert_threshold: f32,
    pub alerts_enabled: u32,
    pub alert_active: u32,
}
impl OverlayState {
    fn validate(self) -> std::result::Result<(), (i32, String)> {
        if !self.cpu_percent.is_finite()
            || !(0.0..=100.0).contains(&self.cpu_percent)
            || !self.alert_threshold.is_finite()
            || !(0.0..=100.0).contains(&self.alert_threshold)
            || self.alerts_enabled > 1
            || self.alert_active > 1
        {
            return Err((
                INVALID,
                "state requires finite percentages in 0..100 and boolean flags in 0..1".into(),
            ));
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct OverlayActions {
    pub settings_changed: u32,
    pub alert_threshold: f32,
    pub alerts_enabled: u32,
    pub close_requested: u32,
}

pub struct Overlay {
    owner: ThreadId,
    ui: EguiOverlay,
    poisoned: bool,
}

fn boundary(f: impl FnOnce() -> std::result::Result<i32, (i32, String)>) -> i32 {
    LAST_ERROR.with(|error| error.borrow_mut().clear());
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(code)) => code,
        Ok(Err((code, message))) => {
            LAST_ERROR.with(|error| *error.borrow_mut() = message);
            code
        }
        Err(_) => {
            LAST_ERROR.with(|error| {
                *error.borrow_mut() = "Rust operation panicked; destroy this handle".into()
            });
            PANIC
        }
    }
}
fn failure(error: OverlayError) -> (i32, String) {
    (FAILED, error.to_string())
}

#[unsafe(no_mangle)]
pub extern "C" fn overlay_abi_version() -> u32 {
    ABI_VERSION
}

/// Returns defaults by value; no allocation or display connection.
#[unsafe(no_mangle)]
pub extern "C" fn overlay_default_config() -> OverlayConfig {
    OverlayConfig::default()
}

/// # Safety
/// config/out must be valid, aligned, non-overlapping pointers; out must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_create(
    config: *const OverlayConfig,
    out: *mut *mut Overlay,
) -> i32 {
    boundary(|| {
        if out.is_null() {
            return Err((INVALID, "null output handle".into()));
        }
        // SAFETY: caller provides writable output storage; no allocation is published on error.
        unsafe {
            out.write(std::ptr::null_mut());
        }
        if config.is_null() {
            return Err((INVALID, "null configuration".into()));
        }
        // SAFETY: caller guarantees a readable, aligned configuration for this call.
        let config = unsafe { config.read() };
        if config.abi_version != ABI_VERSION || config.flags & !3 != 0 {
            return Err((
                INVALID,
                "unsupported ABI version or configuration flags".into(),
            ));
        }
        let backend = match config.backend {
            0 => BackendPreference::Auto,
            1 => BackendPreference::WaylandOnly,
            2 => BackendPreference::X11Only,
            _ => return Err((INVALID, "backend must be auto=0, Wayland=1 or X11=2".into())),
        };
        let window = OverlayBuilder::new()
            .with_title("liboverlay egui dashboard")
            .with_size(config.width, config.height)
            .with_backend(backend)
            .with_renderer(RendererPreference::GpuRequired)
            .with_software_gpu(config.flags & 2 != 0)
            .with_click_through(config.flags & 1 != 0)
            .with_keyboard_mode(KeyboardMode::OnDemand)
            .build()
            .map_err(failure)?;
        let ui = EguiOverlay::new(window).map_err(failure)?;
        let handle = Box::new(Overlay {
            owner: thread::current().id(),
            ui,
            poisoned: false,
        });
        // SAFETY: out was validated above and ownership transfers to overlay_destroy.
        unsafe {
            out.write(Box::into_raw(handle));
        }
        Ok(OK)
    })
}

/// # Safety
/// handle must be a live owned handle. state/actions must be valid aligned,
/// non-overlapping pointers. All handle operations must be serialized on its creator thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_tick(
    handle: *mut Overlay,
    state: *const OverlayState,
    actions: *mut OverlayActions,
) -> i32 {
    boundary(|| {
        if handle.is_null() || state.is_null() || actions.is_null() {
            return Err((INVALID, "null handle, state or actions".into()));
        }
        // SAFETY: caller guarantees live, aligned storage and no overlapping borrows.
        let (handle, state) = unsafe { (&mut *handle, state.read()) };
        // SAFETY: caller provides writable actions, distinct from handle and state.
        unsafe {
            actions.write(OverlayActions::default());
        }
        if handle.owner != thread::current().id() {
            return Err((WRONG_THREAD, "handle belongs to another thread".into()));
        }
        if handle.poisoned {
            return Err((FAILED, "handle is poisoned; destroy it".into()));
        }
        state.validate()?;
        if !handle.ui.window().is_running() {
            return Ok(CLOSED);
        }
        let mut result = OverlayActions::default();
        // egui may run multiple layout passes. Retain actions from every pass.
        // A panic poisons the handle even though it is contained at the C boundary.
        handle.poisoned = true;
        let drawn = handle.ui.run(|ctx| {
            let pass = dashboard::show(ctx, state);
            if pass.settings_changed != 0 {
                result = pass;
            }
            result.close_requested |= pass.close_requested;
        });
        handle.poisoned = false;
        // SAFETY: actions remains writable for the duration of the call. Actions
        // survive transient draw errors so the caller can apply each edit once.
        unsafe {
            actions.write(result);
        }
        match drawn {
            Ok(_) if result.close_requested != 0 => {
                handle.ui.window().close().map_err(failure)?;
                Ok(CLOSED)
            }
            Ok(_) => Ok(OK),
            Err(OverlayError::Closed) => Ok(CLOSED),
            Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. }) => Ok(BUSY),
            Err(error) => Err(failure(error)),
        }
    })
}

/// # Safety
/// handle is null or live, uniquely owned, and passed on its creator thread.
/// After a creator-thread call the handle is consumed, even if closing fails.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_destroy(handle: *mut Overlay) -> i32 {
    boundary(|| {
        if handle.is_null() {
            return Ok(OK);
        }
        // SAFETY: caller guarantees a live pointer; this read does not transfer ownership.
        if unsafe { (*handle).owner } != thread::current().id() {
            return Err((
                WRONG_THREAD,
                "handle belongs to another thread; it was not destroyed".into(),
            ));
        }
        // SAFETY: caller transfers the unique allocation created by overlay_create.
        let handle = unsafe { Box::from_raw(handle) };
        handle.ui.window().close().map_err(failure)?;
        Ok(OK)
    })
}

/// Copies this thread's last error, truncating to capacity-1 and NUL terminating.
/// Returns required bytes including NUL. Does not clear the error.
/// # Safety
/// buffer may be null only when capacity is zero; otherwise capacity bytes are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn overlay_last_error(buffer: *mut c_char, capacity: usize) -> usize {
    LAST_ERROR.with(|error| {
        let error = error.borrow();
        if capacity > 0 && !buffer.is_null() {
            let count = error.len().min(capacity - 1);
            // SAFETY: caller guarantees capacity writable bytes; string is not retained.
            unsafe {
                std::ptr::copy_nonoverlapping(error.as_ptr(), buffer.cast::<u8>(), count);
                buffer.add(count).write(0);
            }
        }
        error.len() + 1
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abi_layout_and_validation() {
        assert_eq!(std::mem::size_of::<OverlayConfig>(), 20);
        assert_eq!(std::mem::size_of::<OverlayState>(), 16);
        assert_eq!(std::mem::size_of::<OverlayActions>(), 16);
        assert!(OverlayState {
            cpu_percent: f32::NAN,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(OverlayState {
            alerts_enabled: 2,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
    #[test]
    fn invalid_calls_return_errors_without_unwinding() {
        let mut out = std::ptr::null_mut();
        // SAFETY: out is writable; a null config is explicitly rejected.
        let status = unsafe { overlay_create(std::ptr::null(), &mut out) };
        assert_eq!(status, INVALID);
        assert!(out.is_null());
        let mut message = [0i8; 5];
        // SAFETY: the destination has exactly five writable bytes.
        let size = unsafe { overlay_last_error(message.as_mut_ptr(), message.len()) };
        assert!(size > message.len());
        assert_eq!(message[4], 0);
        // SAFETY: null destruction is explicitly allowed.
        assert_eq!(unsafe { overlay_destroy(std::ptr::null_mut()) }, OK);
        assert_eq!(boundary(|| panic!("boundary test")), PANIC);
    }
    #[test]
    fn ui_builds_without_a_display() {
        let context = liboverlay::egui::egui::Context::default();
        let mut actions = OverlayActions::default();
        let output = context.run(Default::default(), |ctx| {
            actions = dashboard::show(
                ctx,
                OverlayState {
                    cpu_percent: 42.0,
                    alert_threshold: 80.0,
                    alerts_enabled: 1,
                    alert_active: 0,
                },
            );
        });
        assert!(!output.shapes.is_empty());
        assert_eq!(actions.settings_changed, 0);
        assert_eq!(actions.alerts_enabled, 1);
        assert_eq!(actions.alert_threshold, 80.0);
    }
}
