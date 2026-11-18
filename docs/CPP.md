# C/C++ interface and egui

## Generic C/C++ API

`include/liboverlay_canvas.h` exposes drawing and widgets independently of
application logic. `include/liboverlay_canvas.hpp` provides `Canvas` (RAII) and
`Frame` (owned commands and strings). The application defines its entire
interface in C or C++; Rust and egui remain inside the library. No
application-specific Rust code is required.

### Build a C++ application from the source archive

Install Rust 1.85 or newer with Cargo, CMake 3.20 or newer, a C/C++ toolchain,
pkg-config and the Wayland development files (for example, `libwayland-dev`
on Debian/Ubuntu). To open a canvas at runtime, the Vulkan loader, a compatible
driver and a compositor offering surface alpha are required.

Extract the full source archive as a sibling of the application's directory:

```sh
mkdir -p ../liboverlay-rs
tar -xzf liboverlay-rs-0.2.0-source.tar.gz -C ../liboverlay-rs --strip-components=1
```

In the application's `CMakeLists.txt`:

```cmake
cmake_minimum_required(VERSION 3.20)
project(my_application LANGUAGES CXX)
add_subdirectory("../liboverlay-rs" "liboverlay")
add_executable(my_application main.cpp)
target_compile_features(my_application PRIVATE cxx_std_17)
target_link_libraries(my_application PRIVATE liboverlay::overlay)
```

```sh
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build
./build/my_application
```

The target exports the include directory and invokes Cargo with the supplied
lockfile. It produces `liboverlay_c.so` in the CMake build tree and links the
application to that copy. Keep the build tree in place when running this example.
Distributors who relocate their executable must also arrange for the dynamic
loader to find the locally built library. The full archive contains source only.
The separate Rust `.crate` contains the core library and is intended for Cargo
consumers.

### Draw from C++

```cpp
#include "liboverlay_canvas.hpp"

// Call regularly from the thread that owns canvas.
void render(liboverlay::Canvas& canvas, bool& enabled) {
    const auto info = canvas.info();
    liboverlay::Frame frame;
    frame.rect({40, 40, 120, 80}, 0x4ddadcff, 2);
    frame.text({40, 130}, "My overlay", 16);
    frame.begin_panel(100, {200, 40, 250, 180});
    frame.label("Display");
    frame.checkbox(101, "Show", enabled);
    frame.button(102, "Action");
    frame.end_panel();
    const auto result = canvas.submit(frame);
    for (const auto& event : result.events) {
        if (event.kind == OVERLAY_EVENT_CHANGE && event.id == 101)
            enabled = event.value != 0;
    }
    result.check(); // Check after applying events.
    (void)info;
}
```

Create `Canvas` with `overlay_canvas_default_options()`, then set
`input_mode=OVERLAY_INPUT_POINTER` to receive clicks. By default the surface is
passive and fills the selected monitor. Two nonzero dimensions create a
fixed-size surface. `output` names an exact monitor; when empty, the library
selects automatically. Option strings are copied during creation and frame
strings during submission.

### Drawing, controls and actions

Commands cover rectangles, lines, circles, triangles, text, clipping, panels,
labels, buttons, checkboxes, sliders and separators. Coordinates use surface
logical pixels; `info()` provides current dimensions. Colors are
non-premultiplied `0xRRGGBBAA` values. `Frame::clip` limits subsequent primitives;
the next `clip` replaces the previous rectangle and `reset_clip` restores the
full surface. Panels do not inherit primitive clipping.

Panels arrange scrollable vertical content. They have a position, width and
maximum content height. Panels cannot be nested. Each panel and interactive
control has a stable, distinct, nonzero ID. Labels and primitives need no ID.

The application owns state: `CHECKBOX` and `SLIDER` receive a value and return a
`CHANGE` event containing their ID and the new value. A button returns `CLICK`.
Apply these events even if rendering returns `BUSY` or an error after processing
the interface; events are not replayed. The C `OverlayCommand` structure and its
fields are documented in the header.

A frame is fully validated before consuming input: finite numbers, consistent
dimensions and ranges, UTF-8 text, balanced panels and unique IDs. Limits are
8,192 commands, 128 interactive controls, 64 KiB per string and 1 MiB of total
text. C callers reserve at least `OVERLAY_MAX_EVENTS` events; the C++ wrapper
handles this. Pointers must remain valid and aligned during the call; Rust
retains no foreign pointer.

### Backend, target and input

`Canvas::info()` reports the selected backend, dimensions, target and
capture/shortcut capabilities. Wayland and X11 are overlay backends; X11 can run
through Xorg or XWayland. Native or XWayland target protocols are identified
when Hyprland provides that information.

`options.target_pid` or `Canvas::follow(pid)` enables automatic tracking of an
external window. The library queries Hyprland for native and XWayland clients,
or `_NET_WM_PID`/RandR on X11. It selects the monitor and recreates its surface
if the target changes outputs. `info.target` supplies the viewport as fractions
of the monitor. An absent, hidden or non-visible-workspace window has
`visible=0`: the application then decides whether to hide its data. Tracking is
refreshed by `info()` and `follow()` with a 100 ms cache. `submit()` uses the
already prepared surface to preserve coordinates calculated by the caller.
The static `overlay_target_query` query is also available before surface
creation.

`Canvas::input` accepts:

- `PASSIVE`: click-through with keyboard focus released.
- `POINTER`: receive clicks across the surface without exclusive focus.
- `MODAL`: receive clicks and exclusive Wayland keyboard focus; closing cancels
  active input. X11 returns `OVERLAY_UNSUPPORTED` for this mode.

### Shortcuts and desktop limitations

`canvas.shortcut("Insert", id)` registers a shortcut and returns a status.
Shift/Ctrl/Alt/Super modifiers are optional. `submit()` delivers `SHORTCUT` events
even without focus; the application determines their effect, for example by
checking `liboverlay::active_pid()`. A conflict preserves the existing shortcut
and returns `OVERLAY_CONFLICT`.

The current shortcut integration uses Hyprland's Lua API with `hl.dsp.event` and
its local event stream: no helper process is launched and no application-specific
command socket is required. Internal exchanges are bounded and bindings are
removed through their exact objects during normal destruction. No configuration
file is modified. Losing the stream emits `SHORTCUT_LOST`; remove and register
the shortcut again or notify the user. Reloading Hyprland configuration can
remove bindings; restart/register them again after a reload.

Other desktops return `OVERLAY_UNSUPPORTED` for global shortcuts. Wayland window
tracking also depends on desktop integration: backend detection does not grant
these permissions. Shortcut portals and other compositors are not implemented
in this API. These limitations are separate from rendering, which remains
available on negotiated backends.
Sources: [Hyprland shortcut API](https://wiki.hypr.land/configuring/core/binds/),
[dispatchers and events](https://wiki.hypr.land/configuring/core/dispatchers/).

Handles are confined to their creating thread, including destruction. Status,
diagnostics and error rules follow the legacy ABI described below. The generic
API is additive and versioned separately; the older CPU example continues to
work unchanged.

### Example and checks

```sh
cmake -S examples/cpp -B target/cpp -DCMAKE_BUILD_TYPE=Release
cmake --build target/cpp -j 2
ctest --test-dir target/cpp --output-on-failure
target/cpp/overlay_canvas --seconds 10
python3 scripts/check_canvas.py
```

The final script tests real checkbox, slider and button events and ABI contracts
on private Sway and Xvfb/picom sessions. Test dependencies match the other
graphical suites, with libXtst added for X11 input; an X11 PNG capture is optional
when Pillow is installed.

## Legacy API: sample dashboard

The repository provides two distinct layers:

- The Rust `egui` feature adds `liboverlay::egui::EguiOverlay`, a reusable adapter
  for building an egui interface on an existing GPU window.
- The local `liboverlay-c` package exposes a lifecycle C ABI and a sample CPU
  dashboard. Its application schema is defined in `include/liboverlay.h`.
  The `include/liboverlay.hpp` wrapper manages its handle with C++17 RAII.

The application remains a standalone executable. C++ retains measurements,
calculations and control of settings. Rust draws the interface, receives input
and returns actions. The libraries share a process; no IPC or loading into
another application is required.

## Building and running the C++ example

Requirements: Linux, Rust 1.85 or later, CMake 3.20+, C11/C++17 compilers, native
`gpu` profile dependencies and a Vulkan driver. Transparency requires a Wayland
compositor with layer-shell or an X11 server with compositing. Desktop stacking
rules still apply.

The C/C++ kit requires the complete Git checkout (workspace, `ffi` and `include`);
the crates.io `liboverlay-rs` package distributes the Rust library and examples.

From the repository root:

```sh
cmake -S examples/cpp -B target/cpp -DCMAKE_BUILD_TYPE=Release
cmake --build target/cpp -j 2
ctest --test-dir target/cpp --output-on-failure
target/cpp/overlay_monitor --backend auto --seconds 30
```

CMake builds the Rust package in release mode, then links the executable to
`target/cpp/cargo-target/release/liboverlay_c.so`. The build path is configured
to locate this library. To distribute the application, install the library and
native dependencies with an appropriate search path; the build directory is
not an installable package.

The example reads `/proc/stat` every 200 ms, calculates an average in C++ and
displays the dashboard in Rust. The loop calls `overlay_tick` regularly with a
16 ms pause. The duration bounds execution; the Close button also exits.
`--simulate` replaces measurements with a deterministic curve. `--allow-software`
explicitly permits a software Vulkan adapter for tests. `--self-test` checks
C++ application logic without opening a window.

To build both library formats directly:

```sh
cargo build --locked --release -p liboverlay-c
```

Outputs are `target/release/liboverlay_c.so` and `liboverlay_c.a`. The static
format may still require system libraries; retrieve the linker list with
`cargo rustc -p liboverlay-c --release -- --print native-static-libs`. The C
package is local to the workspace and is not published on crates.io.

## ABI version 1 contract

`overlay_default_config` supplies a complete configuration: ABI 1, 420×220
logical pixels, automatic backend, GPU-required rendering, interactive pointer,
on-demand keyboard and software adapters rejected. Unknown bits and incompatible
versions are rejected. ABI 1 requires the exact structures from the matching
header; do not mix library/header versions.

1. Call `overlay_create` with a configuration and output pointer.
2. On each iteration, fill `OverlayState`, call `overlay_tick` and apply settings
   when `settings_changed` is 1. Return the updated state on the next iteration.
   Rust does not replay actions.
3. Stop on `OVERLAY_CLOSED`. `OVERLAY_BUSY` indicates a discarded draw or full
   queue; actions must still be applied before retrying.
4. Call `overlay_destroy` once on the creating thread.

Negative statuses indicate an error. Immediately read the thread-local diagnostic
with `overlay_last_error`, before another status-returning call. The message is
UTF-8 and its required length includes NUL; a truncated copy may split a UTF-8
character. A Rust panic is contained and invalidates the handle for later ticks;
destroy it. Fatal allocation errors and invalid pointers remain outside the
recovery contract.

All access to a handle must be serialized on its creating thread. Pointers must
be valid, aligned and non-overlapping during the call. Rust copies data and
retains no C++ pointer. Percentages accept only finite values from 0 to 100;
integer booleans must be 0 or 1. No `std::string`, `std::vector`, egui object or
wgpu object crosses the ABI.

`overlay_destroy(NULL)` succeeds. On the correct thread, destruction consumes
the handle even if closing returns an error. On another thread, it returns
`OVERLAY_WRONG_THREAD` without consuming the handle. The C++ wrapper is neither
copyable nor movable and must also be destroyed on its creating thread.

## Adapting the interface to another application

The dashboard is not a generic interface-description language. For another
application, change or replace these together:

- C states and actions in `include/liboverlay.h` and their Rust `#[repr(C)]`
  equivalents in `ffi/src/lib.rs`; increment the ABI if the contract changes.
- The interface in `ffi/src/dashboard.rs`.
- State production and action handling in C++.

Calculations do not need to be rewritten in Rust. For large lists, add an
explicit pointer/length contract and bounded copy rather than sharing either
language's internal containers.

## Using only the Rust adapter

Enable `features = ["egui"]`, build an `OverlayWindow` with
`RendererPreference::GpuRequired`, then pass it to `EguiOverlay::new`. Configure
the pointer and `KeyboardMode::OnDemand` in the builder if needed. Call
`run(|ctx| { ... })` for each iteration. The context supports style and font
configuration. Do not consume this window's events elsewhere.

The renderer uses the existing wgpu surface, device and queue: no second window
or winit/eframe dependency. egui/egui-wgpu are pinned to 0.32.3, compatible with
wgpu 25 and the declared minimum Rust version. Other features remain available
without enabling egui.

Only one egui draw remains pending. Textures are updated before drawing and
their deletion is deferred; deltas survive discarded frames during geometry
changes. The call waits for command submission with a two-second limit; it
guarantees neither GPU completion nor physical presentation. After a timeout,
the pending draw remains tracked and no newer renderer update overtakes it.
A UI action may have been produced even if drawing subsequently fails.

## Input and current limitations

The bridge translates pointer input, buttons, wheel, common XKB keys, modifiers,
focus and `TextCommitted`, preserving logical coordinates and egui zoom.
Committed text is inserted once. The dashboard uses buttons and a slider,
without a text field.

This initial integration does not connect the system clipboard, URLs,
accessibility, additional egui windows, touch or the complete IME context
(preedit, caret and surrounding-text deletion). Corresponding liboverlay-rs
support remains independent; its capabilities do not imply that all of them
are connected to egui. `run` returns `FullOutput` so a Rust integrator can handle
platform commands.

This older adapter registers no global shortcut: keyboard input is received
while the overlay has focus. Click-through prevents interaction with its
controls.

## Validation

```sh
cargo test --locked --workspace --all-features --all-targets
cargo clippy --locked --workspace --all-features --all-targets -- -D warnings
cmake --build target/cpp -j 2
ctest --test-dir target/cpp --output-on-failure
```

Display-independent tests check error contracts, ABI structures, input
translation, interface construction, C linkage and C++ application logic. For
rendering and interactions in a private session:

```sh
SWAY=/path/to/sway python3 scripts/check_cpp.py --build-dir target/cpp
```

This final test requires Sway, a usable compositor renderer and Vulkan. It never
interacts with the current desktop session.

The test uses a virtual pointer attached only to the private compositor. It
checks C++ updates received from the checkbox and slider, the close button,
rejection of calls from another thread, rejection of non-finite state and
scale/resolution changes during rendering. `--x11` adds a run on
Xvfb/picom with lavapipe; `XVFB`, `PICOM` and `LAVAPIPE_ICD` select these tools.
The driver must provide a surface with alpha: a driver advertising only
`Opaque` is rejected even when an X11 compositor is active.

To reproduce the CI software profile, select the pixman compositor with
`LIBOVERLAY_TEST_COMPOSITOR=pixman` and lavapipe through `VK_DRIVER_FILES` (or
`VK_ICD_FILENAMES` on older loaders). For a hardware Vulkan compositor,
`WLR_RENDER_DRM_DEVICE` selects the render node; some drivers require
`LIBOVERLAY_TEST_UNSUPPORTED_GPU=1` for the test Sway instance.

Local validation on November 15, 2026:

- Rust 1.85.0 and 1.99.1: 31 core unit tests with all features and three C-bridge
  tests; the minimum-version run uses rustc 1.85 with the host Cargo.
- Full suite, doctest, Clippy, rustdoc and Rust package checks passed with the
  host compiler; ignored graphical scenarios remain separate.
- C11 and C++17: CMake build and two CTest tests passed; static C++ linkage and
  the application-logic test also passed.
- Private sessions: Wayland exchanges and closing, fractional-scale rendering,
  live ABI checks, the pixman/lavapipe software profile and
  X11/picom/lavapipe rendering passed. Corresponding CI is configured without
  claiming a remote execution.

Local logs and captures are retained under `target/validation/cpp*` and
`target/validation/egui-*`; these artifacts are not distributed in the package.
