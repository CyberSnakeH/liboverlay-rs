//! Bounded command validation and rendering, independent of the desktop.
use super::{types::*, Result, INVALID, MAX_COMMANDS, MAX_WIDGETS};
use liboverlay::egui::egui;
use std::collections::HashSet;
#[derive(Clone)]
pub(super) struct OwnedCommand {
    raw: Command,
    text: String,
}

pub(super) unsafe fn copy_commands(frame: Frame) -> Result<Vec<OwnedCommand>> {
    if frame.abi_version != 1
        || frame.reserved != 0
        || frame.count > MAX_COMMANDS
        || (frame.count > 0 && frame.commands.is_null())
    {
        return Err((
            INVALID,
            "invalid frame version, count or commands pointer".into(),
        ));
    }
    let source = if frame.count == 0 {
        &[]
    } else {
        // SAFETY: the frame contract requires count readable, aligned commands.
        unsafe { std::slice::from_raw_parts(frame.commands, frame.count) }
    };
    let mut out = Vec::with_capacity(source.len());
    let mut total_text = 0;
    let mut panel = false;
    let mut ids = HashSet::new();
    let mut widgets = 0;
    for &raw in source {
        let finite = [
            raw.x,
            raw.y,
            raw.width,
            raw.height,
            raw.stroke,
            raw.value,
            raw.minimum,
            raw.maximum,
            raw.x2,
            raw.y2,
            raw.x3,
            raw.y3,
        ];
        if finite.iter().any(|v| !v.is_finite() || v.abs() > 1e7)
            || raw.stroke < 0.
            || raw.stroke > 1024.
        {
            return Err((INVALID, "non-finite or excessive command geometry".into()));
        }
        let allowed = match raw.kind {
            1 | 3 | 4 => 1,
            5 => 2 | 4 | 8 | 16,
            _ => 0,
        };
        if raw.flags & !allowed != 0
            || (raw.kind == 5 && (raw.flags & 6 == 6 || raw.flags & 24 == 24))
        {
            return Err((INVALID, "invalid command flags".into()));
        }
        match raw.kind {
            1..=7 if !panel => {
                if matches!(raw.kind, 1 | 6) && (raw.width < 0. || raw.height < 0.)
                    || raw.kind == 3 && raw.width < 0.
                    || raw.kind == 5 && !(1.0..=512.).contains(&raw.height)
                {
                    return Err((INVALID, "invalid primitive size".into()));
                }
            }
            10 if !panel && raw.width > 0. && raw.height > 0. => {
                panel = true;
            }
            11 if panel => {
                panel = false;
            }
            12..=16 if panel => {}
            _ => return Err((INVALID, "unknown command or invalid panel nesting".into())),
        }
        if matches!(raw.kind, 10 | 13..=15) && (raw.id == 0 || !ids.insert(raw.id)) {
            return Err((
                INVALID,
                "panel/widget ids must be distinct and nonzero".into(),
            ));
        }
        if matches!(raw.kind, 13..=15) {
            widgets += 1;
        }
        if widgets > MAX_WIDGETS {
            return Err((INVALID, "too many interactive widgets".into()));
        }
        if raw.kind == 14 && raw.value != 0. && raw.value != 1. {
            return Err((INVALID, "checkbox value must be 0 or 1".into()));
        }
        if raw.kind == 15
            && (raw.minimum >= raw.maximum || raw.value < raw.minimum || raw.value > raw.maximum)
        {
            return Err((INVALID, "invalid slider range/value".into()));
        }
        // SAFETY: all strings follow the frame's readable-storage contract.
        let owned = unsafe { text(raw.text, 65536) }?;
        total_text += owned.len();
        if total_text > 1024 * 1024 {
            return Err((INVALID, "frame text exceeds 1 MiB".into()));
        }
        // Do not retain the foreign pointer, even in the private copied command.
        out.push(OwnedCommand {
            raw: Command {
                text: Text::default(),
                ..raw
            },
            text: owned,
        });
    }
    if panel {
        return Err((INVALID, "unclosed panel".into()));
    }
    Ok(out)
}
fn color(v: u32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8, v as u8)
}
fn emit(events: &mut Vec<Event>, kind: u32, id: u32, value: f32) {
    if let Some(event) = events.iter_mut().find(|e| e.kind == kind && e.id == id) {
        event.value = value;
    } else {
        events.push(Event {
            kind,
            id,
            value,
            reserved: 0,
        });
    }
}
pub(super) fn show(ctx: &egui::Context, commands: &[OwnedCommand], events: &mut Vec<Event>) {
    use egui::{Pos2, Rect, Stroke, Vec2};
    let screen = ctx.screen_rect();
    let mut painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        egui::Id::new("liboverlay.canvas"),
    ));
    let mut i = 0;
    while i < commands.len() {
        let command = &commands[i];
        let c = command.raw;
        let p = Pos2::new(c.x, c.y);
        let rect = Rect::from_min_size(p, Vec2::new(c.width, c.height));
        let stroke = Stroke::new(c.stroke, color(c.color));
        match c.kind {
            1 => {
                if c.flags & 1 != 0 {
                    painter.rect_filled(rect, 0., color(c.color));
                } else {
                    painter.rect_stroke(rect, 0., stroke, egui::StrokeKind::Outside);
                }
            }
            2 => {
                painter.line_segment([p, Pos2::new(c.x2, c.y2)], stroke);
            }
            3 => {
                if c.flags & 1 != 0 {
                    painter.circle_filled(p, c.width, color(c.color));
                } else {
                    painter.circle_stroke(p, c.width, stroke);
                }
            }
            4 => {
                let points = vec![p, Pos2::new(c.x2, c.y2), Pos2::new(c.x3, c.y3)];
                painter.add(egui::Shape::convex_polygon(
                    points,
                    if c.flags & 1 != 0 {
                        color(c.color)
                    } else {
                        egui::Color32::TRANSPARENT
                    },
                    if c.flags & 1 != 0 {
                        Stroke::NONE
                    } else {
                        stroke
                    },
                ));
            }
            5 => {
                let h = if c.flags & 2 != 0 {
                    egui::Align::Center
                } else if c.flags & 4 != 0 {
                    egui::Align::Max
                } else {
                    egui::Align::Min
                };
                let v = if c.flags & 8 != 0 {
                    egui::Align::Center
                } else if c.flags & 16 != 0 {
                    egui::Align::Max
                } else {
                    egui::Align::Min
                };
                painter.text(
                    p,
                    egui::Align2([h, v]),
                    &command.text,
                    egui::FontId::proportional(c.height),
                    color(c.color),
                );
            }
            6 => painter = painter.with_clip_rect(rect.intersect(screen)),
            7 => painter = painter.with_clip_rect(screen),
            10 => {
                let start = i + 1;
                let end = start
                    + commands[start..]
                        .iter()
                        .position(|c| c.raw.kind == 11)
                        .expect("validated panel");
                egui::Area::new(egui::Id::new(("liboverlay.panel", c.id)))
                    .fixed_pos(p)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style())
                            .inner_margin(12.)
                            .show(ui, |ui| {
                                ui.set_width(c.width);
                                ui.set_max_height(c.height);
                                egui::ScrollArea::vertical()
                                    .max_height(c.height)
                                    .show(ui, |ui| {
                                        for widget in &commands[start..end] {
                                            let w = widget.raw;
                                            ui.push_id(w.id, |ui| match w.kind {
                                                12 => {
                                                    ui.colored_label(color(w.color), &widget.text);
                                                }
                                                13 => {
                                                    if ui.button(&widget.text).clicked() {
                                                        emit(events, 1, w.id, 1.);
                                                    }
                                                }
                                                14 => {
                                                    let mut value = w.value != 0.;
                                                    if ui
                                                        .checkbox(&mut value, &widget.text)
                                                        .changed()
                                                    {
                                                        emit(
                                                            events,
                                                            2,
                                                            w.id,
                                                            u8::from(value) as f32,
                                                        );
                                                    }
                                                }
                                                15 => {
                                                    let mut value = w.value;
                                                    if ui
                                                        .add(
                                                            egui::Slider::new(
                                                                &mut value,
                                                                w.minimum..=w.maximum,
                                                            )
                                                            .text(&widget.text),
                                                        )
                                                        .changed()
                                                    {
                                                        emit(events, 2, w.id, value);
                                                    }
                                                }
                                                16 => {
                                                    ui.separator();
                                                }
                                                _ => unreachable!("validated widget"),
                                            });
                                        }
                                    });
                            });
                    });
                i = end;
            }
            _ => unreachable!("validated command"),
        }
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn copied(commands: &[Command]) -> Result<Vec<OwnedCommand>> {
        // SAFETY: all test commands have live storage; test strings are static.
        unsafe {
            copy_commands(Frame {
                abi_version: 1,
                reserved: 0,
                commands: commands.as_ptr(),
                count: commands.len(),
            })
        }
    }
    #[test]
    fn commands_are_transactionally_validated() {
        assert!(copied(&[Command {
            kind: 14,
            id: 1,
            ..Default::default()
        }])
        .is_err());
        assert!(copied(&[Command {
            kind: 1,
            width: f32::NAN,
            ..Default::default()
        }])
        .is_err());
        let panel = Command {
            kind: 10,
            id: 1,
            width: 300.,
            height: 200.,
            ..Default::default()
        };
        assert!(copied(&[panel]).is_err());
        assert!(copied(&[
            panel,
            Command {
                kind: 13,
                id: 1,
                ..Default::default()
            },
            Command {
                kind: 11,
                ..Default::default()
            }
        ])
        .is_err());
        assert!(copied(&[Command {
            kind: 15,
            value: 2.,
            minimum: 0.,
            maximum: 1.,
            ..Default::default()
        }])
        .is_err());
        let bad = b"\xff";
        assert!(copied(&[Command {
            kind: 5,
            height: 14.,
            text: Text {
                data: bad.as_ptr().cast(),
                length: 1
            },
            ..Default::default()
        }])
        .is_err());
    }
    #[test]
    fn generic_ui_and_text_ownership() {
        let mut label = String::from("generic interface");
        let commands = [
            Command {
                kind: 10,
                id: 1,
                width: 250.,
                height: 200.,
                ..Default::default()
            },
            Command {
                kind: 12,
                color: 0xffffffff,
                text: Text {
                    data: label.as_ptr().cast(),
                    length: label.len(),
                },
                ..Default::default()
            },
            Command {
                kind: 14,
                id: 2,
                value: 1.,
                ..Default::default()
            },
            Command {
                kind: 11,
                ..Default::default()
            },
        ];
        let owned = copied(&commands).unwrap();
        label.clear();
        assert_eq!(owned[1].text, "generic interface");
        assert!(owned[1].raw.text.data.is_null());
        let ctx = egui::Context::default();
        let mut events = Vec::new();
        let out = ctx.run(Default::default(), |ctx| show(ctx, &owned, &mut events));
        assert!(!out.shapes.is_empty());
        assert!(events.is_empty());
        emit(&mut events, 2, 2, 1.);
        emit(&mut events, 2, 2, 0.);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].value, 0.);
    }
}
