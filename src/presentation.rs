//! Server-reported presentation timestamps, distinct from frame submission.
use std::time::Duration;

/// Protocol that reported completion; neither route measures optical response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationSource {
    /// Wayland presentation-time feedback and its explicit quality flags.
    Wayland,
    /// X11 Present pixmap completion; hardware timing quality is unspecified.
    X11,
}

/// Presentation feedback for a successfully submitted frame, enabled explicitly.
/// Timestamps are comparable only within the same protocol clock. A compositor's
/// completion does not include monitor pixel response or physical input latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationFeedback {
    /// Backend-local increasing identifier, starting at one for tracked frames.
    pub submission: u64,
    /// Buffer geometry generation associated with the submitted frame.
    pub generation: u64,
    /// Protocol used to obtain these timestamps.
    pub source: PresentationSource,
    /// Time in the server's clock domain, not an epoch-based wall clock.
    pub timestamp: Duration,
    /// Wayland POSIX clock ID; X11 UST does not negotiate a clock ID.
    pub clock_id: Option<u32>,
    /// Advertised refresh interval, absent when the server reports zero or X11.
    pub refresh_interval: Option<Duration>,
    /// Presentation sequence / X11 media stream counter.
    pub sequence: u64,
    /// Wayland explicitly reported synchronization with vertical retrace.
    pub vsync: bool,
    /// Wayland explicitly reported a hardware-derived timestamp.
    pub hardware_clock: bool,
    /// Wayland explicitly reported hardware presentation completion.
    pub hardware_completion: bool,
    /// Wayland explicitly reported zero-copy presentation.
    pub zero_copy: bool,
}
