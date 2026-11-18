# liboverlay-rs

A Rust library for transparent Linux overlays, with configurable click-through
and capability negotiation at startup.

The Cargo package is `liboverlay-rs`; its crate name in code is `liboverlay`.
Version 0.2 provides premultiplied ARGB CPU rendering, optional Vulkan
presentation, X11 Present, and Wayland fractional scaling. CPU rendering remains
the default; the `gpu` feature adds direct GPU presentation.

The optional `egui` feature adds a graphical interface to the same GPU surface.
The generic C API and C++17 wrapper let applications define drawings and controls
entirely in C/C++, with events identified by control ID, backend selection,
window tracking, and input management handled by the library. See the
[C/C++ integration guide](docs/CPP.md) for desktop capabilities and limitations.

## Installation

Version 0.2.0 is a release candidate distributed locally as source. For a Rust
application, extract the prepared `.crate` or the full source archive into
`vendor/liboverlay-rs` (remove the archive's top-level directory) and use:

```toml
[dependencies]
liboverlay = { package = "liboverlay-rs", path = "vendor/liboverlay-rs" }
```

The imported module remains `liboverlay`. To enable Vulkan, add
`features = ["gpu"]` to this dependency. The declared minimum Rust version is
1.85; see the [validation coverage](docs/TESTING.md) for the compilers actually
tested.

For C and C++17, use the full source archive. It contains the public headers,
the Rust FFI crate and the CMake target. CMake builds the shared library locally
with Cargo. See the [C/C++ build instructions](docs/CPP.md).

To run the examples from a checkout:

```sh
git clone https://github.com/CyberSnakeH/liboverlay-rs.git
cd liboverlay-rs
cargo run --example basic_hud -- auto 10
```

## 0.2 support scope

Linux x86-64 validation covers the CPU backends, Wayland layer-shell and X11
with the documented server capabilities. Vulkan coverage includes NVIDIA
hardware on Wayland and explicitly enabled lavapipe in isolated sessions. The
C/C++ canvas requires a transparent Vulkan surface. Xorg CPU rendering, alpha,
focus and fullscreen behavior have also been exercised in a QEMU guest.

Physical Intel and AMD GPUs, physical Xorg sessions, monitor hotplug and
fullscreen transitions on other desktops remain unverified. Touch input is
outside the 0.2 support scope. Global shortcuts are implemented for Hyprland.
The project makes no optical latency or physical frame-rate claim.
See the [hardware matrix](docs/HARDWARE_VALIDATION.md) and
[recorded runs](docs/TESTING.md).

## Documentation

- [Usage and API](docs/USAGE.md)
- [C/C++ interface, egui, and application example](docs/CPP.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Tests, results, and validation limits](docs/TESTING.md)
- [Performance measurements](docs/PERFORMANCE.md)
- [Hardware validation and optical measurements](docs/HARDWARE_VALIDATION.md)
- [Development and release preparation](docs/DEVELOPMENT.md)
- [Keyboard, text, touch, and interactive regions](docs/INPUT_DESIGN.md)
- [Changelog](CHANGELOG.md)
- [Remaining work](ROADMAP.md)

```rust,no_run
use liboverlay::{premultiply, OverlayBuilder, OverlayError};
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let overlay = OverlayBuilder::new()
        .with_title("GameHUD")
        .with_size(800, 600)
        .with_click_through(true)
        .build()?;

    println!("{:#?}", overlay.backend_report());
    let start = Instant::now();
    while overlay.is_running() && start.elapsed() < Duration::from_secs(10) {
        overlay.poll_events()?;
        match overlay.draw(|pixels, width, height| {
            pixels.fill(0); // transparent
            for y in 10..height.min(80) {
                for x in 10..width.min(240) {
                    pixels[(y * width + x) as usize] = premultiply(30, 200, 100, 160);
                }
            }
        }) {
            Ok(()) => {},
            Err(OverlayError::QueueFull | OverlayError::OutdatedFrame { .. }) => {},
            Err(error) => return Err(error.into()),
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    overlay.close()?;
    Ok(())
}
```

## Examples

```sh
cargo run --example basic_hud -- auto 10
cargo run --example basic_hud -- wayland 10
cargo run --example basic_hud -- x11 10
cargo run --example interactive_toggle
cargo run --example input_events
cargo run --example diagnose -- auto
cargo run --features gpu --example gpu_hud -- auto 10
cargo run --features gpu --example diagnose -- x11 prefer-gpu
cargo run --example text_hud -- /path/to/font.ttf wayland 10
```

`interactive_toggle` alternates every two seconds between a blue rectangle that
passes clicks through and an orange rectangle that receives them. It does not
request keyboard input. The example exits after twelve seconds. It creates no
global shortcut and generates no synthetic clicks.

`text_hud` integrates the fontdue CPU text renderer with a font supplied by the
application. It prepares glyphs at physical scale and rebuilds them when geometry
changes. The text dependency applies only to examples and is not added to the
dependencies of applications using the library.

## Backend selection and capabilities

`Auto` examines `WAYLAND_DISPLAY`/`WAYLAND_SOCKET`, attempts a real Wayland
connection, then tries `DISPLAY` if necessary. XWayland and Xorg use the same X11
backend. An explicit `WaylandOnly` or `X11Only` preference disables fallback.

| Feature | Wayland layer-shell | X11 / XWayland |
|---|---|---|
| Transparency | `wl_shm::ARGB8888` | ARGB RENDER visual and compositing |
| Click-through | Empty input region + commit | Empty SHAPE INPUT region, version >= 1.1 |
| Stacking | layer-shell Overlay layer | Raise or EWMH request, depending on mode |
| Keyboard | XKB and repeat; OnDemand with layer-shell v4+ | XKB and repeat; managed window for OnDemand |
| Composed text | XCompose and text-input-v3 | XCompose and XIM |
| Custom regions | Union of logical rectangles | SHAPE INPUT 1.1 |
| Touch (experimental, outside 0.2 support) | wl_touch, per seat | XInput 2.2+, per source |
| CPU frame pacing | Compositor callback | Synchronized Present, otherwise a timer |
| GPU frame pacing | FIFO swapchain and compositor callback | FIFO swapchain, if the driver meets the requirements |
| Scaling | Fractional with fractional-scale + viewporter, otherwise integer | X11 screen pixels |
| Named output | `wl_output` name, version >= 4 | RandR monitors, version >= 1.5 |

Transparency is required by default. `Requirement::Preferred` explicitly allows
an opaque X11 window if compositing or an alpha visual is unavailable.
`StackingPolicy::RequireOverlayLayer` requires layer-shell; `PreferOverlay`
allows the weaker X11 policies. The report retains failures from earlier
attempts and the limitations of the selected backend. It describes protocol
capabilities, not observed desktop pixels.

An XWayland server does not guarantee stacking above a native Wayland application
in fullscreen mode. Lock screens and compositor interfaces remain under the
compositor's control. Without layer-shell or an X11 fallback that satisfies the
requirements, creation returns `NoBackend`.

## Rendering, threads, and lifecycle

- Pixels are numeric `0xAARRGGBB` values with premultiplied alpha. Use
  `premultiply(r, g, b, a)` to convert ordinary components. The X11 server's byte
  order is handled during transfer.
- Each `frame()` starts with transparent pixels. `draw()` obtains that frame,
  calls the application's function, then presents it. At most two CPU allocations
  are retained for reuse; their contents are cleared before returning them to the
  application.
- An `OverlayWindow` is a cloneable `Send + Sync` handle. The private runtime owns
  connections and protocol objects. Dropping the last handle closes the runtime.
  `close()` waits for resource cleanup and joins the thread, including its native
  destructors. Dropping the last handle also requests this shutdown. Waiting for
  the event loop is bounded by the command timeout; the final join also waits for
  native libraries to destroy thread-local data. Shutdown bypasses the drawing
  queue even when full, and is shared by all clones. Concurrent callers wait for
  the same cleanup.
- `present()` consumes the frame and means “queued.” `FrameSubmitted` means “sent
  to the server,” without confirming physical display. A frame replaced or made
  obsolete by a resize produces `FrameDiscarded`.
- The command queue holds at most eight entries, and the runtime retains at most
  one pending frame. It can discard frames to keep the most recent one. The event
  queue is bounded to 256 entries; consecutive movements are coalesced. Overflow
  replaces pending notifications with an observable `InputReset`, followed by the
  current event; the application must reset the state indicated by its scope and
  resend its text context.
- Wayland retains at most three SHM buffers across all generations and waits for
  `wl_buffer.release` before rewriting them. A hidden window may stop receiving
  frame callbacks; input commands continue to be processed.
- X11 negotiates Present 1.0 for CPU frames, requests a copy at the next vertical
  interval, and waits for `CompleteNotify` and `IdleNotify` before rewriting the
  pixmap. Only one presentation is in flight. Without Present, the timer uses
  `with_frame_interval()`, and the report indicates `FramePacing::Timer`. Pixels
  are transferred using MIT-SHM 1.2 and file descriptors when available; a
  completion notification permits rewriting them. `PutImage` remains the
  fallback when shared memory cannot be negotiated.
- `set_click_through()` waits for the request to be sent, independently of
  rendering. A timeout does not cancel a command already queued.
- `poll_events()` returns fatal runtime errors. A lost connection closes the
  overlay; backend migration after display is not implicit. Clones consume the
  same event queue: notifications are not broadcast separately to every handle.
  A fatal error is returned once.
- Frame dimensions are physical. `info()` also provides logical dimensions, scale,
  and generation. A frame from another overlay is rejected.
  `WindowInfo.scale_factor` is now an exact `ScaleFactor` in units of 1/120:
  `as_f64()` provides the usual multiplier, `numerator()` the protocol value, and
  `ScaleFactor::ONE` represents 100%. This API change motivates version 0.2.

## Presentation and partial updates

`with_presentation_feedback(true)` requests `FramePresented` notifications in
addition to `FrameSubmitted`. Wayland negotiates presentation-time; X11 CPU uses
Present when available. Clocks, sequences, refresh periods, and hardware flags
remain explicit. Server completion does not measure the monitor's optical
response. The `presentation_timing` example records these values on a fully
transparent surface.

`with_cpu_damage_tracking(true)` compares complete CPU frames to limit uploads
and damage to changed pixels. This option is disabled by default: it retains an
additional image, and comparison can cost more than it saves for fullscreen
animation. Each frame must still contain the complete desired image. The
[measurements](docs/PERFORMANCE.md) compare counters, moving widgets, and
fullscreen changes on both backends.

## Direct GPU rendering

Build with `--features gpu`, then choose a `RendererPreference`:

- `Cpu`: the default behavior, rendering through `frame()` / `draw()` / `present()`.
- `GpuRequired`: requires a Vulkan surface compatible with the requested transparency.
- `GpuPreferred`: tries Vulkan in the selected backend, then retains CPU rendering
  if necessary; `backend_report().warnings` records the fallback reason.

With a GPU renderer, `gpu_context()` exposes the device, queue, format, and alpha
mode. Prepare shaders and pipelines before drawing. `draw_gpu()` queues a
function that receives a texture view, an encoder, and the current dimensions.
This function runs on the display thread: it must remain short and must not call
the window's synchronous commands. An obsolete drawing function may be discarded
before execution. With `panic=unwind`, a panic in a GPU callback becomes
`RuntimePanic` and closes the runtime after releasing the acquired image. The
panic hook remains the application's. Device loss is monitored even while idle
or waiting for a compositor callback.

An executable built with `panic=abort` retains the process termination behavior
of that profile; recovery boundaries cannot intercept that termination.
`downgrade()` provides a `WeakOverlayWindow` for callbacks that need to refer to
the window. `upgrade()` acquires a handle during execution if the window still
has an owner. This weak reference lets RAII cleanup work even when a callback is
waiting for presentation. A strong clone captured in a callback retains ownership
of the window.

Swapchain images are initialized as transparent, then commands are submitted
directly to the GPU without reading pixels back to the CPU. The surface and its
acquired textures remain private to the runtime. The
[`gpu_hud.rs`](examples/gpu_hud.rs) example prepares a WGSL pipeline, then draws an
animated bar with transparent margins. Colors are premultiplied in a non-sRGB
UNORM format. Software adapters are rejected by default; `with_software_gpu(true)`
explicitly permits them for diagnostics.

FIFO mode is requested for vertical synchronization. This mode and
`FrameSubmitted` counts do not prove the screen's physical frame rate. The GPU is
selected only if advertised alpha modes satisfy the requirements:
`PreMultiplied`, or `Inherit` when the window's native alpha is established. An
opaque mode requires `Requirement::Preferred` and is reported as such.

In the tested NVIDIA/XWayland session, Vulkan advertises only `Opaque`. A
transparent overlay with `GpuRequired` is therefore rejected; `GpuPreferred`
preserves alpha by using the CPU. Transparent GPU presentation and 125% scaling
were validated in the native Wayland session. Other drivers may advertise other
capabilities; no desktop list replaces this negotiation.

## Fractional scaling

When `wp_fractional_scale_manager_v1` and `wp_viewporter` are available, the backend
uses the surface's preferred scale, rounds physical dimensions to the nearest
integer with halves rounded up, retains `buffer_scale = 1`, and sets the viewport
destination to logical dimensions. A scale change advances the frame generation
and produces `Resized`; old busy buffers are retained until released. Without
this pair of protocols, the backend retains integer output scaling.

## Keyboard, text, and touch

`KeyboardMode::OnDemand` enables raw keys and their XKB translation, including
keysyms, modifiers, layout, and repeat. For a text field,
`with_text_input(TextInputState::enabled())` also enables local XCompose
composition and integration with the desktop's input protocol. The application
inserts only `TextCommitted` and displays `TextPreedit` separately from the
document. Wayland supports surrounding context through text-input-v3; X11 uses
XIM without surrounding context. XCompose locale tables must be installed for
the local composition fallback.

`InputRegion` restricts clicks and new touch contacts to a union of rectangles.
`set_click_through(true)` hides that region; `false` restores it. `Touch` contacts
distinguish sources and phases, including cancellations. Available capabilities
and interruptions are explicit. The `input_events` example displays these
notifications on its own surface. The [input contracts](docs/INPUT_DESIGN.md)
describe coordinates, limits, and reset handling.

`PointerAxis` provides scroll axes: logical pixels on Wayland and wheel steps on
X11, with positive values pointing down or right.

## Dependencies and portability

Without the `gpu` feature, X11 uses `x11rb::RustConnection` and Wayland uses the
Rust backend of `wayland-client`. It requires neither X11 headers nor linking to
`libX11`, `libxcb`, `libxkbcommon`, or `libwayland-client`. XKB translation,
Compose, and XIM use Rust libraries; locale data remains external. An application's
Cargo feature unification can still enable Wayland's system backend: check your
own dependency graph.

The `gpu` feature uses the Vulkan loader and an ICD driver. It loads
`libwayland-client.so.0` for Wayland, or an additional `libxcb.so.1` connection for
the X11 Vulkan surface. These libraries are loaded dynamically without manual C
compilation. In a binary built with `gpu`, feature unification also enables the
Wayland system backend for that binary's CPU mode. The profile without `gpu`
retains Rust transport and the static musl target.

The GPU profile shares one Vulkan instance initialized once per process. Surfaces
and devices remain specific to each window. This cache avoids repeated driver
discovery and the resource accumulation observed with the tested NVIDIA driver.
The cache retains native resources until process exit; closing a window does not
reset the entire GPU loader.

The `wayland` and `x11` features are enabled by default and can be selected
separately, including with `gpu`. X11 connections use local Unix sockets only.
X11 tracks RandR changes and monitor changes, including those that do not resize
the root window. It recalculates anchoring and, for `Fill`, dimensions. Removing
a named output closes the overlay on both backends. X11 automatic selection
follows the primary monitor or the first available one; Wayland retains the
compositor's policy.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo check --no-default-features --features x11
cargo check --no-default-features --features wayland
cargo test --test x11_protocol alpha_input_region_focus_and_cleanup -- --ignored --nocapture --exact
cargo test --features gpu --test gpu_runtime -- --ignored --nocapture
cargo run --features gpu --example gpu_stability -- cycles 100 /tmp/liboverlay-cycles.tsv
cargo run --features gpu --example gpu_stability -- soak 300 /tmp/liboverlay-soak.tsv
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl --examples
```

The CPU profile targets static musl binaries. The GPU path requires the dynamic
loader and graphics libraries listed above; GPU tests used the GNU target.
Neither profile provides protocols absent from the compositor.

The `x11_protocol` tests are ignored by default. The
`alpha_input_region_focus_and_cleanup` scenario opens its own small window, reads
its pixels and input region back from the server, checks focus, and verifies
its destruction; it requires a local composited X11 display and injects no input
events. The `scripts/check_x11.py --composited --wm` runner adds alpha blending and
click-to-focus checks on private displays. See [TESTING.md](docs/TESTING.md) for
their scope.

Validation includes isolated Xvfb and Sway suites, Rust 1.85, one thousand Vulkan
cycles, two hours of 1920×1080 rendering, and musl binaries executed on Alpine and
Debian. Earlier visual tests also cover real click routing. Results, completed
durations, and limitations are described in the
[validation documentation](docs/TESTING.md). Remaining checks and planned changes
are listed in the [roadmap](ROADMAP.md).

## References

- [Wayland: surfaces, SHM, input regions, and callbacks](https://wayland.freedesktop.org/docs/html/apa.html)
- [layer-shell protocol](https://github.com/swaywm/sway/blob/master/protocols/wlr-layer-shell-unstable-v1.xml)
- [XWayland architecture](https://wayland.freedesktop.org/docs/book/Xwayland.html)
- [SHAPE 1.1](https://www.x.org/releases/X11R7.5/doc/Xext/shape.pdf)
- [EWMH](https://specifications.freedesktop.org/wm/latest-single/)
- [fractional-scale protocol](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/staging/fractional-scale/fractional-scale-v1.xml)
