//! Present notification ordering and pixmap reuse state.
use x11rb::protocol::present;

// COPY prevents a flip from keeping our single upload pixmap busy until a
// subsequent frame. Both notifications are required before its next upload:
// Idle permits reuse, Complete supplies the next vertical-blank target.
pub(super) struct PresentState {
    pub(super) event: present::Event,
    pub(super) serial: u32,
    idle: bool,
    complete: bool,
    pub(super) msc: u64,
}
impl PresentState {
    pub(super) fn new(event: present::Event) -> Self {
        Self {
            event,
            serial: 0,
            idle: true,
            complete: true,
            msc: 0,
        }
    }
    pub(super) fn ready(&self) -> bool {
        self.idle && self.complete
    }
    pub(super) fn submitted(&mut self) {
        self.serial = self.serial.wrapping_add(1);
        self.idle = false;
        self.complete = false;
    }
    pub(super) fn completed(&mut self, serial: u32, msc: u64) {
        if serial == self.serial {
            self.complete = true;
            self.msc = msc;
        }
    }
    pub(super) fn released(&mut self, serial: u32) {
        if serial == self.serial {
            self.idle = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn present_waits_for_both_notifications_and_ignores_obsolete_serials() {
        for idle_first in [true, false] {
            let mut state = PresentState::new(42);
            state.submitted();
            if idle_first {
                state.released(1);
            } else {
                state.completed(1, 100);
            }
            assert!(!state.ready());
            state.released(0);
            state.completed(0, 50);
            assert!(!state.ready());
            if idle_first {
                state.completed(1, 100);
            } else {
                state.released(1);
            }
            assert!(state.ready());
            assert_eq!(state.msc, 100);
            state.submitted();
            state.completed(1, 100);
            state.released(1);
            assert!(!state.ready());
        }
    }
}
