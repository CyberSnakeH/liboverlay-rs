use crate::{
    ButtonState, InputId, InputResetReason, KeyboardInput, KeyboardModifiers, OverlayEvent,
};
use kbvm::{
    lookup::LookupTable,
    xkb::{
        compose::{ComposeTable, FeedResult, State as ComposeState},
        diagnostic::{Diagnostic, DiagnosticHandler},
        Context, Keymap,
    },
    Components, GroupIndex, Keycode, Keysym, ModifierMask,
};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

#[cfg(feature = "wayland")]
pub(crate) const MAX_KEYMAP_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_TEXT_BYTES: usize = 65536;

pub(crate) struct IgnoreDiagnostics;
impl DiagnosticHandler for IgnoreDiagnostics {
    fn filter(&self, _: kbvm::xkb::diagnostic::DiagnosticKind, _: bool) -> bool {
        false
    }
    fn handle(&mut self, _: Diagnostic) {}
}

pub(crate) struct Keyboard {
    pub source: InputId,
    pub table: Option<LookupTable>,
    pub components: Components,
    pub focused: bool,
    compose: Option<(ComposeTable, ComposeState)>,
    preview: String,
    pressed: BTreeSet<u32>,
    repeat_rate: u32,
    repeat_delay: Duration,
    repeat: Option<(u32, Instant)>,
}

impl Keyboard {
    pub fn new(source: InputId) -> Self {
        let context = Context::default();
        let table = context
            .compose_table_builder()
            .build(IgnoreDiagnostics)
            .or_else(|| {
                let mut builder = context.compose_table_builder();
                builder.locale("en_US.UTF-8");
                builder.build(IgnoreDiagnostics)
            });
        Self::from_compose(source, table)
    }
    fn from_compose(source: InputId, table: Option<ComposeTable>) -> Self {
        Self {
            source,
            table: None,
            components: Components::default(),
            focused: false,
            compose: table.map(|table| {
                let state = table.create_state();
                (table, state)
            }),
            preview: String::new(),
            pressed: BTreeSet::new(),
            repeat_rate: 0,
            repeat_delay: Duration::from_millis(600),
            repeat: None,
        }
    }
    pub fn has_compose(&self) -> bool {
        self.compose.is_some()
    }
    pub fn set_keymap(&mut self, map: Keymap) {
        self.table = Some(map.to_builder().build_lookup_table());
        self.reset();
    }
    pub fn modifiers(&self) -> KeyboardModifiers {
        KeyboardModifiers {
            depressed: self.components.mods_pressed.0,
            latched: self.components.mods_latched.0,
            locked: self.components.mods_locked.0,
            effective: self.components.mods.0,
            group: self.components.group.0,
        }
    }
    pub fn update_modifiers(
        &mut self,
        depressed: u32,
        latched: u32,
        locked: u32,
        group: u32,
    ) -> OverlayEvent {
        self.components.mods_pressed = ModifierMask(depressed);
        self.components.mods_latched = ModifierMask(latched);
        self.components.mods_locked = ModifierMask(locked);
        self.components.group_locked = GroupIndex(group);
        self.components.update_effective();
        OverlayEvent::ModifiersChanged {
            source: self.source,
            modifiers: self.modifiers(),
        }
    }
    pub fn focus(&mut self, focused: bool) -> Vec<OverlayEvent> {
        self.reset();
        self.focused = focused;
        let mut events = vec![OverlayEvent::KeyboardFocus {
            source: self.source,
            focused,
        }];
        if !focused {
            events.push(OverlayEvent::TextPreedit {
                source: self.source,
                text: String::new(),
                cursor: None,
            });
            events.push(OverlayEvent::InputReset {
                source: Some(self.source),
                scope: crate::InputResetScope::Keyboard,
                reason: InputResetReason::FocusLost,
            });
        }
        events
    }
    #[cfg(feature = "wayland")]
    pub fn enter_pressed(&mut self, keys: impl Iterator<Item = u32>) {
        self.pressed.extend(keys.take(crate::MAX_PRESSED_KEYS));
    }
    pub fn reset(&mut self) {
        self.pressed.clear();
        self.repeat = None;
        self.reset_compose();
    }
    pub fn reset_compose(&mut self) {
        self.preview.clear();
        if let Some((table, state)) = &mut self.compose {
            *state = table.create_state();
        }
    }
    #[cfg(any(feature = "wayland", test))]
    pub fn repeat_info(&mut self, rate: i32, delay: i32) {
        self.repeat_rate = rate.clamp(0, 1000) as u32;
        self.repeat_delay = Duration::from_millis(delay.clamp(0, 60000) as u64);
        if self.repeat_rate == 0 {
            self.repeat = None;
        }
    }
    #[cfg(any(feature = "wayland", test))]
    pub fn repeat_wait(&self) -> Option<Duration> {
        self.repeat
            .map(|(_, next)| next.saturating_duration_since(Instant::now()))
    }
    #[cfg(any(feature = "wayland", test))]
    pub fn tick(&mut self, text_enabled: bool) -> Vec<OverlayEvent> {
        let Some((code, next)) = self.repeat else {
            return Vec::new();
        };
        if Instant::now() < next || self.repeat_rate == 0 {
            return Vec::new();
        }
        // Emit at most one repeat after a delayed wake; never flood the queue.
        self.repeat = Some((
            code,
            Instant::now() + Duration::from_secs_f64(1. / f64::from(self.repeat_rate)),
        ));
        self.key(code, ButtonState::Pressed, true, text_enabled, true)
    }
    pub fn key(
        &mut self,
        code: u32,
        state: ButtonState,
        repeat: bool,
        text_enabled: bool,
        local_text: bool,
    ) -> Vec<OverlayEvent> {
        if !self.focused {
            return Vec::new();
        }
        if code > u32::MAX - 8 {
            return vec![OverlayEvent::InputReset {
                source: Some(self.source),
                scope: crate::InputResetScope::Keyboard,
                reason: InputResetReason::Cancelled,
            }];
        }
        let mut events = vec![OverlayEvent::Key { code, state }];
        if state == ButtonState::Pressed
            && !self.pressed.contains(&code)
            && self.pressed.len() >= crate::MAX_PRESSED_KEYS
        {
            self.reset();
            events.insert(
                0,
                OverlayEvent::InputReset {
                    source: Some(self.source),
                    scope: crate::InputResetScope::Keyboard,
                    reason: InputResetReason::KeyLimit,
                },
            );
        }
        let repeated = repeat || (state == ButtonState::Pressed && self.pressed.contains(&code));
        match state {
            ButtonState::Pressed => {
                self.pressed.insert(code);
                if !repeat
                    && self.repeat_rate > 0
                    && self
                        .table
                        .as_ref()
                        .is_some_and(|t| t.repeats(Keycode::from_evdev(code)))
                {
                    self.repeat = Some((code, Instant::now() + self.repeat_delay));
                }
            }
            ButtonState::Released => {
                self.pressed.remove(&code);
                if self.repeat.is_some_and(|(current, _)| current == code) {
                    self.repeat = None;
                }
            }
        }
        let Some(table) = &self.table else {
            return events;
        };
        let lookup = table.lookup(
            self.components.group,
            self.components.mods,
            Keycode::from_evdev(code),
        );
        let unconsumed_modifiers = lookup.remaining_mods().0;
        let keys: Vec<_> = lookup
            .into_iter()
            .take(32)
            .map(|key| (key.keysym(), key.char()))
            .collect();
        let text: String = keys
            .iter()
            .filter_map(|(_, c)| *c)
            .filter(|c| !c.is_control())
            .collect();
        let input = KeyboardInput {
            source: self.source,
            code,
            keysyms: keys.iter().map(|(sym, _)| sym.0).collect(),
            text: (!text.is_empty()).then_some(text),
            state,
            repeat: repeated,
            modifiers: self.modifiers(),
            unconsumed_modifiers,
        };
        let composed = if text_enabled && local_text {
            self.compose_input(&input)
        } else {
            Vec::new()
        };
        events.push(OverlayEvent::KeyboardInput(input));
        events.extend(composed);
        events
    }
    pub fn compose_input(&mut self, input: &KeyboardInput) -> Vec<OverlayEvent> {
        if input.state != ButtonState::Pressed || input.unconsumed_modifiers & (4 | 8 | 64) != 0 {
            return Vec::new();
        }
        input
            .keysyms
            .iter()
            .flat_map(|&sym| {
                let sym = Keysym(sym);
                self.compose_key(sym, sym.char())
            })
            .collect()
    }
    fn compose_key(&mut self, sym: Keysym, character: Option<char>) -> Vec<OverlayEvent> {
        if sym.is_modifier() {
            return Vec::new();
        }
        if let Some((table, state)) = &mut self.compose {
            match table.feed(state, sym) {
                Some(FeedResult::Pending) => {
                    if let Some(c) = sym.char().filter(|c| !c.is_control()) {
                        if self.preview.len() < 128 {
                            self.preview.push(c);
                        }
                    }
                    return vec![OverlayEvent::TextPreedit {
                        source: self.source,
                        text: self.preview.clone(),
                        cursor: Some(self.preview.len()..self.preview.len()),
                    }];
                }
                Some(FeedResult::Aborted) => {
                    self.preview.clear();
                    return vec![OverlayEvent::TextPreedit {
                        source: self.source,
                        text: String::new(),
                        cursor: None,
                    }];
                }
                Some(FeedResult::Composed { string, keysym }) => {
                    let text = string
                        .map(str::to_owned)
                        .or_else(|| keysym.and_then(Keysym::char).map(|c| c.to_string()))
                        .unwrap_or_default();
                    self.preview.clear();
                    let mut events = vec![OverlayEvent::TextPreedit {
                        source: self.source,
                        text: String::new(),
                        cursor: None,
                    }];
                    if !text.is_empty() && text.len() <= MAX_TEXT_BYTES {
                        events.push(OverlayEvent::TextCommitted {
                            source: self.source,
                            text,
                        });
                    }
                    return events;
                }
                None => {}
            }
        }
        character
            .filter(|c| !c.is_control())
            .map(|c| {
                vec![OverlayEvent::TextCommitted {
                    source: self.source,
                    text: c.to_string(),
                }]
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn keyboard() -> Keyboard {
        let context = Context::default();
        let map = context
            .keymap_from_bytes(
                IgnoreDiagnostics,
                None,
                include_bytes!("../../tests/fixtures/input-keymap.xkb"),
            )
            .unwrap();
        let mut compose = context.compose_table_builder();
        compose
            .buffer("<dead_acute> <e> : \"é\" eacute\n<dead_acute> <dead_acute> : \"´\" acute\n");
        let mut keyboard = Keyboard::from_compose(
            InputId {
                seat: 1,
                device: None,
            },
            compose.build(IgnoreDiagnostics),
        );
        keyboard.set_keymap(map);
        keyboard.focus(true);
        keyboard
    }
    fn commits(events: Vec<OverlayEvent>) -> Vec<String> {
        events
            .into_iter()
            .filter_map(|e| {
                if let OverlayEvent::TextCommitted { text, .. } = e {
                    Some(text)
                } else {
                    None
                }
            })
            .collect()
    }
    #[test]
    fn layouts_shift_caps_and_control_use_the_current_keymap() {
        let mut keyboard = keyboard();
        assert_eq!(
            commits(keyboard.key(16, ButtonState::Pressed, false, true, true)),
            ["q"]
        );
        keyboard.update_modifiers(1, 0, 0, 1);
        let events = keyboard.key(16, ButtonState::Pressed, false, true, true);
        assert!(events.iter().any(|e|matches!(e,OverlayEvent::KeyboardInput(input) if input.text.as_deref()==Some("A") && input.modifiers.group==1 && input.unconsumed_modifiers==0)));
        assert_eq!(commits(events), ["A"]);
        keyboard.update_modifiers(0, 0, 2, 0);
        assert_eq!(
            commits(keyboard.key(30, ButtonState::Pressed, false, true, true)),
            ["A"]
        );
        keyboard.update_modifiers(4, 0, 0, 0);
        assert!(commits(keyboard.key(30, ButtonState::Pressed, false, true, true)).is_empty());
    }
    #[test]
    fn composition_commits_once_and_focus_loss_cancels_a_dead_key() {
        let mut keyboard = keyboard();
        assert!(commits(keyboard.key(40, ButtonState::Pressed, false, true, true)).is_empty());
        assert_eq!(
            commits(keyboard.key(18, ButtonState::Pressed, false, true, true)),
            ["é"]
        );
        keyboard.key(40, ButtonState::Pressed, false, true, true);
        keyboard.focus(false);
        keyboard.focus(true);
        assert_eq!(
            commits(keyboard.key(18, ButtonState::Pressed, false, true, true)),
            ["e"]
        );
        assert_eq!(
            commits(keyboard.key(57, ButtonState::Pressed, false, true, true)),
            ["中"]
        );
    }
    #[test]
    fn excess_pressed_keys_reset_keyboard_state_without_growing_storage() {
        let mut keyboard = keyboard();
        for code in 0..crate::MAX_PRESSED_KEYS as u32 {
            keyboard.key(code, ButtonState::Pressed, false, false, false);
        }
        let events = keyboard.key(300, ButtonState::Pressed, false, false, false);
        assert!(matches!(
            events.first(),
            Some(OverlayEvent::InputReset {
                scope: crate::InputResetScope::Keyboard,
                reason: InputResetReason::KeyLimit,
                ..
            })
        ));
        assert_eq!(keyboard.pressed.len(), 1);
        assert!(keyboard.pressed.contains(&300));
    }
    #[test]
    fn repeat_is_bounded_and_stops_on_release_or_focus_loss() {
        let mut keyboard = keyboard();
        keyboard.repeat_info(25, 20);
        keyboard.key(16, ButtonState::Pressed, false, true, true);
        keyboard.repeat = Some((16, Instant::now() - Duration::from_millis(1)));
        assert!(keyboard
            .tick(true)
            .iter()
            .any(|e| matches!(e,OverlayEvent::KeyboardInput(input) if input.repeat)));
        assert!(keyboard.tick(true).is_empty());
        keyboard.key(16, ButtonState::Released, false, true, true);
        assert!(keyboard.repeat_wait().is_none());
        keyboard.key(16, ButtonState::Pressed, false, true, true);
        keyboard.focus(false);
        assert!(keyboard.repeat_wait().is_none());
        assert!(keyboard
            .key(u32::MAX, ButtonState::Pressed, false, true, true)
            .is_empty());
    }
}
