//! Layouts shared with include/liboverlay_canvas.h.
use super::{Result, INVALID};
use std::ffi::c_char;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Text {
    pub data: *const c_char,
    pub length: usize,
}
impl Default for Text {
    fn default() -> Self {
        Self {
            data: std::ptr::null(),
            length: 0,
        }
    }
}
// SAFETY: callers guarantee readable storage for each non-null input. Copies are
// made immediately, and lengths are checked before constructing the slice.
pub(super) unsafe fn text(value: Text, limit: usize) -> Result<String> {
    if value.length > limit || (value.length != 0 && value.data.is_null()) {
        return Err((INVALID, "invalid string length or pointer".into()));
    }
    if value.length == 0 {
        return Ok(String::new());
    }
    // SAFETY: the API requires value.length readable bytes for this call.
    let bytes = unsafe { std::slice::from_raw_parts(value.data.cast::<u8>(), value.length) };
    let string = std::str::from_utf8(bytes).map_err(|_| (INVALID, "text must be UTF-8".into()))?;
    if string.contains('\0') {
        return Err((INVALID, "embedded NUL in text".into()));
    }
    Ok(string.into())
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Options {
    pub abi_version: u32,
    pub struct_size: u32,
    pub width: u32,
    pub height: u32,
    pub backend: u32,
    pub flags: u32,
    pub input_mode: u32,
    pub target_pid: u32,
    pub title: Text,
    pub output: Text,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            abi_version: 1,
            struct_size: std::mem::size_of::<Self>() as u32,
            width: 0,
            height: 0,
            backend: 0,
            flags: 0,
            input_mode: 0,
            target_pid: 0,
            title: Text::default(),
            output: Text::default(),
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Target {
    pub visible: u32,
    pub protocol: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub output: [u8; 128],
}
impl Default for Target {
    fn default() -> Self {
        Self {
            visible: 0,
            protocol: 0,
            x: 0.,
            y: 0.,
            width: 1.,
            height: 1.,
            output: [0; 128],
        }
    }
}
impl Target {
    pub fn output(&self) -> String {
        String::from_utf8_lossy(
            &self.output[..self.output.iter().position(|v| *v == 0).unwrap_or(128)],
        )
        .into_owned()
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Info {
    pub backend: u32,
    pub width: u32,
    pub height: u32,
    pub input_mode: u32,
    pub keyboard_capture: u32,
    pub global_shortcuts: u32,
    pub target: Target,
    pub output: [u8; 128],
}
impl Default for Info {
    fn default() -> Self {
        Self {
            backend: 0,
            width: 0,
            height: 0,
            input_mode: 0,
            keyboard_capture: 0,
            global_shortcuts: 0,
            target: Target::default(),
            output: [0; 128],
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Command {
    pub kind: u32,
    pub id: u32,
    pub color: u32,
    pub flags: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub stroke: f32,
    pub value: f32,
    pub minimum: f32,
    pub maximum: f32,
    pub x2: f32,
    pub y2: f32,
    pub x3: f32,
    pub y3: f32,
    pub text: Text,
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Frame {
    pub abi_version: u32,
    pub reserved: u32,
    pub commands: *const Command,
    pub count: usize,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Event {
    pub kind: u32,
    pub id: u32,
    pub value: f32,
    pub reserved: u32,
}
