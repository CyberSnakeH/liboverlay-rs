//! XIM transport and UTF-8 preedit. All requests use the overlay's own connection.
use super::XConnection;
use crate::{
    input::keyboard::MAX_TEXT_BYTES, InputId, KeyboardInput, OverlayEvent, TextInputState,
};
use std::{
    collections::{BTreeSet, VecDeque},
    rc::Rc,
    time::{Duration, Instant},
};
use x11rb::protocol::{
    xproto::{self, ConnectionExt as _},
    Event,
};
use xim::{
    x11rb::{HasConnection, X11rbClient},
    AttributeName, Client, ClientCore, ClientError, ClientHandler, InputStyle, Request,
};

pub(super) struct ConnectionRef(Rc<XConnection>);
impl HasConnection for ConnectionRef {
    type Connection = XConnection;
    fn conn(&self) -> &XConnection {
        &self.0
    }
}
type ClientConnection = X11rbClient<ConnectionRef>;
type Result<T> = std::result::Result<T, ClientError>;

struct Packet {
    event: xproto::KeyPressEvent,
    translated: Option<KeyboardInput>,
    forward: bool,
}

struct AsyncPacket {
    packet: Packet,
    release: Option<KeyboardInput>,
}

pub(super) struct Ime {
    conn: Rc<XConnection>,
    client: ClientConnection,
    handler: Handler,
    protocol: u32,
    connect_atom: u32,
    owner: u32,
    transport: Option<u32>,
    client_window: Option<u32>,
    queue: VecDeque<Packet>,
    current: Option<Packet>,
    asynchronous: VecDeque<AsyncPacket>,
    asynchronous_held: BTreeSet<u8>,
    deadline: Option<Instant>,
    pub unhandled: Vec<KeyboardInput>,
}

impl Ime {
    pub(super) fn new(
        conn: Rc<XConnection>,
        screen: usize,
        window: u32,
        source: InputId,
        state: TextInputState,
    ) -> Result<Option<Self>> {
        if std::env::var("XMODIFIERS")
            .ok()
            .is_none_or(|v| !v.starts_with("@im=") || v == "@im=none" || v == "@im=")
        {
            return Ok(None);
        }
        let protocol = conn.intern_atom(false, b"_XIM_PROTOCOL")?.reply()?.atom;
        let connect_atom = conn.intern_atom(false, b"_XIM_XCONNECT")?.reply()?.atom;
        let name = std::env::var("XMODIFIERS").unwrap_or_default();
        let selection = conn
            .intern_atom(
                false,
                format!("@server={}", name.trim_start_matches("@im=")).as_bytes(),
            )?
            .reply()?
            .atom;
        let owner = conn.get_selection_owner(selection)?.reply()?.owner;
        if owner == x11rb::NONE {
            return Ok(None);
        }
        conn.change_window_attributes(
            owner,
            &xproto::ChangeWindowAttributesAux::new()
                .event_mask(xproto::EventMask::STRUCTURE_NOTIFY),
        )?
        .check()?;
        let client = match ClientConnection::init(ConnectionRef(conn.clone()), screen, None) {
            Ok(client) => client,
            Err(ClientError::NoXimServer | ClientError::InvalidReply) => return Ok(None),
            Err(error) => return Err(error),
        };
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|v| !v.is_empty() && !v.contains('\0') && v.len() <= 128)
            .unwrap_or_else(|| "en_US.UTF-8".into());
        Ok(Some(Self {
            conn,
            client,
            protocol,
            connect_atom,
            owner,
            transport: None,
            client_window: None,
            handler: Handler {
                source,
                window,
                state,
                locale,
                im: 0,
                ic: 0,
                ready: false,
                focused: false,
                active: false,
                forward_mask: 1,
                synchronous_mask: 0,
                events: Vec::new(),
                forwarded: Vec::new(),
                preedit: String::new(),
                caret: 0,
                resetting: 0,
            },
            queue: VecDeque::new(),
            current: None,
            asynchronous: VecDeque::new(),
            asynchronous_held: BTreeSet::new(),
            deadline: Some(Instant::now() + Duration::from_secs(5)),
            unhandled: Vec::new(),
        }))
    }
    pub(super) fn wait(&self) -> Option<Duration> {
        self.deadline
            .map(|t| t.saturating_duration_since(Instant::now()))
    }
    pub(super) fn set_source(&mut self, source: InputId) {
        self.handler.source = source;
    }
    pub(super) fn windows(&self) -> impl Iterator<Item = u32> {
        std::iter::once(self.owner)
            .chain(self.transport)
            .chain(self.client_window)
    }
    pub(super) fn expired(&self) -> bool {
        self.deadline.is_some_and(|t| Instant::now() >= t)
    }
    pub(super) fn events(&mut self) -> Vec<OverlayEvent> {
        std::mem::take(&mut self.handler.events)
    }
    pub(super) fn focus(&mut self, focused: bool) -> Result<()> {
        self.handler.focused = focused;
        if !focused {
            self.reset()?;
        }
        self.handler.sync_focus(&mut self.client)
    }
    pub(super) fn configure(&mut self, state: TextInputState) -> Result<()> {
        if !state.enabled || state.change_cause == crate::TextChangeCause::Other {
            self.reset()?;
        }
        self.handler.state = state;
        self.handler.sync_focus(&mut self.client)
    }
    pub(super) fn reset(&mut self) -> Result<()> {
        self.queue.clear();
        self.asynchronous.clear();
        self.asynchronous_held.clear();
        self.unhandled.clear();
        self.handler.preedit.clear();
        self.handler.caret = 0;
        if let Some(packet) = &mut self.current {
            packet.translated = None;
        }
        if self.handler.ready {
            self.handler.resetting = self.handler.resetting.saturating_add(1);
            self.deadline = Some(Instant::now() + Duration::from_secs(5));
            self.client.send_req(Request::ResetIc {
                input_method_id: self.handler.im,
                input_context_id: self.handler.ic,
            })?;
        }
        self.handler.events.push(OverlayEvent::TextPreedit {
            source: self.handler.source,
            text: String::new(),
            cursor: None,
        });
        Ok(())
    }
    pub(super) fn forward(
        &mut self,
        event: xproto::KeyPressEvent,
        translated: Option<KeyboardInput>,
    ) -> Result<bool> {
        let kind = event.response_type & 0x7f;
        let mask = if kind == xproto::KEY_PRESS_EVENT {
            1
        } else {
            2
        };
        if !self.handler.ready || !self.handler.active {
            return Ok(false);
        }
        let forward = self.handler.forward_mask & mask != 0;
        if !forward && kind == xproto::KEY_RELEASE_EVENT {
            if self.asynchronous_held.remove(&event.detail) {
                return Ok(false);
            }
            if let Some(packet) = self.asynchronous.iter_mut().rev().find(|p| {
                p.packet.event.detail == event.detail
                    && p.packet.event.response_type & 0x7f == xproto::KEY_PRESS_EVENT
            }) {
                packet.release = translated;
                return Ok(true);
            }
        }
        if !forward
            && self.current.is_none()
            && self.queue.is_empty()
            && self.handler.resetting == 0
        {
            return Ok(false);
        }
        if self.queue.len() >= 256 {
            return Err(ClientError::InvalidReply);
        }
        self.queue.push_back(Packet {
            event,
            translated,
            forward,
        });
        self.send_next()?;
        Ok(true)
    }
    fn send_next(&mut self) -> Result<()> {
        if self.current.is_some() || self.handler.resetting > 0 {
            return Ok(());
        }
        while let Some(packet) = self.queue.pop_front() {
            if !packet.forward {
                if let Some(input) = packet.translated {
                    self.unhandled.push(input);
                }
                continue;
            }
            // Only the event classes selected by the IM server are synchronous.
            // IMdkit-based servers do not acknowledge arbitrary synchronous flags.
            let mask = if packet.event.response_type & 0x7f == xproto::KEY_PRESS_EVENT {
                1
            } else {
                2
            };
            let synchronous = self.handler.synchronous_mask & mask != 0;
            self.client.forward_event(
                self.handler.im,
                self.handler.ic,
                if synchronous {
                    xim::ForwardEventFlag::SYNCHRONOUS
                } else {
                    xim::ForwardEventFlag::empty()
                },
                &packet.event,
            )?;
            if synchronous {
                self.current = Some(packet);
                self.deadline = Some(Instant::now() + Duration::from_secs(5));
                return Ok(());
            }
            // Consumed asynchronous keys have no reply. Retain bounded snapshots
            // for late returned keys without making progress depend on a reply.
            if self.asynchronous.len() == 256 {
                self.asynchronous.pop_front();
            }
            self.asynchronous.push_back(AsyncPacket {
                packet,
                release: None,
            });
        }
        if self.handler.ready {
            self.deadline = None;
        }
        Ok(())
    }
    pub(super) fn filter(&mut self, event: &Event) -> Result<bool> {
        if let Event::DestroyNotify(event) = event {
            if event.window == self.owner || Some(event.window) == self.transport {
                return Err(ClientError::NoXimServer);
            }
        }
        if let Event::Error(error) = event {
            if self.windows().any(|window| window == error.bad_value) {
                return Err(ClientError::InvalidReply);
            }
        }
        let mut consumed = false;
        let mut request = None;
        let mut property = None;
        if let Event::ClientMessage(message) = event {
            if Some(message.window) == self.client_window
                && message.type_ == self.connect_atom
                && message.format == 32
            {
                let window = message.data.as_data32()[0];
                self.conn
                    .change_window_attributes(
                        window,
                        &xproto::ChangeWindowAttributesAux::new()
                            .event_mask(xproto::EventMask::STRUCTURE_NOTIFY),
                    )?
                    .check()?;
                self.transport = Some(window);
            }
            if Some(message.window) == self.client_window && message.type_ == self.protocol {
                let bytes = if message.format == 8 {
                    message.data.as_data8().to_vec()
                } else if message.format == 32 {
                    let [_, atom, ..] = message.data.as_data32();
                    let reply = self
                        .conn
                        .get_property(
                            false,
                            message.window,
                            atom,
                            xproto::AtomEnum::ANY,
                            0,
                            262144,
                        )?
                        .reply()?;
                    if reply.bytes_after != 0 || reply.value.is_empty() {
                        return Err(ClientError::InvalidReply);
                    }
                    property = Some((message.window, atom));
                    reply.value
                } else {
                    return Err(ClientError::InvalidReply);
                };
                request = Some(xim::read::<Request>(&bytes)?);
            }
        }
        match request {
            Some(Request::Commit {
                input_method_id,
                input_context_id,
                data,
            }) => {
                let (text, synchronous) = match data {
                    xim::CommitData::Chars {
                        commited,
                        syncronous,
                    }
                    | xim::CommitData::Both {
                        commited,
                        syncronous,
                        ..
                    } => (
                        xim_ctext::compound_text_to_utf8(&commited)
                            .map_err(|_| ClientError::InvalidReply)?,
                        syncronous,
                    ),
                    xim::CommitData::Keysym { keysym, syncronous } => (
                        kbvm::Keysym(keysym)
                            .char()
                            .filter(|c| !c.is_control())
                            .map(|c| c.to_string())
                            .unwrap_or_default(),
                        syncronous,
                    ),
                };
                if text.len() > MAX_TEXT_BYTES {
                    return Err(ClientError::InvalidReply);
                }
                self.handler.handle_commit(
                    &mut self.client,
                    input_method_id,
                    input_context_id,
                    &text,
                )?;
                if synchronous {
                    self.client.send_req(Request::SyncReply {
                        input_method_id,
                        input_context_id,
                    })?;
                }
                consumed = true;
            }
            Some(Request::PreeditDraw {
                input_method_id,
                input_context_id,
                caret,
                chg_first,
                chg_length,
                status,
                preedit_string,
                feedbacks,
            }) => {
                let text = xim_ctext::compound_text_to_utf8(&preedit_string)
                    .map_err(|_| ClientError::InvalidReply)?;
                self.handler.handle_preedit_draw(
                    &mut self.client,
                    input_method_id,
                    input_context_id,
                    caret,
                    chg_first,
                    chg_length,
                    status,
                    &text,
                    feedbacks,
                )?;
                consumed = true;
            }
            Some(Request::ResetIcReply {
                input_method_id,
                input_context_id,
                ..
            }) => {
                if !self.handler.valid(input_method_id, input_context_id) {
                    return Err(ClientError::InvalidReply);
                }
                self.handler.resetting = self.handler.resetting.saturating_sub(1);
                self.send_next()?;
                consumed = true;
            }
            Some(Request::SyncReply {
                input_method_id,
                input_context_id,
            }) => {
                if !self.handler.valid(input_method_id, input_context_id) {
                    return Err(ClientError::InvalidReply);
                }
                self.current = None;
                self.send_next()?;
                consumed = true;
            }
            _ => {}
        }
        if consumed {
            if let Some((window, property)) = property {
                self.conn.delete_property(window, property)?;
            }
        } else {
            consumed = self.client.filter_event(event, &mut self.handler)?;
            if consumed {
                if let Event::SelectionNotify(e) = event {
                    self.client_window = Some(e.requestor);
                }
            }
        }
        for returned in self.handler.forwarded.drain(..) {
            let matches = |packet: &Packet| {
                (returned.time == 0 || packet.event.time == returned.time)
                    && packet.event.detail == returned.detail
                    && packet.event.response_type & 0x7f == returned.response_type & 0x7f
            };
            if let Some(packet) = self.current.as_mut().filter(|p| matches(p)) {
                if let Some(translated) = packet.translated.take() {
                    self.unhandled.push(translated);
                }
            } else if let Some(index) = self.asynchronous.iter().position(|p| matches(&p.packet)) {
                let mut pending = self
                    .asynchronous
                    .remove(index)
                    .expect("matched async key exists");
                if let Some(input) = pending.packet.translated.take() {
                    if input.state == crate::ButtonState::Pressed {
                        self.asynchronous_held.insert(returned.detail);
                    } else {
                        self.asynchronous_held.remove(&returned.detail);
                    }
                    self.unhandled.push(input);
                    if let Some(release) = pending.release {
                        self.asynchronous_held.remove(&returned.detail);
                        self.unhandled.push(release);
                    }
                }
            }
        }
        if self.handler.ready && self.current.is_none() && self.handler.resetting == 0 {
            self.deadline = None;
        }
        Ok(consumed)
    }
}

impl Drop for Ime {
    fn drop(&mut self) {
        if self.handler.ready {
            let _ = self.client.disconnect();
        }
        if let Some(window) = self.client_window {
            let _ = self.conn.destroy_window(window);
        }
    }
}

struct Handler {
    source: InputId,
    window: u32,
    state: TextInputState,
    locale: String,
    im: u16,
    ic: u16,
    ready: bool,
    focused: bool,
    active: bool,
    forward_mask: u32,
    synchronous_mask: u32,
    events: Vec<OverlayEvent>,
    forwarded: Vec<xproto::KeyPressEvent>,
    preedit: String,
    caret: usize,
    resetting: u32,
}

impl Handler {
    fn valid(&self, im: u16, ic: u16) -> bool {
        self.ready && self.im == im && self.ic == ic
    }
    fn sync_focus(&mut self, client: &mut ClientConnection) -> Result<()> {
        if !self.ready {
            return Ok(());
        }
        let active = self.focused && self.state.enabled;
        if active {
            let rect = self.state.cursor_rectangle;
            let attrs = client
                .build_ic_attributes()
                .nested_list(AttributeName::PreeditAttributes, |b| {
                    b.push(
                        AttributeName::SpotLocation,
                        xim::Point {
                            x: rect.x,
                            y: (i32::from(rect.y) + i32::from(rect.height)) as i16,
                        },
                    );
                })
                .build();
            client.set_ic_values(self.im, self.ic, attrs)?;
        }
        if active != self.active {
            if active {
                client.set_focus(self.im, self.ic)?;
            } else {
                client.unset_focus(self.im, self.ic)?;
            }
            self.events.push(OverlayEvent::InputMethod {
                source: self.source,
                active,
            });
            self.active = active;
        }
        Ok(())
    }
    fn preview(&mut self) {
        let cursor = byte_index(&self.preedit, self.caret).map(|b| b..b);
        self.events.push(OverlayEvent::TextPreedit {
            source: self.source,
            text: self.preedit.clone(),
            cursor,
        });
    }
}

impl ClientHandler<ClientConnection> for Handler {
    fn handle_connect(&mut self, client: &mut ClientConnection) -> Result<()> {
        client.open(&self.locale)
    }
    fn handle_open(&mut self, client: &mut ClientConnection, im: u16) -> Result<()> {
        self.im = im;
        client.get_im_values(im, &[AttributeName::QueryInputStyle])
    }
    fn handle_get_im_values(
        &mut self,
        client: &mut ClientConnection,
        im: u16,
        mut attrs: xim::AHashMap<AttributeName, Vec<u8>>,
    ) -> Result<()> {
        let bytes = attrs
            .remove(&AttributeName::QueryInputStyle)
            .ok_or(ClientError::InvalidReply)?;
        let styles: xim::InputStyleList = xim::read(&bytes)?;
        let candidates = [
            InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NOTHING,
            InputStyle::PREEDIT_NOTHING | InputStyle::STATUS_NOTHING,
            InputStyle::PREEDIT_NONE | InputStyle::STATUS_NONE,
        ];
        let style = candidates
            .into_iter()
            .find(|s| styles.styles.contains(s))
            .ok_or(ClientError::InvalidReply)?;
        let attrs = client
            .build_ic_attributes()
            .push(AttributeName::InputStyle, style)
            .push(AttributeName::ClientWindow, self.window)
            .push(AttributeName::FocusWindow, self.window)
            .build();
        client.create_ic(im, attrs)
    }
    fn handle_create_ic(&mut self, client: &mut ClientConnection, im: u16, ic: u16) -> Result<()> {
        self.im = im;
        self.ic = ic;
        self.ready = true;
        self.sync_focus(client)
    }
    fn handle_set_event_mask(
        &mut self,
        _: &mut ClientConnection,
        im: u16,
        ic: u16,
        forward: u32,
        synchronous: u32,
    ) -> Result<()> {
        if self.im == im && (self.ic == ic || !self.ready) {
            self.forward_mask = forward;
            self.synchronous_mask = synchronous;
        }
        Ok(())
    }
    fn handle_forward_event(
        &mut self,
        _: &mut ClientConnection,
        im: u16,
        ic: u16,
        _: xim::ForwardEventFlag,
        event: xproto::KeyPressEvent,
    ) -> Result<()> {
        if self.valid(im, ic) && self.focused && self.resetting == 0 {
            self.forwarded.push(event);
        }
        Ok(())
    }
    fn handle_commit(
        &mut self,
        _: &mut ClientConnection,
        im: u16,
        ic: u16,
        text: &str,
    ) -> Result<()> {
        if self.valid(im, ic) && self.active && self.resetting == 0 {
            // A commit does not reset XIM's incremental preedit buffer. Some
            // engines commit one syllable and replace the old preedit in a
            // subsequent draw; keep that wire state until Draw/Done/Start.
            self.events.push(OverlayEvent::TextPreedit {
                source: self.source,
                text: String::new(),
                cursor: None,
            });
            if !text.is_empty() {
                self.events.push(OverlayEvent::TextCommitted {
                    source: self.source,
                    text: text.into(),
                });
            }
        }
        Ok(())
    }
    fn handle_preedit_start(
        &mut self,
        client: &mut ClientConnection,
        im: u16,
        ic: u16,
    ) -> Result<()> {
        self.preedit.clear();
        self.caret = 0;
        client.send_req(Request::PreeditStartReply {
            input_method_id: im,
            input_context_id: ic,
            return_value: MAX_TEXT_BYTES as i32,
        })
    }
    fn handle_preedit_draw(
        &mut self,
        _: &mut ClientConnection,
        im: u16,
        ic: u16,
        caret: i32,
        first: i32,
        len: i32,
        _: xim::PreeditDrawStatus,
        text: &str,
        _: Vec<xim::Feedback>,
    ) -> Result<()> {
        if !self.valid(im, ic) || !self.active || self.resetting > 0 {
            return Ok(());
        }
        let first = usize::try_from(first).map_err(|_| ClientError::InvalidReply)?;
        let end = first
            .checked_add(usize::try_from(len).map_err(|_| ClientError::InvalidReply)?)
            .ok_or(ClientError::InvalidReply)?;
        let start = byte_index(&self.preedit, first).ok_or(ClientError::InvalidReply)?;
        let end = byte_index(&self.preedit, end).ok_or(ClientError::InvalidReply)?;
        if self.preedit.len() - (end - start) + text.len() > MAX_TEXT_BYTES {
            return Err(ClientError::InvalidReply);
        }
        self.preedit.replace_range(start..end, text);
        self.caret = usize::try_from(caret)
            .map_err(|_| ClientError::InvalidReply)?
            .min(self.preedit.chars().count());
        self.preview();
        Ok(())
    }
    fn handle_preedit_done(&mut self, _: &mut ClientConnection, im: u16, ic: u16) -> Result<()> {
        if self.valid(im, ic) {
            self.preedit.clear();
            self.caret = 0;
            self.preview();
        }
        Ok(())
    }
    fn handle_preedit_caret(
        &mut self,
        _: &mut ClientConnection,
        im: u16,
        ic: u16,
        position: &mut i32,
        direction: xim::CaretDirection,
        _: xim::CaretStyle,
    ) -> Result<()> {
        if self.valid(im, ic) && self.active && self.resetting == 0 {
            self.caret = move_caret(&self.preedit, self.caret, *position, direction);
            *position = self.caret as i32;
            self.preview();
        }
        Ok(())
    }
    fn handle_disconnect(&mut self) {
        self.ready = false;
        self.active = false;
        self.events.push(OverlayEvent::InputMethod {
            source: self.source,
            active: false,
        });
    }
}

fn byte_index(text: &str, index: usize) -> Option<usize> {
    text.char_indices()
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .nth(index)
}

fn move_caret(text: &str, current: usize, requested: i32, direction: xim::CaretDirection) -> usize {
    use xim::CaretDirection::*;
    let chars: Vec<_> = text.chars().collect();
    let length = chars.len();
    let current = current.min(length);
    let start = chars[..current]
        .iter()
        .rposition(|c| *c == '\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let end = chars[current..]
        .iter()
        .position(|c| *c == '\n')
        .map(|i| current + i)
        .unwrap_or(length);
    match direction {
        ForwardChar => current.saturating_add(1).min(length),
        BackwardChar => current.saturating_sub(1),
        AbsolutePosition => (requested.max(0) as usize).min(length),
        LineStart => start,
        LineEnd => end,
        ForwardWord => {
            let mut i = current;
            while i < length && !chars[i].is_whitespace() {
                i += 1;
            }
            while i < length && chars[i].is_whitespace() {
                i += 1;
            }
            i
        }
        BackwardWord => {
            let mut i = current;
            while i > 0 && chars[i - 1].is_whitespace() {
                i -= 1;
            }
            while i > 0 && !chars[i - 1].is_whitespace() {
                i -= 1;
            }
            i
        }
        CaretDown | NextLine => {
            if end == length {
                return current;
            }
            let next = end + 1;
            let next_end = chars[next..]
                .iter()
                .position(|c| *c == '\n')
                .map(|i| next + i)
                .unwrap_or(length);
            (next + current - start).min(next_end)
        }
        CaretUp | PreviousLine => {
            if start == 0 {
                return current;
            }
            let previous_end = start - 1;
            let previous = chars[..previous_end]
                .iter()
                .rposition(|c| *c == '\n')
                .map(|i| i + 1)
                .unwrap_or(0);
            (previous + current - start).min(previous_end)
        }
        DontChange => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preedit_indices_are_unicode_characters_and_convert_to_utf8_only_at_the_api() {
        assert_eq!(byte_index("に🙂a", 2), Some(7));
        assert_eq!(byte_index("に🙂a", 4), None);
        assert_eq!(
            move_caret("é文\nabc", 1, 0, xim::CaretDirection::NextLine),
            4
        );
        assert_eq!(
            move_caret("é文\nabc", 4, 0, xim::CaretDirection::PreviousLine),
            1
        );
        assert_eq!(
            move_caret("é文 abc", 0, 0, xim::CaretDirection::ForwardWord),
            3
        );
        assert_eq!(
            move_caret("é文 abc", 6, 0, xim::CaretDirection::BackwardWord),
            3
        );
    }
}
