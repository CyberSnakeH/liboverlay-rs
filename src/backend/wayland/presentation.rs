//! Bounded presentation-time feedback for this surface's commits.
use super::*;
use wayland_protocols::wp::presentation_time::client::{wp_presentation, wp_presentation_feedback};
pub(super) type Manager = wp_presentation::WpPresentation;
pub(super) type Feedback = wp_presentation_feedback::WpPresentationFeedback;

#[derive(Clone, Copy)]
pub(super) struct Token {
    submission: u64,
    generation: u64,
}
impl WaylandBackend {
    pub(super) fn skip_presentation(&mut self) {
        if self.feedback_enabled && self.state.presentation.is_some() {
            self.state.next_submission = self.state.next_submission.wrapping_add(1);
            self.state.events.push(OverlayEvent::PresentationDiscarded {
                submission: self.state.next_submission,
                generation: self.info.generation,
            });
        }
    }
    pub(super) fn request_presentation(&mut self) {
        if !self.feedback_enabled {
            return;
        }
        let Some(manager) = &self.state.presentation else {
            return;
        };
        self.state.next_submission = self.state.next_submission.wrapping_add(1);
        let token = Token {
            submission: self.state.next_submission,
            generation: self.info.generation,
        };
        if self.state.feedbacks.len() >= 64 {
            self.state.events.push(OverlayEvent::PresentationDiscarded {
                submission: token.submission,
                generation: token.generation,
            });
            return;
        }
        let feedback = manager.feedback(&self.surface, &self.queue.handle(), token);
        self.state.feedbacks.insert(token.submission, feedback);
    }
}
impl Dispatch<Manager, ()> for State {
    fn event(
        state: &mut Self,
        _: &Manager,
        event: wp_presentation::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_presentation::Event::ClockId { clk_id } = event {
            state.presentation_clock = Some(clk_id);
        }
    }
}
impl Dispatch<Feedback, Token> for State {
    fn event(
        state: &mut Self,
        _: &Feedback,
        event: wp_presentation_feedback::Event,
        token: &Token,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let event = match event {
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            } => {
                if tv_nsec >= 1_000_000_000 {
                    state.feedbacks.remove(&token.submission);
                    state.events.push(OverlayEvent::PresentationDiscarded {
                        submission: token.submission,
                        generation: token.generation,
                    });
                    return;
                }
                let flags = match flags {
                    WEnum::Value(flags) => flags,
                    WEnum::Unknown(bits) => {
                        wp_presentation_feedback::Kind::from_bits_truncate(bits)
                    }
                };
                Some(OverlayEvent::FramePresented(crate::PresentationFeedback {
                    submission: token.submission,
                    generation: token.generation,
                    source: crate::PresentationSource::Wayland,
                    timestamp: Duration::new(
                        (u64::from(tv_sec_hi) << 32) | u64::from(tv_sec_lo),
                        tv_nsec,
                    ),
                    clock_id: state.presentation_clock,
                    refresh_interval: (refresh > 0)
                        .then(|| Duration::from_nanos(u64::from(refresh))),
                    sequence: (u64::from(seq_hi) << 32) | u64::from(seq_lo),
                    vsync: flags.contains(wp_presentation_feedback::Kind::Vsync),
                    hardware_clock: flags.contains(wp_presentation_feedback::Kind::HwClock),
                    hardware_completion: flags
                        .contains(wp_presentation_feedback::Kind::HwCompletion),
                    zero_copy: flags.contains(wp_presentation_feedback::Kind::ZeroCopy),
                }))
            }
            wp_presentation_feedback::Event::Discarded => {
                Some(OverlayEvent::PresentationDiscarded {
                    submission: token.submission,
                    generation: token.generation,
                })
            }
            _ => None,
        };
        if let Some(event) = event {
            state.feedbacks.remove(&token.submission);
            state.events.push(event);
        }
    }
}
