//! Shared input contracts, independent of the display transport.
#[cfg(any(feature = "wayland", feature = "x11"))]
pub(crate) mod keyboard;
mod region;
mod text;
pub(crate) mod touch;

pub use region::{InputRectangle, InputRegion, MAX_INPUT_RECTANGLES};
pub use text::{SurroundingText, TextChangeCause, TextInputPurpose, TextInputState};

/// Maximum simultaneous pressed keys tracked for one keyboard source.
pub const MAX_PRESSED_KEYS: usize = 256;

/// XKB modifier masks and active layout group, copied at event time.
/// Low mask bits follow XKB core conventions: Shift=1, Lock=2, Control=4,
/// Mod1=8, Mod2=16, Mod3=32, Mod4=64, Mod5=128. Custom maps may assign
/// different meanings to Mod1–Mod5; the original masks are retained.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyboardModifiers {
    /// Physically held modifiers.
    pub depressed: u32,
    /// Modifiers latched for a subsequent key.
    pub latched: u32,
    /// Locked modifiers, including Caps Lock on conventional maps.
    pub locked: u32,
    /// Effective union of all modifier masks.
    pub effective: u32,
    /// Zero-based effective layout group.
    pub group: u32,
}

/// Keymap-translated key event. For text editing, insert only `TextCommitted`;
/// `text` here describes the individual key before compose/IME processing.
#[derive(Debug, Clone)]
pub struct KeyboardInput {
    /// Backend-local source of the key.
    pub source: InputId,
    /// Linux evdev code, using the XKB +8 convention on X11.
    pub code: u32,
    /// XKB keysyms produced by the active layout and modifiers.
    pub keysyms: Vec<u32>,
    /// Printable text produced by this key before composition.
    pub text: Option<String>,
    /// Press or release transition.
    pub state: crate::ButtonState,
    /// Whether this press is an automatic repeat.
    pub repeat: bool,
    /// Active modifiers and layout at the time of this event.
    pub modifiers: KeyboardModifiers,
    /// Effective modifier bits not consumed by selecting this key's XKB level.
    /// Use these bits when matching shortcuts against the translated keysyms.
    pub unconsumed_modifiers: u32,
}

/// Backend-local input identity, stable until its seat/device is removed.
/// Wayland exposes a seat but no physical device identifier. X11 uses the
/// master device as the seat and provides a physical source when available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InputId {
    /// Wayland registry seat name or XInput master device ID.
    pub seat: u32,
    /// XInput source ID; `None` where the transport does not identify hardware.
    pub device: Option<u32>,
}

/// Lifecycle transition for a touch contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    /// New contact; coordinates are surface-local logical pixels.
    Started,
    /// An existing contact moved.
    Moved,
    /// The contact was released normally.
    Ended,
    /// The contact was interrupted; coordinates retain its last known position.
    Cancelled,
}

/// Which application input state was invalidated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputResetScope {
    /// Pressed keys, repetition and composition; touch/pointer state stays valid.
    Keyboard,
    /// Active touch contacts; keyboard/pointer state stays valid.
    Touch,
    /// Keys, buttons, contacts and composition.
    All,
}

/// Why the input state identified by a reset's scope became invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputResetReason {
    /// The bounded event queue lost transitions. Reapply text context if editing.
    QueueOverflow,
    /// Keyboard focus was lost.
    FocusLost,
    /// The input seat or device disappeared.
    DeviceRemoved,
    /// The runtime closed, including after a display or device failure.
    Closed,
    /// The display cancelled an input sequence.
    Cancelled,
    /// The keyboard map changed while input was active.
    KeymapChanged,
    /// More touch contacts arrived than the configured bound can retain.
    ContactLimit,
    /// More pressed keys arrived than one keyboard source can retain.
    KeyLimit,
}
