//! Local X11 connection establishment and bounded protocol waits.
use crate::{OverlayBuilder, OverlayError, Result};
use std::{
    cell::Cell,
    io,
    os::fd::{AsFd, BorrowedFd},
    time::{Duration, Instant},
};
use x11rb::{
    reexports::x11rb_protocol::{parse_display, xauth},
    rust_connection::{DefaultStream, PollMode, RustConnection, Stream},
    utils::RawFdContainer,
};

pub(super) struct TimedStream {
    inner: DefaultStream,
    deadline: Cell<Option<Instant>>,
    timeout: Duration,
}
impl AsFd for TimedStream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}
impl Stream for TimedStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        use rustix::event::{poll, PollFd, PollFlags, Timespec};
        let deadline = self
            .deadline
            .get()
            .unwrap_or_else(|| Instant::now() + self.timeout);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::TimedOut, "X11 server did not respond")
                })?;
            let mut flags = PollFlags::empty();
            if mode.readable() {
                flags |= PollFlags::IN;
            }
            if mode.writable() {
                flags |= PollFlags::OUT;
            }
            let mut fds = [PollFd::new(self, flags)];
            let timeout = Timespec::try_from(remaining).map_err(io::Error::other)?;
            match poll(&mut fds, Some(&timeout)) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "X11 server did not respond",
                    ))
                }
                Ok(_) => return Ok(()),
                Err(rustix::io::Errno::INTR) => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }
    fn read(&self, bytes: &mut [u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.read(bytes, fds)
    }
    fn write(&self, bytes: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.inner.write(bytes, fds)
    }
    fn write_vectored(
        &self,
        bytes: &[io::IoSlice<'_>],
        fds: &mut Vec<RawFdContainer>,
    ) -> io::Result<usize> {
        self.inner.write_vectored(bytes, fds)
    }
}
pub(super) type XConnection = RustConnection<TimedStream>;

pub(super) fn connect(
    config: &OverlayBuilder,
    deadline: Instant,
) -> Result<(XConnection, usize, String)> {
    let display_name =
        std::env::var("DISPLAY").map_err(|e| crate::error::backend_error("X11", e))?;
    let display = parse_display::parse_display(Some(&display_name))
        .map_err(|e| crate::error::backend_error("X11", e))?;
    // The address iterator computes an optional TCP port as u16, even when
    // only its Unix-socket addresses are used below.
    if display.display > u16::MAX - 6000 {
        return Err(OverlayError::InvalidConfiguration(
            "X11 display number is out of range".into(),
        ));
    }
    let screen = usize::from(display.screen);
    let mut last = None;
    for address in display.connect_instruction() {
        // Only local Unix sockets are supported, avoiding unbounded DNS/TCP
        // connection attempts in the display worker.
        if !matches!(address, parse_display::ConnectAddress::Socket(_)) {
            continue;
        }
        match DefaultStream::connect(&address) {
            Ok((stream, (family, address))) => {
                let auth = match xauth::get_auth(family, &address, display.display) {
                    Ok(auth) => auth,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error.into()),
                };
                let (auth_name, auth_data) = auth.unwrap_or_default();
                let stream = TimedStream {
                    inner: stream,
                    deadline: Cell::new(Some(deadline)),
                    timeout: config.command_timeout,
                };
                return Ok((
                    RustConnection::connect_to_stream_with_auth_info(
                        stream, screen, auth_name, auth_data,
                    )?,
                    screen,
                    display_name,
                ));
            }
            Err(error) => last = Some(error),
        }
    }
    Err(last
        .map(OverlayError::Io)
        .unwrap_or(OverlayError::Unsupported("local X11 Unix socket")))
}

impl TimedStream {
    pub(super) fn finish_startup(&self) {
        self.deadline.set(None);
    }
}
