use crate::{
    BackendAttempt, BackendPreference, BackendReport, BackendType, Frame, OverlayBuilder,
    OverlayError, OverlayEvent, Result, WindowInfo,
};
use std::{
    os::fd::BorrowedFd,
    time::{Duration, Instant},
};

#[cfg(feature = "wayland")]
mod wayland;
#[cfg(feature = "x11")]
mod x11;

pub(crate) trait OverlayBackend {
    fn fd(&self) -> BorrowedFd<'_>;
    fn info(&self) -> WindowInfo;
    fn dispatch(&mut self) -> Result<Vec<OverlayEvent>>;
    fn present(&mut self, frame: &Frame) -> Result<bool>;
    fn set_click_through(&mut self, enabled: bool) -> Result<()>;
    fn flush(&mut self) -> Result<()>;
    fn wants_write(&self) -> bool {
        false
    }
    fn frame_wait(&self) -> Option<Duration> {
        None
    }
    #[cfg(feature = "gpu")]
    fn gpu_context(&self) -> Option<crate::gpu::GpuContext> {
        None
    }
    #[cfg(feature = "gpu")]
    fn present_gpu(&mut self, _: &mut Option<crate::gpu::GpuDraw>, _: WindowInfo) -> Result<bool> {
        Err(OverlayError::Unsupported("GPU rendering on this backend"))
    }
}

pub(crate) fn create(
    config: &OverlayBuilder,
    deadline: Instant,
) -> Result<(Box<dyn OverlayBackend>, BackendReport)> {
    let candidates = match config.backend {
        BackendPreference::WaylandOnly => vec![BackendType::Wayland],
        BackendPreference::X11Only => vec![BackendType::X11],
        BackendPreference::Auto => {
            let mut candidates = Vec::new();
            if std::env::var_os("WAYLAND_DISPLAY").is_some()
                || std::env::var_os("WAYLAND_SOCKET").is_some()
            {
                candidates.push(BackendType::Wayland);
            }
            if std::env::var_os("DISPLAY").is_some() {
                candidates.push(BackendType::X11);
            }
            candidates
        }
    };
    if candidates.is_empty() {
        return Err(OverlayError::NoDisplayServer);
    }
    let mut attempts = Vec::new();
    let count = candidates.len();
    for (index, candidate) in candidates.into_iter().enumerate() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let _attempt_deadline = Instant::now() + remaining / (count - index) as u32;
        let result: Result<(Box<dyn OverlayBackend>, BackendReport)> = match candidate {
            #[cfg(feature = "wayland")]
            BackendType::Wayland => wayland::WaylandBackend::create(config, _attempt_deadline)
                .map(|(backend, report)| (Box::new(backend) as Box<dyn OverlayBackend>, report)),
            #[cfg(feature = "x11")]
            BackendType::X11 => x11::X11Backend::create(config, _attempt_deadline)
                .map(|(backend, report)| (Box::new(backend) as Box<dyn OverlayBackend>, report)),
            #[allow(unreachable_patterns)]
            _ => Err(OverlayError::Unsupported(
                "backend disabled at compile time",
            )),
        };
        match result {
            Ok((backend, mut report)) => {
                report.fallback_attempts = attempts;
                return Ok((backend, report));
            }
            Err(error) => attempts.push(BackendAttempt {
                backend: candidate,
                reason: error.to_string(),
            }),
        }
    }
    Err(OverlayError::NoBackend { attempts })
}

#[cfg(feature = "wayland")]
pub(crate) fn wait_readable(fd: BorrowedFd<'_>, deadline: Instant, writable: bool) -> Result<()> {
    use rustix::event::{poll, PollFd, PollFlags, Timespec};
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(OverlayError::Timeout("waiting for the display server"))?;
        let timeout = Timespec::try_from(remaining)
            .map_err(|_| OverlayError::InvalidConfiguration("timeout overflow".into()))?;
        let mut flags = PollFlags::IN;
        if writable {
            flags |= PollFlags::OUT;
        }
        let mut fds = [PollFd::from_borrowed_fd(fd, flags)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => return Err(OverlayError::Timeout("waiting for the display server")),
            Ok(_) => return Ok(()),
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(std::io::Error::from(error).into()),
        }
    }
}
