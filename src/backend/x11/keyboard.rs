use super::XConnection;
use crate::{input::keyboard::Keyboard, InputId, InputResetReason, OverlayEvent, Result};
use kbvm::xkb::x11::KbvmX11Ext;
use x11rb::protocol::{
    xkb::{self, ConnectionExt as _},
    Event,
};

pub(super) struct KeyboardInput {
    pub keyboard: Keyboard,
    device: u16,
}

impl KeyboardInput {
    pub(super) fn new(conn: &XConnection) -> Result<Self> {
        conn.setup_xkb_extension().map_err(error)?;
        let device = conn.get_xkb_core_device_id().map_err(error)?;
        let map = conn.get_xkb_keymap(device).map_err(error)?;
        let mut keyboard = Keyboard::new(InputId {
            seat: u32::from(device),
            device: None,
        });
        keyboard.set_keymap(map);
        keyboard.components = conn.get_xkb_components(device).map_err(error)?;
        subscribe(conn, device)?;
        Ok(Self { keyboard, device })
    }
    pub(super) fn event(&mut self, conn: &XConnection, event: &Event) -> Result<Vec<OverlayEvent>> {
        let mut events = Vec::new();
        match event {
            Event::XkbStateNotify(e) if u16::from(e.device_id) == self.device => {
                let event = self.keyboard.update_modifiers(
                    u32::from(e.base_mods),
                    u32::from(e.latched_mods),
                    u32::from(e.locked_mods),
                    u32::from(e.group),
                );
                if self.keyboard.focused {
                    events.push(event);
                }
            }
            Event::XkbMapNotify(e) if u16::from(e.device_id) == self.device => {
                self.reload(conn)?;
                events.push(OverlayEvent::InputReset {
                    source: Some(self.keyboard.source),
                    scope: crate::InputResetScope::Keyboard,
                    reason: InputResetReason::KeymapChanged,
                });
            }
            Event::XkbNewKeyboardNotify(e)
                if u16::from(e.device_id) == self.device
                    || u16::from(e.old_device_id) == self.device =>
            {
                let previous = self.keyboard.source;
                self.reload(conn)?;
                if self.keyboard.source != previous {
                    events.push(OverlayEvent::InputReset {
                        source: Some(previous),
                        scope: crate::InputResetScope::Keyboard,
                        reason: InputResetReason::DeviceRemoved,
                    });
                }
                events.push(OverlayEvent::InputReset {
                    source: Some(self.keyboard.source),
                    scope: crate::InputResetScope::Keyboard,
                    reason: InputResetReason::KeymapChanged,
                });
            }
            _ => {}
        }
        Ok(events)
    }
    fn reload(&mut self, conn: &XConnection) -> Result<()> {
        let device = conn.get_xkb_core_device_id().map_err(error)?;
        if device != self.device {
            subscribe(conn, device)?;
        }
        self.keyboard.source.seat = u32::from(device);
        self.keyboard
            .set_keymap(conn.get_xkb_keymap(device).map_err(error)?);
        self.keyboard.components = conn.get_xkb_components(device).map_err(error)?;
        self.device = device;
        Ok(())
    }
    pub(super) fn core_modifiers(&mut self, state: u16) {
        // Core event state includes the event-time modifier mask and XKB group.
        // Preserve the separately negotiated depressed/latched/locked masks.
        self.keyboard.components.mods = kbvm::ModifierMask(u32::from(state & 0xff));
        self.keyboard.components.group = kbvm::GroupIndex(u32::from((state >> 13) & 3));
    }
}

fn subscribe(conn: &XConnection, device: u16) -> Result<()> {
    conn.xkb_select_events(
        device,
        xkb::EventType::default(),
        xkb::EventType::NEW_KEYBOARD_NOTIFY
            | xkb::EventType::MAP_NOTIFY
            | xkb::EventType::STATE_NOTIFY,
        xkb::MapPart::KEY_TYPES
            | xkb::MapPart::KEY_SYMS
            | xkb::MapPart::MODIFIER_MAP
            | xkb::MapPart::VIRTUAL_MODS,
        xkb::MapPart::KEY_TYPES
            | xkb::MapPart::KEY_SYMS
            | xkb::MapPart::MODIFIER_MAP
            | xkb::MapPart::VIRTUAL_MODS,
        &xkb::SelectEventsAux::default(),
    )?
    .check()?;
    conn.xkb_per_client_flags(
        device,
        xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
        xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
        xkb::BoolCtrl::default(),
        xkb::BoolCtrl::default(),
        xkb::BoolCtrl::default(),
    )?
    .reply()?;
    Ok(())
}

fn error(error: kbvm::xkb::x11::X11Error) -> crate::OverlayError {
    crate::error::backend_error("X11", format!("XKB: {error}"))
}
