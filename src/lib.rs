//! Transparent Linux overlays with explicit backend capabilities.
//!
//! The public window is a thread-safe command handle. Display connections and
//! shared-memory buffers stay on a private runtime thread. See the README for
//! compositor-dependent limitations and the meaning of frame submission.
#![doc = include_str!("../README.md")]
#![cfg_attr(not(target_os = "linux"), allow(unused))]

#[cfg(not(target_os = "linux"))]
compile_error!("liboverlay-rs currently supports Linux only");

mod backend;
mod builder;
mod capabilities;
#[cfg(any(feature = "wayland", feature = "x11"))]
mod damage;
#[cfg(feature = "egui")]
pub mod egui;
mod error;
mod events;
#[cfg(feature = "gpu")]
pub mod gpu;
mod input;
mod presentation;
mod render;
mod runtime;

pub use builder::{
    Anchor, BackendPreference, KeyboardMode, OutputSelector, OverlayBuilder, RendererPreference,
    Requirement, StackingPolicy,
};
pub use capabilities::{
    BackendAttempt, BackendReport, BackendType, Capabilities, FramePacing, GpuAdapterInfo,
    RendererType, StackingSupport, Support,
};
pub use error::{OverlayError, Result};
pub use events::{ButtonState, OverlayEvent, ScrollAxis, ScrollUnit, WindowInfo};
pub use input::touch::MAX_TOUCH_CONTACTS;
pub use input::MAX_PRESSED_KEYS;
pub use input::{
    InputId, InputRectangle, InputRegion, InputResetReason, InputResetScope, TouchPhase,
    MAX_INPUT_RECTANGLES,
};
pub use input::{
    KeyboardInput, KeyboardModifiers, SurroundingText, TextChangeCause, TextInputPurpose,
    TextInputState,
};
pub use presentation::{PresentationFeedback, PresentationSource};
pub use render::{premultiply, Frame, PixelFormat, ScaleFactor, Size};
pub use runtime::{OverlayWindow, WeakOverlayWindow};
