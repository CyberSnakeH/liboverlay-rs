use crate::{BackendAttempt, Size};

pub type Result<T> = std::result::Result<T, OverlayError>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OverlayError {
    #[error("no display configured: WAYLAND_DISPLAY/WAYLAND_SOCKET and DISPLAY are unset")]
    NoDisplayServer,
    #[error("GPU: {0}")]
    Gpu(String),
    #[error("display runtime panicked")]
    RuntimePanic,
    #[error("invalid dimensions {0:?}")]
    InvalidSize(Size),
    #[error("frame allocation exceeds the configured {limit} byte limit")]
    MemoryLimit { limit: usize },
    #[error("could not allocate {bytes} bytes")]
    Allocation { bytes: usize },
    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),
    #[error("no usable display backend; attempts: {attempts:?}")]
    NoBackend { attempts: Vec<BackendAttempt> },
    #[error("unsupported capability: {0}")]
    Unsupported(&'static str),
    #[error("{backend} backend: {detail}")]
    Backend {
        backend: &'static str,
        detail: String,
    },
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("overlay is closed")]
    Closed,
    #[error("runtime command queue is full")]
    QueueFull,
    #[error("timed out while {0}; an already queued command may still complete")]
    Timeout(&'static str),
    #[error("frame belongs to a different overlay")]
    ForeignFrame,
    #[error("outdated frame: expected {expected:?}, received {actual:?}")]
    OutdatedFrame { expected: Size, actual: Size },
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
