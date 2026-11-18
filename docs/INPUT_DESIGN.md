# Input: contracts and integration

Custom regions, keyboard translation, composition and touch are implemented for
Wayland and X11. Touch is experimental and outside the 0.2 support scope.
These features use the same display thread as rendering. The core does
not automatically install global shortcuts. The C/C++ API can explicitly
register Hyprland shortcuts; see [CPP.md](CPP.md). Negotiated capabilities appear
in `backend_report()`, a startup snapshot taken before any subsequent hardware
hotplug.

## Interactive regions

`InputRegion` represents the entire surface (`Full`), no input (`Empty`) or a
union of rectangles (`Rectangles`). Each `InputRectangle` is half-open:
`[x, x + width) × [y, y + height)`. Dimensions must be positive; negative
coordinates and overlaps are accepted. An empty list is equivalent to `Empty`.
The limit is `MAX_INPUT_RECTANGLES`, or 1,024 rectangles. The `i16`/`u16` types
preserve a common representation for both protocols.

Coordinates are surface-local: logical coordinates on Wayland, server pixels on
X11. The server intersects the region with the current extent. The library
retains the original rectangles: an area outside the old window can become
interactive after enlargement. A Wayland scale change does not multiply their
coordinates.

`with_input_region()` and `set_input_region()` enable the region by disabling
click-through. `set_click_through(true)` temporarily hides it;
`set_click_through(false)` restores it. Regions affect pointer and touch input
without taking keyboard focus. They do not revoke a gesture already assigned
by the server: a contact or held button can continue generating events until
its sequence ends.

Synchronous commands validate arguments before queuing. Their return confirms
that the request was sent, independently of drawing cadence; it does not confirm
when a physical event will be affected. A timeout does not cancel an already
queued command. These methods are rejected from a GPU callback, which runs on
the display thread itself.

Wayland uses `wl_surface.set_input_region` and a commit; X11 uses SHAPE INPUT 1.1.
See [Wayland](https://wayland.freedesktop.org/docs/html/apa.html#protocol-spec-wl_surface)
and [SHAPE](https://www.x.org/releases/X11R7.5/doc/Xext/shape.pdf).

## Keyboard

Keyboard input is disabled by default. `KeyboardMode::OnDemand` requests
interaction subject to compositor or window-manager policy. Creating a window
with `TextInputState::enabled()` also selects this mode.

On Wayland, `set_keyboard_capture(true)` temporarily requests exclusive focus
for the layer-shell surface. This requires a window created with
`KeyboardMode::OnDemand`. `set_keyboard_capture(false)` releases focus and
cancels active input to prevent stuck keys or drags. A modal interface can
combine this with `InputRegion::Full` when opening and `InputRegion::Empty`
when closing. This method returns `Unsupported` on X11. The application
chooses what its global shortcut does; the C/C++ API can register it with
Hyprland.

Raw `Key` events remain available. `KeyboardInput` adds XKB keysyms, individual
text before composition, modifiers, the group and a repeat indicator. `code`
follows evdev; X11 applies the usual XKB offset of eight. Modifiers not consumed
by XKB level selection allow shortcuts to be recognized without treating a
Shift key needed for a character as a command modifier.

The library uses [kbvm 0.1.7](https://docs.rs/kbvm/0.1.7/) without native linkage
to libxkbcommon. Wayland supplies its keymap and modifiers; X11 reads them through
XKB and tracks changes. Wayland schedules repetition according to seat settings,
without burst catch-up after a wait. X11 requests detectable autorepeat from
the server. Focus loss or a keymap change cancels key states, repeat and local
composition.

## Text and input methods

An application enables a field with `with_text_input()` or, if keyboard input
was authorized at creation, `set_text_input()`. Disabling the field preserves
raw and translated keys. The library transports edits; the application owns
the document, selection and rendering.

- Insert only `TextCommitted`, once. `KeyboardInput.text` describes a key before
  composition and must not cause a second insertion.
- Draw `TextPreedit` separately from the document. Each notification replaces
  the previous preedit; empty text clears it. The cursor is a UTF-8 byte range,
  or `None` if the input method supplies no valid cursor.
- Apply `TextDeleteSurrounding` before the associated commit. Sizes are UTF-8
  bytes around the cursor, excluding the selection. Check document bounds and
  UTF-8 boundaries before modifying content.
- Resend updated context after edits. `SurroundingText` holds at most 4,000 bytes
  without NUL, with `cursor` and `anchor` on UTF-8 boundaries. A document window
  must contain the entire selection. `TextChangeCause::InputMethod` identifies
  an edit from the input method; `Other` includes application-driven cursor
  movement.
- Supply the logical caret rectangle to position input interfaces. `Password`
  and `Pin` fields reject all surrounding context.

On Wayland, `text-input-v3` carries preedit, commits, deletion, context, change
cause and field hints. A `done` batch is processed in this order: clear the
previous preedit, delete, insert, display the new preedit. Batch edits remain
applicable even if their serial predates an application update; they do not
replace that newer context in the library. Each seat can have its own input
context.

On X11, [XIM](https://www.x.org/releases/X11R7.5/doc/libX11/xim.pdf) is negotiated
through [xim 0.5](https://docs.rs/xim/0.5.0/), according to `XMODIFIERS=@im=name`
and the presence of the advertised server. Callback styles supply preedit
through events; a server-managed style can display its own interface. The caret
rectangle feeds `SpotLocation`. Surrounding context and content hints are not
transmitted through this path: `surrounding_text` remains `Unsupported`. XIM
character positions are converted to UTF-8 bytes. Translated events for a
filtered key are emitted only if XIM returns it to the application, preserving
press/release order; raw events remain immediate.

Protocol availability or `InputMethod { active: true }` indicates an enabled
context, not the presence of an input-method process or guaranteed support on
every desktop. An XIM error or server disappearance produces `InputMethodError`
without closing the window. Synchronous XIM exchanges and negotiation are bounded
and monitored with a five-second deadline. The negotiated server mask determines
synchronous classes: no acknowledgement is expected for other keys. A history
bounded to 256 snapshots allows returned asynchronous keys to be translated,
including when the server resets their timestamp to zero. Consumed keys do not
block later keys. The protocol preedit buffer survives an intermediate commit
until its next incremental update; its content is not inserted into the document
a second time.

Local XCompose composition remains available when locale tables are installed.
It supports dead keys and Compose sequences. Without tables, key translation
and direct text remain usable; the report identifies the limitation. No complete
set of layouts or Compose tables is embedded. On Wayland, the compositor filters
keys handled by an input method; returned keys can follow the local path without
duplicate commits.

Reference: [text-input-v3](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/unstable/text-input/text-input-unstable-v3.xml).

## Touch contacts

`Touch` supplies `source`, `id`, `phase`, `x` and `y`. Phases are `Started`,
`Moved`, `Ended` and `Cancelled`. An ID is unique only within its source while
active; the server can reuse it after the contact ends. Coordinates are local
and preserve fractional values. An end or cancellation retains the last known
position. Moves and ends without a known start are ignored. A start reusing an
active ID first cancels the previous contact.

`InputId` identifies a Wayland seat without a physical device, or an XInput master
and its source device. IDs are backend-local and must not be persisted between
sessions. `TouchFrame` ends a Wayland batch; X11 does not synthesize a frame
boundary.

Wayland negotiates `wl_touch` for seats that advertise it. Removing a seat cancels
its contacts. X11 requests XInput 2.2 or later and selects touch events only for
its own window. Hierarchy notifications cancel contacts from removed or disabled
devices. Raw touch events from other applications are not selected.

References: [wl_touch](https://wayland.freedesktop.org/docs/html/apa.html#protocol-spec-wl_touch)
and [XInput 2](https://xorg.freedesktop.org/archive/current/doc/inputproto/XI2proto.txt).

## Overflow and interruption

The public queue holds at most 256 notifications. When full, pending
notifications are replaced with `InputReset` followed by the current event.
The runtime also resets its input states. Commits can therefore be lost if the
application does not drain the queue: it must discard preedit and resend its
document context after a `QueueOverflow` reset.

`InputReset.scope` specifies exactly what the application must clear: `Keyboard`
invalidates keys, repeat and composition; `Touch` invalidates contacts; `All`
also invalidates pointer buttons. `source: None` covers all sources. A keyboard
change does not end an unrelated touch gesture. Closing produces a global reset
before `Closed`.

Tracking is bounded by `MAX_PRESSED_KEYS` (256 per source) and
`MAX_TOUCH_CONTACTS` (64 per overlay). Exceeding these limits produces `KeyLimit`
or `ContactLimit`, respectively, with the corresponding scope. Wayland keymaps
are limited to 1 MiB; preedit and commit strings are limited to 64 KiB.

## Validation

Unit tests use a self-contained keymap and Compose table. Private suites verify
actual region routing, language changes, modifiers, repeat, dead keys and
exchanges with controlled input methods. Touch tests cover multiple sources,
reused IDs, cancellation and seat removal. The exact scope and hardware
limitations are documented in [TESTING.md](TESTING.md).
