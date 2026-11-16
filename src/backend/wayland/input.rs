//! Seat input, keyboard maps, touch and text input.
use super::*;
use crate::input::keyboard::{IgnoreDiagnostics, Keyboard, MAX_KEYMAP_BYTES, MAX_TEXT_BYTES};
use std::os::unix::fs::FileExt;
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};

pub(super) type TextManager = zwp_text_input_manager_v3::ZwpTextInputManagerV3;

fn read_keymap(file: &File, size: u32) -> Result<Vec<u8>> {
    let metadata = file.metadata()?;
    if size == 0 || size as usize > MAX_KEYMAP_BYTES || !metadata.is_file() {
        return Err(crate::error::backend_error(
            "Wayland",
            "invalid XKB keymap size or file",
        ));
    }
    let mut bytes = vec![0; size as usize];
    // SCM_RIGHTS can share an open-file description with the compositor or
    // another keyboard. Read from byte zero without using or changing its cursor.
    // Unlike an mmap, a bounded positional read also handles truncation as I/O.
    if metadata.len() < u64::from(size) {
        return Err(crate::error::backend_error(
            "Wayland",
            "truncated XKB keymap",
        ));
    }
    file.read_exact_at(&mut bytes, 0)?;
    if bytes.last() != Some(&0) {
        return Err(crate::error::backend_error(
            "Wayland",
            "truncated XKB keymap",
        ));
    }
    bytes.pop();
    Ok(bytes)
}

impl Dispatch<wl_seat::WlSeat, u32> for State {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        id: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            if capabilities.contains(wl_seat::Capability::Touch) {
                state
                    .touches
                    .entry(*id)
                    .or_insert_with(|| seat.get_touch(qh, *id));
            } else if let Some(touch) = state.touches.remove(id) {
                state
                    .events
                    .extend(state.touch_state.cancel(Some(*id), None));
                if touch.version() >= 3 {
                    touch.release();
                }
            }
            if capabilities.contains(wl_seat::Capability::Pointer) {
                state
                    .pointers
                    .entry(*id)
                    .or_insert_with(|| seat.get_pointer(qh, ()));
            } else if let Some(pointer) = state.pointers.remove(id) {
                if pointer.version() >= 3 {
                    pointer.release();
                }
            }
            if capabilities.contains(wl_seat::Capability::Keyboard)
                && state.keyboard_mode == KeyboardMode::OnDemand
            {
                state
                    .keyboards
                    .entry(*id)
                    .or_insert_with(|| seat.get_keyboard(qh, *id));
                state.keyboard_states.entry(*id).or_insert_with(|| {
                    Keyboard::new(crate::InputId {
                        seat: *id,
                        device: None,
                    })
                });
            } else if let Some(keyboard) = state.keyboards.remove(id) {
                if let Some(mut keyboard) = state.keyboard_states.remove(id) {
                    state.events.extend(keyboard.focus(false));
                }
                if keyboard.version() >= 3 {
                    keyboard.release();
                }
            }
        }
    }
}
impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let event = match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            } => {
                state.events.push(OverlayEvent::PointerEntered);
                Some(OverlayEvent::PointerMoved {
                    x: surface_x,
                    y: surface_y,
                })
            }
            wl_pointer::Event::Leave { .. } => Some(OverlayEvent::PointerLeft),
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => Some(OverlayEvent::PointerMoved {
                x: surface_x,
                y: surface_y,
            }),
            wl_pointer::Event::Button {
                button,
                state: WEnum::Value(button_state),
                ..
            } => Some(OverlayEvent::PointerButton {
                button,
                state: if button_state == wl_pointer::ButtonState::Pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                },
            }),
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => {
                let axis = match axis {
                    wl_pointer::Axis::VerticalScroll => crate::ScrollAxis::Vertical,
                    wl_pointer::Axis::HorizontalScroll => crate::ScrollAxis::Horizontal,
                    _ => return,
                };
                Some(OverlayEvent::PointerAxis {
                    axis,
                    value,
                    unit: crate::ScrollUnit::Pixels,
                })
            }
            _ => None,
        };
        if let Some(event) = event {
            state.events.push(event);
        }
    }
}
impl Dispatch<wl_touch::WlTouch, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_touch::WlTouch,
        event: wl_touch::Event,
        seat: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let source = crate::InputId {
            seat: *seat,
            device: None,
        };
        match event {
            wl_touch::Event::Down {
                surface, id, x, y, ..
            } if state.surface.as_ref() == Some(&surface) => {
                state
                    .events
                    .extend(state.touch_state.begin(source, i64::from(id), x, y));
            }
            wl_touch::Event::Motion { id, x, y, .. } => {
                if let Some(event) = state.touch_state.motion(source, i64::from(id), x, y) {
                    state.events.push(event);
                }
            }
            wl_touch::Event::Up { id, .. } => {
                if let Some(event) = state.touch_state.end(source, i64::from(id)) {
                    state.events.push(event);
                }
            }
            wl_touch::Event::Cancel => state
                .events
                .extend(state.touch_state.cancel(Some(*seat), None)),
            wl_touch::Event::Frame => state.events.push(OverlayEvent::TouchFrame { source }),
            _ => {}
        }
    }
}
impl Dispatch<wl_keyboard::WlKeyboard, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(keyboard) = state.keyboard_states.get_mut(id) else {
            return;
        };
        match event {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                let file = File::from(fd);
                let bytes = read_keymap(&file, size);
                match bytes.and_then(|bytes| {
                    kbvm::xkb::Context::default()
                        .keymap_from_bytes(IgnoreDiagnostics, None, bytes)
                        .map_err(|e| {
                            crate::error::backend_error(
                                "Wayland",
                                format!("invalid XKB keymap: {e}"),
                            )
                        })
                }) {
                    Ok(map) => {
                        keyboard.set_keymap(map);
                        state.events.push(OverlayEvent::InputReset {
                            source: Some(keyboard.source),
                            scope: crate::InputResetScope::Keyboard,
                            reason: crate::InputResetReason::KeymapChanged,
                        });
                    }
                    Err(error) => state.input_error = Some(error),
                }
            }
            wl_keyboard::Event::Keymap { .. } => {
                keyboard.table = None;
                keyboard.reset();
            }
            wl_keyboard::Event::Enter { surface, keys, .. }
                if state.surface.as_ref() == Some(&surface) =>
            {
                state.events.extend(keyboard.focus(true));
                keyboard.enter_pressed(
                    keys.chunks_exact(4)
                        .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]])),
                );
            }
            wl_keyboard::Event::Leave { .. } => state.events.extend(keyboard.focus(false)),
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                state.events.push(keyboard.update_modifiers(
                    mods_depressed,
                    mods_latched,
                    mods_locked,
                    group,
                ));
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => keyboard.repeat_info(rate, delay),
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(key_state),
                ..
            } => {
                let button = if key_state == wl_keyboard::KeyState::Pressed {
                    ButtonState::Pressed
                } else {
                    ButtonState::Released
                };
                state.events.extend(keyboard.key(
                    key,
                    button,
                    false,
                    state.text_input.enabled,
                    true,
                ));
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct PendingText {
    preedit: Option<(String, Option<std::ops::Range<usize>>)>,
    commit: Option<String>,
    delete: Option<(u32, u32)>,
}

pub(super) struct TextInput {
    pub proxy: zwp_text_input_v3::ZwpTextInputV3,
    entered: bool,
    active: bool,
    surrounding: bool,
    serial: u32,
    pending: PendingText,
}

impl TextInput {
    fn apply(&mut self, state: &crate::TextInputState, reset: bool) {
        if !self.entered {
            return;
        }
        let reset = reset || self.surrounding != state.surrounding.is_some();
        if state.enabled {
            if !self.active || reset {
                if self.active {
                    self.proxy.disable();
                }
                self.proxy.enable();
                self.pending = PendingText::default();
            }
            if let Some(text) = &state.surrounding {
                self.proxy.set_surrounding_text(
                    text.text.clone(),
                    text.cursor as i32,
                    text.anchor as i32,
                );
            }
            self.proxy.set_text_change_cause(match state.change_cause {
                crate::TextChangeCause::InputMethod => zwp_text_input_v3::ChangeCause::InputMethod,
                crate::TextChangeCause::Other => zwp_text_input_v3::ChangeCause::Other,
            });
            let (purpose, hints) = match state.purpose {
                crate::TextInputPurpose::Normal => (
                    zwp_text_input_v3::ContentPurpose::Normal,
                    zwp_text_input_v3::ContentHint::None,
                ),
                crate::TextInputPurpose::Number => (
                    zwp_text_input_v3::ContentPurpose::Number,
                    zwp_text_input_v3::ContentHint::None,
                ),
                crate::TextInputPurpose::Phone => (
                    zwp_text_input_v3::ContentPurpose::Phone,
                    zwp_text_input_v3::ContentHint::None,
                ),
                crate::TextInputPurpose::Email => (
                    zwp_text_input_v3::ContentPurpose::Email,
                    zwp_text_input_v3::ContentHint::None,
                ),
                crate::TextInputPurpose::Password => (
                    zwp_text_input_v3::ContentPurpose::Password,
                    zwp_text_input_v3::ContentHint::HiddenText
                        | zwp_text_input_v3::ContentHint::SensitiveData,
                ),
                crate::TextInputPurpose::Pin => (
                    zwp_text_input_v3::ContentPurpose::Pin,
                    zwp_text_input_v3::ContentHint::HiddenText
                        | zwp_text_input_v3::ContentHint::SensitiveData,
                ),
                crate::TextInputPurpose::Terminal => (
                    zwp_text_input_v3::ContentPurpose::Terminal,
                    zwp_text_input_v3::ContentHint::None,
                ),
            };
            self.proxy.set_content_type(hints, purpose);
            let rect = state.cursor_rectangle;
            self.proxy.set_cursor_rectangle(
                i32::from(rect.x),
                i32::from(rect.y),
                i32::from(rect.width),
                i32::from(rect.height),
            );
        } else if self.active {
            self.proxy.disable();
            self.pending = PendingText::default();
        }
        self.active = state.enabled;
        self.surrounding = state.surrounding.is_some();
        self.proxy.commit();
        self.serial = self.serial.wrapping_add(1);
    }
}

impl State {
    pub(super) fn ensure_text_inputs(&mut self, qh: &QueueHandle<Self>) {
        let Some(manager) = &self.text_manager else {
            return;
        };
        for (id, seat) in &self.seats {
            self.text_inputs.entry(*id).or_insert_with(|| TextInput {
                proxy: manager.get_text_input(seat, qh, *id),
                entered: false,
                active: false,
                surrounding: false,
                serial: 0,
                pending: PendingText::default(),
            });
        }
    }
    pub(super) fn refresh_text_inputs(&mut self, reset: bool) {
        for (seat, input) in &mut self.text_inputs {
            let was_active = input.active;
            input.apply(&self.text_input, reset);
            if was_active != input.active {
                self.events.push(OverlayEvent::InputMethod {
                    source: crate::InputId {
                        seat: *seat,
                        device: None,
                    },
                    active: input.active,
                });
            }
        }
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, u32> for State {
    fn event(
        state: &mut Self,
        _: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        seat: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(input) = state.text_inputs.get_mut(seat) else {
            return;
        };
        let source = crate::InputId {
            seat: *seat,
            device: None,
        };
        match event {
            zwp_text_input_v3::Event::Enter { surface }
                if state.surface.as_ref() == Some(&surface) =>
            {
                input.entered = true;
                input.apply(&state.text_input, true);
                state.events.push(OverlayEvent::InputMethod {
                    source,
                    active: input.active,
                });
            }
            zwp_text_input_v3::Event::Leave { .. } => {
                input.entered = false;
                input.active = false;
                input.pending = PendingText::default();
                if let Some(keyboard) = state.keyboard_states.get_mut(seat) {
                    keyboard.reset_compose();
                }
                state.events.push(OverlayEvent::TextPreedit {
                    source,
                    text: String::new(),
                    cursor: None,
                });
                state.events.push(OverlayEvent::InputMethod {
                    source,
                    active: false,
                });
            }
            zwp_text_input_v3::Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } if input.active => {
                let text = text.unwrap_or_default();
                if text.len() > MAX_TEXT_BYTES {
                    input.pending = PendingText::default();
                    state.events.push(OverlayEvent::InputReset {
                        source: Some(source),
                        scope: crate::InputResetScope::Keyboard,
                        reason: crate::InputResetReason::Cancelled,
                    });
                    return;
                }
                let cursor = if cursor_begin >= 0
                    && cursor_end >= 0
                    && text.is_char_boundary(cursor_begin as usize)
                    && text.is_char_boundary(cursor_end as usize)
                {
                    Some(
                        (cursor_begin.min(cursor_end) as usize)
                            ..(cursor_begin.max(cursor_end) as usize),
                    )
                } else {
                    None
                };
                input.pending.preedit = Some((text, cursor));
            }
            zwp_text_input_v3::Event::CommitString { text } if input.active => {
                let text = text.unwrap_or_default();
                if text.len() <= MAX_TEXT_BYTES {
                    input.pending.commit = Some(text);
                } else {
                    input.pending = PendingText::default();
                    state.events.push(OverlayEvent::InputReset {
                        source: Some(source),
                        scope: crate::InputResetScope::Keyboard,
                        reason: crate::InputResetReason::Cancelled,
                    });
                }
            }
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } if input.active => {
                input.pending.delete = Some((before_length, after_length));
            }
            zwp_text_input_v3::Event::Done { serial: _ } if input.active => {
                // Even an older serial carries valid edits (text-input-v3).
                // Apply them without replacing the application's newer context.
                if let Some(keyboard) = state.keyboard_states.get_mut(seat) {
                    keyboard.reset_compose();
                }
                let pending = std::mem::take(&mut input.pending);
                state.events.push(OverlayEvent::TextPreedit {
                    source,
                    text: String::new(),
                    cursor: None,
                });
                if let Some((before, after)) = pending.delete {
                    state.events.push(OverlayEvent::TextDeleteSurrounding {
                        source,
                        before,
                        after,
                    });
                }
                if let Some(text) = pending.commit.filter(|s| !s.is_empty()) {
                    state
                        .events
                        .push(OverlayEvent::TextCommitted { source, text });
                }
                if let Some((text, cursor)) = pending.preedit {
                    state.events.push(OverlayEvent::TextPreedit {
                        source,
                        text,
                        cursor,
                    });
                }
            }
            _ => {}
        }
    }
}
delegate_noop!(State: ignore zwp_text_input_manager_v3::ZwpTextInputManagerV3);

#[cfg(test)]
mod keymap_tests {
    use super::*;
    use std::io::{Seek, Write};

    #[test]
    fn keymap_reads_from_zero_without_mutating_shared_descriptor_position() {
        let mut file = File::from(
            rustix::fs::memfd_create(c"liboverlay-keymap-test", rustix::fs::MemfdFlags::CLOEXEC)
                .unwrap(),
        );
        let text = b"xkb_keymap { fixture };\0";
        file.write_all(text).unwrap();
        let mut duplicate = file.try_clone().unwrap();
        assert_eq!(file.stream_position().unwrap(), text.len() as u64);
        assert_eq!(
            read_keymap(&file, text.len() as u32).unwrap(),
            &text[..text.len() - 1]
        );
        assert_eq!(
            read_keymap(&duplicate, text.len() as u32).unwrap(),
            &text[..text.len() - 1]
        );
        assert_eq!(duplicate.stream_position().unwrap(), text.len() as u64);
        assert!(read_keymap(&file, 0).is_err());
        assert!(read_keymap(&file, text.len() as u32 + 1).is_err());
        assert!(read_keymap(&file, text.len() as u32 - 1).is_err());
        assert!(read_keymap(&file, MAX_KEYMAP_BYTES as u32 + 1).is_err());
    }
}
