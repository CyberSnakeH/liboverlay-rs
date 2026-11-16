use crate::{BackendAttempt, Size};

/// Result type returned by overlay configuration, rendering and lifecycle APIs.
pub type Result<T> = std::result::Result<T, OverlayError>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
/// Configuration, backend, rendering or runtime failure.
pub enum OverlayError {
    /// No display environment variable was configured for automatic selection.
    #[error("no display configured: WAYLAND_DISPLAY/WAYLAND_SOCKET and DISPLAY are unset")]
    NoDisplayServer,
    /// Vulkan initialization, device or presentation failure.
    #[error("GPU: {0}")]
    Gpu(String),
    /// A display-thread operation or GPU callback unwound.
    #[error("display runtime panicked")]
    RuntimePanic,
    /// Zero, overflowing or protocol-incompatible dimensions.
    #[error("invalid dimensions {0:?}")]
    InvalidSize(Size),
    /// A physical frame/backing buffer would exceed its configured bound.
    #[error("frame allocation exceeds the configured {limit} byte limit")]
    MemoryLimit {
        /// Configured per-buffer bound in bytes.
        limit: usize,
    },
    /// The allocator could not reserve a frame.
    #[error("could not allocate {bytes} bytes")]
    Allocation {
        /// Requested allocation size in bytes.
        bytes: usize,
    },
    /// A title, margin, timeout or other builder option was invalid.
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),
    /// Every permitted display backend failed at startup.
    #[error("no usable display backend; attempts: {attempts:?}")]
    NoBackend {
        /// Backend attempts in execution order, including individual reasons.
        attempts: Vec<BackendAttempt>,
    },
    /// A capability, renderer operation or synchronous callback command is unsupported.
    #[error("unsupported capability: {0}")]
    Unsupported(&'static str),
    /// A display protocol or connection operation failed.
    #[error("{backend} backend: {detail}")]
    Backend {
        /// Backend that reported the failure.
        backend: &'static str,
        /// Original human-readable error detail.
        detail: String,
    },
    /// Operating-system I/O failure.
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    /// The runtime no longer accepts drawing or input commands.
    #[error("overlay is closed")]
    Closed,
    /// The bounded command queue is full; the caller can retry later.
    #[error("runtime command queue is full")]
    QueueFull,
    /// A wait expired. A previously queued command may still complete.
    #[error("timed out while {0}; an already queued command may still complete")]
    Timeout(&'static str),
    /// A CPU frame was submitted to a different overlay from its creator.
    #[error("frame belongs to a different overlay")]
    ForeignFrame,
    /// A CPU frame's dimensions or generation no longer match the window.
    #[error("outdated frame: expected {expected:?}, received {actual:?}")]
    OutdatedFrame {
        /// Current physical extent.
        expected: Size,
        /// Submitted physical extent, possibly equal after a generation change.
        actual: Size,
    },
    /// A private runtime mutex was poisoned by an unwind.
    #[error("runtime state lock was poisoned")]
    Poisoned,
}

#[cfg(any(feature = "wayland", feature = "x11"))]
pub(crate) fn backend_error(backend: &'static str, error: impl std::fmt::Display) -> OverlayError {
    OverlayError::Backend {
        backend,
        detail: error.to_string(),
    }
}

#[cfg(feature = "x11")]
macro_rules! x11_error {
    ($($ty:ty),+ $(,)?) => { $(impl From<$ty> for OverlayError {
        fn from(error: $ty) -> Self { backend_error("X11", error) }
    })+ };
}
#[cfg(feature = "x11")]
x11_error!(
    x11rb::errors::ConnectError,
    x11rb::errors::ConnectionError,
    x11rb::errors::ReplyError,
    x11rb::errors::ReplyOrIdError
);

#[cfg(feature = "wayland")]
macro_rules! wayland_error {
    ($($ty:ty),+ $(,)?) => { $(impl From<$ty> for OverlayError {
        fn from(error: $ty) -> Self { backend_error("Wayland", error) }
    })+ };
}
#[cfg(feature = "wayland")]
wayland_error!(
    wayland_client::ConnectError,
    wayland_client::DispatchError,
    wayland_client::backend::WaylandError
);
