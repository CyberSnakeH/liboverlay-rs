# Changelog

## 0.2.0 — in preparation

### API and rendering

- Additive generic C/C++ API: application-defined primitives, text, panels, and
  controls, ID-based events, and bounded frame validation.
- Hyprland/X11 window tracking, output selection, and input modes exposed through
  this API; event-based Hyprland shortcuts without a helper process.
- Reusable `liboverlay::overlay` CMake target and generic C++ example.

- Temporary Wayland keyboard focus capture with `set_keyboard_capture`; explicit
  release and input cancellation when returning to the application.
- Optional `egui` feature: input and rendering on the existing GPU surface.
- Local `liboverlay-c` package: versioned C ABI, diagnostics, C++17 RAII wrapper,
  CMake build, and CPU egui dashboard with C++ application logic.

- Premultiplied ARGB CPU rendering and optional Vulkan presentation, negotiated at startup.
- Exact `ScaleFactor` in units of 1/120, with Wayland fractional scaling when
  fractional-scale and viewporter are available.
- X11 CPU transfer through MIT-SHM 1.2; `PutImage` remains available as a fallback.
- X11 Present frame pacing with a timer fallback.
- X11 repositioning after monitor changes and shutdown when an explicitly selected
  output disappears.
- `PointerAxis` scrolling with explicit axes, sign, and units.
- Interactive `InputRegion`: bounded unions of rectangles, restoration after
  click-through, and coordinates retained at logical scale.
- XKB translation, layouts, modifiers, and repeat through `KeyboardInput`.
- XCompose composition, committed text, and preedit; Wayland text-input-v3 and
  X11 XIM input methods, with local degradation on XIM errors.
- Wayland UTF-8 field context, content hints, and caret rectangle.
- wl_touch/XInput 2.2 touch contacts, distinct sources, and cancellations.
- Bounded CPU allocation reuse, cleared for every new frame.
- `WeakOverlayWindow` references for callbacks that must not extend a window's lifetime.

- Optional presentation feedback: Wayland presentation-time and X11 CPU Present
  completion, with explicit clock, sequence, and timestamp quality.
- Optional CPU frame comparison to restrict uploads and damage to changed pixels;
  one additional complete image is retained.

### Robustness

- Positional Wayland keymap reads, independent of the compositor-shared file
  descriptor's cursor; avoids false XKB truncation and immediate closure of
  interactive overlays on Hyprland.

- eventfd runtime wakeup to avoid SIGPIPE on shutdown in C/C++ applications that
  retain the signal's default behavior.

- Respect for real IBus/Fcitx servers' XIM synchronization masks and preservation
  of incremental preedit after intermediate commits.
- Observable input resets, with source, scope, and reason, on overflow or
  interruption. Bounded tracking of keys and contacts.

- Shutdown independent of a full drawing queue and a shared join for concurrent calls.
- GPU callback panics surfaced as `RuntimePanic`, with the acquired image discarded
  before swapchain destruction.
- GPU device loss detection while idle and while waiting for callbacks.
- Processing of X11 events already read by synchronous requests, and monitor
  changes that do not change the screen's total dimensions.
- Undecorated interactive X11 windows, with explicit placement and a request to
  preserve focus on opening; activation through interaction remains available.
- One cached Vulkan instance per process; surfaces and devices remain specific to
  each window.

### Validation and tooling

- Isolated Xvfb and Sway suites: missing protocols, fallbacks, resizes, monitors,
  withheld callbacks, and connection loss.
- Tests for concurrency, controlled device loss, callback panics, and independence
  of multiple GPU windows.
- GitHub Actions matrix: stable and 1.85 compilers, features, Clippy, rustdoc,
  Cargo package, musl, and private graphics servers.
- Documentation for every public item, enforced at compilation.
- CPU/Vulkan stability and measurement examples with configurable dimensions and duration.
- `text_hud` example with a CPU text renderer and application-supplied font.
- Separate per-process CPU/RAM and NVIDIA VRAM measurements.
- Vulkan endurance runner on private Sway, retaining logs, source and binary
  digests, timestamps, and exit status.

### Migration

`WindowInfo::scale_factor` is a `ScaleFactor`. Use `as_f64()` for the usual
multiplier and `numerator()` for the exact protocol value. Frames still use the
physical dimensions in `WindowInfo`.

`OverlayEvent` remains non-exhaustive. Applications must handle the new input
events and retain a wildcard case in matches. Events are consumed from a queue
shared by all handle clones. A fatal error is returned once, before remaining
pending events. Queue overflow replaces pending notifications with `InputReset`,
then the current event. Handle its scope to discard invalidated state. For
editing, insert only `TextCommitted`, never also `KeyboardInput.text`.
`Capabilities` adds availability information for regions, translation,
composition, input methods, surrounding context, touch, and presentation feedback.

The `gpu` feature is optional. `GpuPreferred` may select the CPU; consult the
report before using renderer-specific methods.

The [validation documentation](docs/TESTING.md) and [roadmap](ROADMAP.md) define
actual coverage before creating a tag or publishing a release.
