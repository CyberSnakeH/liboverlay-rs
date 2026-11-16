use crate::{ScaleFactor, Size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The most recently negotiated surface geometry.
pub struct WindowInfo {
    /// Surface extent in logical coordinates.
    pub logical_size: Size,
    /// Buffer/swapchain extent after scaling and rounding.
    pub physical_size: Size,
    /// Exact scale in 120ths, including compositor-requested fractional scales.
    pub scale_factor: ScaleFactor,
    /// Changes on any buffer geometry change, including a resize back to an old size.
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Transition of a pointer button or raw key.
pub enum ButtonState {
    /// The control was pressed.
    Pressed,
    /// The control was released.
    Released,
}

/// Positive scrolling moves down or right; negative scrolling moves up or left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAxis {
    /// Left/right scrolling.
    Horizontal,
    /// Up/down scrolling.
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Units attached to a scroll delta; backends do not silently convert between them.
pub enum ScrollUnit {
    /// Surface-local logical pixels reported by Wayland.
    Pixels,
    /// Wheel steps reported by core X11 button events.
    Steps,
}

#[derive(Debug, Clone)]
#[non_exhaustive]
/// Bounded runtime notifications, consumed through `OverlayWindow::poll_events`.
pub enum OverlayEvent {
    /// XKB-translated key, in addition to the legacy raw `Key` event.
    KeyboardInput(crate::KeyboardInput),
    /// Effective keyboard modifiers or layout group changed.
    ModifiersChanged {
        /// Keyboard seat/source.
        source: crate::InputId,
        /// Current XKB components.
        modifiers: crate::KeyboardModifiers,
    },
    /// Keyboard focus entered or left this overlay.
    KeyboardFocus {
        /// Keyboard seat/source.
        source: crate::InputId,
        /// Whether this overlay is focused.
        focused: bool,
    },
    /// UTF-8 insertion from local Compose or the desktop input method.
    TextCommitted {
        /// Keyboard seat/source associated with the text field.
        source: crate::InputId,
        /// Text to insert exactly once at the application cursor.
        text: String,
    },
    /// Composition preview replacing the previous preview, without editing the document.
    TextPreedit {
        /// Keyboard seat/source associated with the text field.
        source: crate::InputId,
        /// UTF-8 preview; empty text clears it.
        text: String,
        /// Selection/caret range as UTF-8 byte offsets, or `None` for no caret.
        cursor: Option<std::ops::Range<usize>>,
    },
    /// Delete UTF-8 bytes adjacent to the cursor before applying the associated commit.
    TextDeleteSurrounding {
        /// Keyboard seat/source associated with the text field.
        source: crate::InputId,
        /// Bytes preceding the cursor, excluding the selection.
        before: u32,
        /// Bytes following the cursor, excluding the selection.
        after: u32,
    },
    /// Desktop input-method focus/context became active or inactive.
    InputMethod {
        /// Keyboard seat/source associated with the text field.
        source: crate::InputId,
        /// A desktop protocol context is enabled; this does not identify an IME process.
        active: bool,
    },
    /// Nonfatal desktop input-method failure; local XCompose remains available.
    InputMethodError {
        /// Keyboard seat/source whose context failed.
        source: crate::InputId,
        /// Bounded diagnostic description.
        message: String,
    },
    /// Input state was invalidated. Clear the state specified by `scope`;
    /// keyboard resets also clear composition. Unrelated input remains valid.
    InputReset {
        /// Affected source, or all sources for queue overflow/runtime closure.
        source: Option<crate::InputId>,
        /// Kinds of state invalidated for the associated source(s).
        scope: crate::InputResetScope,
        /// Cause of invalidation.
        reason: crate::InputResetReason,
    },
    /// Contact transition. IDs are unique only within a source while active.
    Touch {
        /// Backend-local seat and optional physical source device.
        source: crate::InputId,
        /// Protocol contact identifier; may be reused after end or cancellation.
        id: i64,
        /// Contact lifecycle transition.
        phase: crate::TouchPhase,
        /// Last known horizontal logical coordinate.
        x: f64,
        /// Last known vertical logical coordinate.
        y: f64,
    },
    /// All preceding Wayland touch changes for this source form one frame.
    TouchFrame {
        /// Seat/source to which the touch frame belongs.
        source: crate::InputId,
    },
    /// Logical size, physical size, scale or buffer generation changed.
    Resized(WindowInfo),
    /// Surface-local logical coordinates.
    PointerMoved {
        /// Horizontal logical coordinate.
        x: f64,
        /// Vertical logical coordinate.
        y: f64,
    },
    /// The pointer entered the surface.
    PointerEntered,
    /// The pointer left the surface.
    PointerLeft,
    /// A pointer scroll delta, positive down/right and negative up/left.
    PointerAxis {
        /// Horizontal or vertical direction.
        axis: ScrollAxis,
        /// Signed delta in the accompanying unit.
        value: f64,
        /// Pixel deltas on Wayland, discrete wheel steps on core X11.
        unit: ScrollUnit,
    },
    /// Linux evdev button code (for example 272 for the left button).
    PointerButton {
        /// Linux evdev button code.
        button: u32,
        /// Press or release transition.
        state: ButtonState,
    },
    /// Linux evdev key code, without text composition or keymap translation.
    Key {
        /// Linux evdev key code.
        code: u32,
        /// Press or release transition.
        state: ButtonState,
    },
    /// Server-reported presentation completion, when explicitly enabled.
    FramePresented(crate::PresentationFeedback),
    /// A tracked presentation was discarded, tracking was full, or an unchanged
    /// CPU frame needed no new Wayland buffer content.
    PresentationDiscarded {
        /// Backend-local identifier of the frame whose feedback was discarded.
        submission: u64,
        /// Geometry generation associated with that frame.
        generation: u64,
    },
    /// Submitted to the display server; this is not a scanout confirmation.
    FrameSubmitted {
        /// Geometry generation of the submitted frame.
        generation: u64,
    },
    /// A queued frame was superseded or invalidated by a geometry change.
    FrameDiscarded,
    /// The display runtime has stopped and released its backend.
    Closed,
}
