#[cfg(any(feature = "wayland", feature = "x11"))]
use crate::{InputId, InputResetReason, OverlayEvent, TouchPhase};
#[cfg(any(feature = "wayland", feature = "x11"))]
use std::collections::BTreeMap;

/// Maximum number of simultaneous contacts retained by one overlay.
pub const MAX_TOUCH_CONTACTS: usize = 64;

#[derive(Default)]
#[cfg(any(feature = "wayland", feature = "x11"))]
pub(crate) struct TouchTracker {
    contacts: BTreeMap<(InputId, i64), (f64, f64)>,
}

#[cfg(any(feature = "wayland", feature = "x11"))]
impl TouchTracker {
    pub(crate) fn begin(&mut self, source: InputId, id: i64, x: f64, y: f64) -> Vec<OverlayEvent> {
        let mut events = Vec::new();
        if !x.is_finite() || !y.is_finite() {
            return events;
        }
        if self.contacts.len() >= MAX_TOUCH_CONTACTS && !self.contacts.contains_key(&(source, id)) {
            self.contacts.clear();
            events.push(OverlayEvent::InputReset {
                source: None,
                scope: crate::InputResetScope::Touch,
                reason: InputResetReason::ContactLimit,
            });
        }
        if let Some((x, y)) = self.contacts.insert((source, id), (x, y)) {
            events.push(OverlayEvent::Touch {
                source,
                id,
                phase: TouchPhase::Cancelled,
                x,
                y,
            });
        }
        events.push(OverlayEvent::Touch {
            source,
            id,
            phase: TouchPhase::Started,
            x,
            y,
        });
        events
    }
    pub(crate) fn motion(
        &mut self,
        source: InputId,
        id: i64,
        x: f64,
        y: f64,
    ) -> Option<OverlayEvent> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        *self.contacts.get_mut(&(source, id))? = (x, y);
        Some(OverlayEvent::Touch {
            source,
            id,
            phase: TouchPhase::Moved,
            x,
            y,
        })
    }
    pub(crate) fn end(&mut self, source: InputId, id: i64) -> Option<OverlayEvent> {
        let (x, y) = self.contacts.remove(&(source, id))?;
        Some(OverlayEvent::Touch {
            source,
            id,
            phase: TouchPhase::Ended,
            x,
            y,
        })
    }
    pub(crate) fn cancel(&mut self, seat: Option<u32>, device: Option<u32>) -> Vec<OverlayEvent> {
        let mut events = Vec::new();
        self.contacts.retain(|(source, id), (x, y)| {
            if seat.is_none_or(|s| source.seat == s)
                && device.is_none_or(|d| source.device == Some(d))
            {
                events.push(OverlayEvent::Touch {
                    source: *source,
                    id: *id,
                    phase: TouchPhase::Cancelled,
                    x: *x,
                    y: *y,
                });
                false
            } else {
                true
            }
        });
        events
    }
}

#[cfg(all(test, any(feature = "wayland", feature = "x11")))]
mod tests {
    use super::*;
    fn source(seat: u32) -> InputId {
        InputId { seat, device: None }
    }
    #[test]
    fn contacts_are_scoped_to_seats_and_cancelled_on_removal() {
        let mut tracker = TouchTracker::default();
        tracker.begin(source(1), 7, 1., 2.);
        tracker.begin(source(2), 7, 3., 4.);
        assert_eq!(tracker.cancel(Some(1), None).len(), 1);
        assert!(tracker.end(source(1), 7).is_none());
        assert!(matches!(
            tracker.end(source(2), 7),
            Some(OverlayEvent::Touch {
                phase: TouchPhase::Ended,
                x: 3.,
                y: 4.,
                ..
            })
        ));
    }
    #[test]
    fn contact_saturation_is_observable_and_does_not_invent_orphan_moves() {
        let mut tracker = TouchTracker::default();
        for id in 0..MAX_TOUCH_CONTACTS as i64 {
            tracker.begin(source(1), id, 0., 0.);
        }
        let events = tracker.begin(source(1), 99, 1., 1.);
        assert!(matches!(
            events[0],
            OverlayEvent::InputReset {
                scope: crate::InputResetScope::Touch,
                reason: InputResetReason::ContactLimit,
                ..
            }
        ));
        assert!(tracker.motion(source(1), 0, 2., 2.).is_none());
        assert!(tracker.end(source(1), 99).is_some());
    }
}
