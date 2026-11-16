use super::XConnection;
use crate::{input::touch::TouchTracker, InputId, OverlayEvent, Result};
use x11rb::{
    connection::RequestConnection,
    protocol::{
        xinput::{self, ConnectionExt as _},
        Event,
    },
};

pub(super) struct TouchInput {
    window: u32,
    contacts: TouchTracker,
}

impl TouchInput {
    pub(super) fn new(conn: &XConnection, root: u32, window: u32) -> Result<Option<Self>> {
        if conn
            .extension_information(xinput::X11_EXTENSION_NAME)?
            .is_none()
        {
            return Ok(None);
        }
        let version = conn.xinput_xi_query_version(2, 2)?.reply()?;
        if (version.major_version, version.minor_version) < (2, 2) {
            return Ok(None);
        }
        conn.xinput_xi_select_events(
            window,
            &[xinput::EventMask {
                deviceid: 1,
                mask: vec![
                    xinput::XIEventMask::TOUCH_BEGIN
                        | xinput::XIEventMask::TOUCH_UPDATE
                        | xinput::XIEventMask::TOUCH_END,
                ],
            }],
        )?
        .check()?;
        // Only hierarchy metadata is selected on the root; input coordinates
        // and contact events are selected exclusively on our own window.
        conn.xinput_xi_select_events(
            root,
            &[xinput::EventMask {
                deviceid: 0,
                mask: vec![xinput::XIEventMask::HIERARCHY | xinput::XIEventMask::DEVICE_CHANGED],
            }],
        )?
        .check()?;
        Ok(Some(Self {
            window,
            contacts: TouchTracker::default(),
        }))
    }
    pub(super) fn reset(&mut self) -> Vec<OverlayEvent> {
        self.contacts.cancel(None, None)
    }
    pub(super) fn event(&mut self, event: &Event) -> Vec<OverlayEvent> {
        let mut events = Vec::new();
        match event {
            Event::XinputTouchBegin(e) if e.event == self.window => {
                events.extend(self.contacts.begin(
                    source(e),
                    i64::from(e.detail),
                    f64::from(e.event_x) / 65536.,
                    f64::from(e.event_y) / 65536.,
                ));
            }
            Event::XinputTouchUpdate(e) if e.event == self.window => {
                if let Some(event) = self.contacts.motion(
                    source(e),
                    i64::from(e.detail),
                    f64::from(e.event_x) / 65536.,
                    f64::from(e.event_y) / 65536.,
                ) {
                    events.push(event);
                }
            }
            Event::XinputTouchEnd(e) if e.event == self.window => {
                self.contacts.motion(
                    source(e),
                    i64::from(e.detail),
                    f64::from(e.event_x) / 65536.,
                    f64::from(e.event_y) / 65536.,
                );
                if let Some(event) = self.contacts.end(source(e), i64::from(e.detail)) {
                    events.push(event);
                }
            }
            Event::XinputHierarchy(e) => {
                for info in &e.infos {
                    let flags = xinput::HierarchyMask::MASTER_REMOVED
                        | xinput::HierarchyMask::SLAVE_REMOVED
                        | xinput::HierarchyMask::DEVICE_DISABLED
                        | xinput::HierarchyMask::SLAVE_DETACHED
                        | xinput::HierarchyMask::SLAVE_ATTACHED;
                    if u32::from(info.flags & flags) != 0 {
                        events.extend(self.contacts.cancel(Some(u32::from(info.deviceid)), None));
                        events.extend(self.contacts.cancel(None, Some(u32::from(info.deviceid))));
                    }
                }
            }
            Event::XinputDeviceChanged(e) if e.reason == xinput::ChangeReason::DEVICE_CHANGE => {
                events.extend(self.contacts.cancel(None, Some(u32::from(e.sourceid))));
            }
            _ => {}
        }
        events
    }
}

fn source(event: &xinput::TouchBeginEvent) -> InputId {
    InputId {
        seat: u32::from(event.deviceid),
        device: Some(u32::from(event.sourceid)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::x11_utils::{Serialize, TryParse};
    #[test]
    fn wire_contacts_are_local_and_keep_source_identity_through_cancellation() {
        let mut input = TouchInput {
            window: 44,
            contacts: TouchTracker::default(),
        };
        let mut event = xinput::TouchBeginEvent {
            response_type: 35,
            event_type: 18,
            event: 44,
            deviceid: 2,
            sourceid: 8,
            detail: 7,
            event_x: 12 * 65536 + 32768,
            event_y: 20 * 65536 + 16384,
            ..Default::default()
        };
        event.length = (event.serialize().len() as u32 - 32) / 4;
        let bytes = event.serialize();
        let (decoded, _) = xinput::TouchBeginEvent::try_parse(&bytes).unwrap();
        let events = input.event(&Event::XinputTouchBegin(decoded));
        assert!(
            matches!(events[0],OverlayEvent::Touch { source:InputId {seat:2,device:Some(8)},id:7,phase:crate::TouchPhase::Started,x,y } if x==12.5 && y==20.25)
        );
        event.event = 99;
        assert!(input
            .event(&Event::XinputTouchBegin(event.clone()))
            .is_empty());
        event.event = 44;
        event.sourceid = 9;
        input.event(&Event::XinputTouchBegin(event));
        let removed = xinput::HierarchyEvent {
            infos: vec![xinput::HierarchyInfo {
                deviceid: 8,
                flags: xinput::HierarchyMask::DEVICE_DISABLED,
                ..Default::default()
            }],
            ..Default::default()
        };
        let events = input.event(&Event::XinputHierarchy(removed));
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            OverlayEvent::Touch {
                source: InputId {
                    device: Some(8),
                    ..
                },
                phase: crate::TouchPhase::Cancelled,
                ..
            }
        ));
        assert_eq!(input.reset().len(), 1);
    }
}
