use crate::{ScaleFactor, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowInfo {
    pub logical_size: Size,
    pub physical_size: Size,
    /// Exact scale in 120ths, including compositor-requested fractional scales.
    pub scale_factor: ScaleFactor,
    /// Changes on any buffer geometry change, including a resize back to an old size.
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonState {
    Pressed,
    Released,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum OverlayEvent {
    Resized(WindowInfo),
    /// Surface-local logical coordinates.
    PointerMoved {
        x: f64,
        y: f64,
    },
    PointerEntered,
    PointerLeft,
    /// Linux evdev button code (for example 272 for the left button).
    PointerButton {
        button: u32,
        state: ButtonState,
    },
    /// Linux evdev key code, without text composition or keymap translation.
    Key {
        code: u32,
        state: ButtonState,
    },
    /// Submitted to the display server; this is not a scanout confirmation.
    FrameSubmitted {
        generation: u64,
    },
    FrameDiscarded,
    Closed,
}
