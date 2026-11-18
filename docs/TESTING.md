# Testing and validation

## Automated checks

The egui/C++ integration adds a Rust ABI suite, a C test linked to the library,
a C++ application-logic test and a private graphical launcher. See [CPP.md](CPP.md)
for this bridge's commands and limitations. The historical results below predate
that extension and do not establish its hardware validation.

From the repository root:

```sh
sh scripts/check.sh
cargo check --locked --no-default-features
cargo check --locked --no-default-features --features x11
cargo check --locked --no-default-features --features wayland
cargo check --locked --no-default-features --features gpu
cargo check --locked --no-default-features --features gpu,x11
cargo check --locked --no-default-features --features gpu,wayland
```

The suite covers allocation limits, premultiplied alpha, scale rounding, foreign
or outdated frames, bounded queues, Present notification order and thread
cleanup. The public type is checked for `Send + Sync` at compile time.

The default and all-features suites each passed 15 tests, including documentation,
during historical validation with Rust 1.97.1. The extended suite also runs with
Rust 1.85.0 and 1.99.1. With the new input support, it contains 29 unit tests with
all features, two display-independent API tests and one doctest; graphical
scenarios run separately in their private sessions. Formatting and Clippy are
checked with both compilers. Graphical tests remain ignored by default and
require their test environment.

`scripts/check.sh` includes examples, doctests, warning-free rustdoc, error paths
and a verified Cargo package build. All public items must be documented. The
GitHub Actions workflow covers stable, 1.85, the feature matrix including egui,
musl, Xvfb and Sway; remote execution requires pushing the revision to GitHub.

## Isolated graphical sessions

```sh
python3 scripts/check_x11.py --composited --wm
python3 scripts/check_wayland.py
python3 scripts/check_wayland.py --gpu
```

Xvfb and Sway must be available; `XVFB` and `SWAY` select their paths.
`--composited` also requires picom, selectable through `PICOM`. `--wm` requires
Openbox, selectable through `OPENBOX`. `--gpu` uses lavapipe, selectable through
`LAVAPIPE_ICD`. These suites create their own displays and do not take control
of the current session's compositor. Wheel, click and keyboard injection is
limited to private servers. The Wayland suite creates its own virtual devices
and input method without taking input control over the user session.

The X11 suite checks pixels after multiple uploads and resizes, all four
Present/MIT-SHM combinations, placement on monitors with negative coordinates,
monitor movement and removal, and server loss. Xvfb displays are not composited:
`Preferred` policy and the transparency report are checked there. Present cannot
be disabled directly in Xvfb; a proxy hides its `QueryExtension` response and
forwards other messages and memory file descriptors. The composited profile
starts picom in its own Xvfb, checks input regions and focus, then reads only
the composited pixels over the test's black background. This verifies alpha
blending without capturing the desktop. The `--wm` profile starts Openbox with
a temporary configuration and waits for startup to finish. A test-owned background
window enters fullscreen. Neither passive nor interactive overlays may steal
its focus. The interactive overlay remains undecorated, respects its placement
and then receives focus on a simulated click; closing it returns focus to the
background. This observation does not guarantee identical stacking with all
window managers or fullscreen applications.

The Wayland suite uses two virtual Sway outputs with different scales. It checks
rapid dimension and scale changes, outdated frames, removal of a named output,
connection loss and creation after compositor restart. A proxy can hide
layer-shell, fractional-scale or viewporter, or withhold real frame callbacks.
Input and closing remain functional. Vulkan cases check missing drivers,
transparency requirements, default rejection of software rendering, lavapipe
permission and CPU fallback after texture limits are exceeded. Device loss
remains detectable while waiting for a callback. Geometry changes are verified
with CPU and software Vulkan rendering, including submission of the current
generation after each change.

## Validation of new input support

The November 15 campaign adds these scenarios to the private suites:

| Area | Checks |
|---|---|
| X11 regions | XTEST clicks on half-open edges, overlaps, negative coordinates, Full/Empty, hide/restore and enlargement |
| Wayland regions | Actual Sway routing to the background or overlay; identical logical coordinates at 100% and 200% scale; commands without a rendering callback |
| X11 keyboard | us/fr keymap changes, translation and dead keys through XTEST events |
| Wayland keyboard | Private virtual keyboard, us/fr groups, Shift, composition, repeat and stopping on release |
| XIM | Controlled server: Japanese preedit, UTF-8 cursors, synchronized commit, keys returned in order, ResetIC and server disappearance without closing the overlay |
| text-input-v3 | Independent input-method-v2 implementation in Sway: activation on layer-shell, keyboard capture, preedit, deletion followed by commit without duplication, updated surrounding context and deactivation |
| Wayland touch | Two proxy-controlled seats, simultaneous contacts sharing an ID, fractional and negative coordinates, cancellation, seat removal and ignored orphan events |
| X11 touch | Serialized then decoded XI2 packets: fixed-point coordinates, master/source, window filtering and cancellation per device |
| Bounds and interruptions | Invalid regions, UTF-8 boundaries, hidden-context protection, contact/key limits, overflow and reset scopes |

IME exchanges use the real client implementation and, on Wayland, the real
compositor implementation. Controlled input servers are supplemented by tests
with actual IBus/Anthy and Fcitx5/Hangul, described below. Coverage does not extend
to all their versions or every desktop integration. Touch is validated at the
protocol and contact-tracking level; no physical touch device was used. Virtual
IDs and proxies do not establish a touch-panel driver's behavior.

Detailed results are retained in `target/validation/input-x11.log`,
`input-wayland-gpu.log`, `input-quality.log` and `input-msrv.log`. New dependencies
preserve Rust 1.85 compatibility: `encoding_rs` is constrained to compatible
version 0.8.35. CPU examples, including `input_events`, are built for musl and
inspected to exclude an ELF interpreter and required dynamic linkage.

The same new musl diagnostic binary also submitted 39 frames and closed normally
in each Alpine 3.22 and Debian 12 userspace, connected to private Sway. The six
hardware Vulkan tests were rerun, including rejection of the new synchronous
commands from a callback. An additional 30-second run at 1920×1080 submitted
1,859 frames without pauses or errors; after closing, the process retained
28 file descriptors and two threads. Logs are in `input-containers.log`,
`input-gpu-runtime.log` and `input-gpu/`. This short run checks immediate
regressions; historical two-hour endurance is documented separately.

## Additional compatibility and measurements on November 15

```sh
python3 scripts/check_desktop_ime.py --framework ibus
python3 scripts/check_desktop_ime.py --framework fcitx5
python3 scripts/check_desktop_ime.py --framework fcitx5 --backend wayland
python3 scripts/check_fullscreen.py
```

IBus 1.5.35 with Anthy 1.5.18 committed Japanese `にほん` through XIM. Fcitx5
5.1.23 with Hangul 5.1.11 committed Korean `한글` through XIM, then natively
through text-input-v3 on Sway. The Fcitx XIM scenario temporarily enables
`UseOnTheSpot=True` to receive preedit through callbacks; a second run with
`--server-preedit` also checks the server-managed style. Intermediate commits
are concatenated and checked for duplication. Each process uses its own D-Bus
bus and configuration directories; the user session's input method is not
replaced.

These tests exposed and helped fix two defects: waiting for acknowledgements
on asynchronous XIM keys, and clearing the preedit buffer too early after a
commit. The controlled scenario also reproduces a commit immediately followed
by incremental preedit to prevent this regression.

Eight fullscreen transitions of a real GTK window were exercised beneath an
interactive Sway overlay with a private NVIDIA Vulkan renderer. Clicks and keys
reached the overlay at every stage. Existing Openbox/Xvfb tests still pass.
These tests do not guarantee every desktop's policies.

Partial updates are checked by reading complete X11 pixels with and without
MIT-SHM/Present, then capturing only the private Sway display. Images include
movement, erasure, identical frames, size changes and scale changes. The identical
case also checks that no unnecessary drawing callback blocks a later update.
A proxy exercises the absence of presentation-time; capabilities remain explicit
and rendering works.

The virtual latency test retains thirty samples on private Sway;
`presentation_timing` retains timestamps and flags advertised by the real
Hyprland session. The latter observation uses a transparent, passive surface.
Results are in [PERFORMANCE.md](PERFORMANCE.md). The six hardware Vulkan scenarios
and GPU presentation tracking were also rerun in private Sway. Logs are in
`target/validation/desktop/`.

The [hardware inventory and procedures](HARDWARE_VALIDATION.md) identify tests
that cannot be performed with the available equipment: Intel/AMD GPUs, a touch
panel, physical hotplug and optical measurements. Their absence is not counted
as a pass.

## Graphical tests

```sh
cargo test --features gpu --test gpu_runtime -- --ignored --nocapture --test-threads=1
cargo test --test x11_protocol alpha_input_region_focus_and_cleanup -- --ignored --nocapture --exact
cargo test --features gpu --test visual_input manual_wayland_visual_clicks -- --ignored --nocapture --exact
```

- `gpu_runtime` checks closing, resources that outlive the window, rejection of
  synchronous commands in a callback and independence of two GPU windows using
  the same Vulkan instance. It also checks controlled device destruction and a
  callback panic with typed error reporting. Destruction during the callback is
  followed by a new creation and draw to verify that the shared instance remains
  usable. On wgpu 25 this scenario can invoke the panic hook during discard
  validation; the library recovers this error and releases the image before
  the surface. These tests use `panic=unwind`; they do not guarantee panic
  recovery in an executable configured with `panic=abort`.
- `x11_protocol` reads back its own window's pixels and checks input regions,
  focus and window destruction.
- `manual_wayland_visual_clicks` displays a checkerboard and two colored bands.
  Three real clicks must reach the background, overlay and background in that
  order. The green band is semitransparent; the red band is opaque.

`visual_input` also includes capture tests through the KWin interface and XTEST
click tests. Captures depend on normal compositor permissions. The XTEST test
checks the window beneath the pointer before sending a button and requests
restoration at the end. Run these tests individually in a test session, using
the names listed by `cargo test --test visual_input -- --list`.

## GPU stability

```sh
cargo run --features gpu --example gpu_stability -- cycles 1000 /tmp/liboverlay-cycles.tsv 1024 768
cargo run --features gpu --example gpu_stability -- soak 7200 /tmp/liboverlay-soak.tsv 1920 1080
```

For a long run independent of desktop visibility, this launcher creates a
private Sway session with a virtual output and Vulkan renderer:

```sh
cargo build --locked --features gpu --example gpu_stability
python3 scripts/check_gpu_stability.py --output-dir target/validation/gpu
```

Sway must be available or selected through `SWAY`. The driver must provide the
wlroots Vulkan renderer and a hardware Wayland Vulkan surface. For a proprietary
driver rejected by Sway by default, `--unsupported-gpu` forwards its explicit
acceptance option. `--render-node` selects the compositor's DRM node; `--width`,
`--height` and `--seconds` configure the run. The program log records the GPU
actually selected by the library.

The results directory retains logs, two TSV files, binary and source digests,
start/end times and the exit code. A manifest without an end time or exit code
does not establish a successful test. Choose a new directory for each campaign
to preserve measurements. Temporary sockets and the private compositor are
cleaned up on normal exit.

The program requires a hardware adapter and creates a fully transparent passive
surface. `LIBOVERLAY_VALIDATION_VISIBLE=1` explicitly enables the colored pattern.
Logical dimensions are configurable; the default remains 160×90. It records RSS,
file-descriptor count and process thread count. Cycles alternate explicit closing
and destruction of the last handle, retaining application-owned GPU resources
until after closing. Prolonged rendering encodes WGSL commands and waits for
submitted queue work to finish. It accepts up to 24 hours of rendering or
100,000 cycles. Compositor presentation pauses are recorded as `soak_paused`,
with pause count and the largest submission gap in the final result. GPU errors,
unexpected runtime closure and no submissions at all remain failures. A pause
does not establish continuous rendering during that interval.

To measure CPU, RAM and VRAM separately for the process alone:

```sh
cargo build --locked --features gpu --example gpu_stability
python3 scripts/measure_process.py --output /tmp/process.tsv -- \
  target/debug/examples/gpu_stability soak 7200 /tmp/soak.tsv 1920 1080
```

NVIDIA VRAM comes from `nvidia-smi pmon`, which includes graphics processes.
Unavailable measurements remain empty. The protocol and size/rate comparisons
are detailed in [PERFORMANCE.md](PERFORMANCE.md).

The `instances` diagnostic mode deliberately creates distinct instances,
bypassing the library cache. It isolates costs or resources retained during
driver discovery.

## Validation on November 14 and 15, 2026

Environment: Fedora, Rust 1.85.0 and 1.99.1, NVIDIA GeForce RTX 5070 Ti, driver
620.70.06, a Hyprland session and private Sway 1.12/wlroots 0.20 sessions.
Xvfb 21.1.26 supplies isolated X11 tests. Test executables downloaded for this
validation are extracted into a working directory without system installation.
The suites and thousand-cycle run were repeated on November 15; campaign logs
are retained in `target/validation/2026-11-15`. Performance comparisons date
from November 14.

| Test | Result |
|---|---|
| Local suites, formatting, Clippy, rustdoc | Passed with Rust 1.85.0 and 1.99.1 |
| X11 with/without MIT-SHM and Present | Pixels, resizing and closing verified |
| X11 composited alpha | RGB values verified over a private black background with picom |
| X11 focus and fullscreen | Passive stacking, opening without activation, placement, no border, focus on click, raw keys and focus restoration verified on private Openbox 3.6.1 |
| Virtual X11 monitors | Movement, Fill dimensions, negative coordinates and removal verified |
| Wayland protocols and withheld callbacks | Fallbacks, input and closing verified on private Sway |
| Mixed scales and geometry | Rapid changes, frame generation and output removal verified |
| Connection loss | Error and shutdown observed on private Xvfb and Sway; new creation after restart |
| Missing/software Vulkan | CPU fallback, default rejection and explicit permission verified |
| Controlled device loss | GPU error while idle and while waiting for a callback |
| GPU callback panic | `RuntimePanic`, shutdown and cleanup without process termination |
| One thousand Vulkan cycles at 1024×768 | Passed, 3,000 submitted frames, 28 descriptors and 2 threads after each close |
| Cycle RSS after initialization | Maximum 220,188 KiB over cycles 10–19; 220,484 KiB over the final ten |
| Process VRAM during cycles | Twelve NVIDIA samples from 7 to 29 MiB, without claiming coverage of every cycle |
| Prolonged rendering at 1920×1080 | Two hours completed, 445,418 submitted frames, no pauses or errors |
| Rendering RSS after the first minute | From 149,380 to 151,960 KiB; 133,776 KiB after closing |
| Prolonged-rendering VRAM | 50 MiB over all 720 available NVIDIA samples |
| Prolonged-rendering descriptors and threads | 58–61 descriptors and 7 active threads; 28 descriptors and 2 threads after closing |
| Optimized benchmark | 18 Wayland and 12 X11 cases of ten seconds, without discard or pause; results in PERFORMANCE.md |
| musl CPU | Wayland rendering in Alpine 3.22 and Debian 12; XWayland rendering on the host |

The November 15 endurance run took place from 01:14 to 03:14 UTC on a private
Sway 1920×1080 virtual output at 100% scale with a hardware Vulkan renderer.
The test exited with code 0 after 7,200 seconds of rendering. The largest
measured submission interval, including final cleanup, was 67 ms; it does not
measure scanout. Manifest digests match the executed binary and sources checked
at that time. This campaign predates the new input support and does not establish
two-hour endurance for its final revision. `target/validation/2026-11-15` retains
the `results.json` summary and raw `gpu/soak.tsv` and `gpu/process.tsv`
measurements.

An initial transparent run on the user session stopped its old watchdog after
about 24 minutes without a reported GPU error: submissions had been suspended
for five seconds. This observation motivated the distinction between presentation
pauses and runtime errors. It does not establish successful continuous two-hour
rendering. Retried runs retain resource/error tracking; a private GPU compositor
also permits measurement independent of user-session visibility.

Another Hyprland attempt ended after about 96 minutes with `Closed`, following
presentation suspensions. The cause of closing was not established. This run
does not validate two hours of continuous rendering. Campaigns without a retained
final result are not counted as passes.

## Historical graphical results

Tests on October 29, 2026 used KDE/KWin Wayland with an NVIDIA GeForce RTX 3070 Ti
Laptop GPU, driver 580.178.04 and 125% scale:

| Test | Result |
|---|---|
| 100 GPU create/close cycles | Passed, 300 submitted frames; 30 descriptors and 3 threads after each close |
| Cycle RSS | Plateau after initialization: maxima of 155,668 KiB over cycles 10–19 and 155,828 KiB over the final ten |
| Continuous rendering for 300 s | 18,504 submitted frames, no stop or five-second submission gap |
| Rendering RSS after ten seconds | Between 146,692 and 146,696 KiB |
| Two simultaneous GPU windows | Closed the first, then drew again on the second: passed |
| Real clicks | Press and release received only by the expected target at all three stages |
| Composited appearance | Checkerboard visible through the green band and margins; opaque red; appearance confirmed during manual inspection |

Repeated creation of distinct Vulkan instances had exposed file-descriptor
accumulation in this configuration. Sharing one instance per process avoids
that growth in the tests performed. This cache deliberately retains native
resources until process exit.

On the tested NVIDIA/XWayland session, Vulkan advertises only an opaque alpha
mode. Required GPU rendering with required transparency explicitly fails;
GPU-preferred mode then uses the CPU and preserves transparency. Opacity must
be explicitly permitted to use this X11 GPU surface.

KWin did not permit automatic captures of the composited desktop. Visual
validation relies on manual observation; tests verified click events and X11
window pixels.

## Error paths and musl

```sh
cargo build --example diagnose
python3 scripts/check_failure_paths.py
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl --examples
```

The script uses a temporary Unix socket for a silent Wayland server. No display,
an out-of-range X11 number and this silent server produced typed errors without
panicking; the final scenario times out after about five seconds.

The musl CPU examples ran in the reference session with fractional Wayland
scaling, XWayland Present and automatic backend fallback. ELF inspection showed
neither `DT_NEEDED` entries nor a dynamic interpreter. The tested GPU path uses
the GNU target and dynamically loaded native libraries. November 14 validation
also built musl examples with Rust 1.85.0 and verified the absence of `DT_NEEDED`
entries and an ELF interpreter. The same diagnostic binary submitted Wayland
frames in Alpine 3.22 and Debian 12 by connecting to private Sway on the host.
This checks the static client in those userspaces, rather than two distinct
physical desktop installations.

## Coverage limitations

Device and connection loss are controlled and do not reproduce every hardware
failure. Virtual monitors and proxies exercise real protocol paths but do not
replace plugging in a cable. Measurements do not cover physical display cadence,
display latency, hardware Intel/AMD GPUs, a hardware Xorg session or every
fullscreen mode. The [roadmap](../ROADMAP.md) keeps these checks open.

## Generic C/C++ API

`cargo test -p liboverlay-c` covers command validation, string copying, Hyprland
geometry and fragmented/repeated shortcut events. `ctest --test-dir target/cpp`
also checks the generic C ABI. `python3 scripts/check_canvas.py` starts private
Sway and Xvfb/picom sessions: thread contracts, invalid-frame rejection, input
modes, rendering, checkbox/slider clicks and closing with actions returned to
C++. Hyprland shortcut tests require that real session and remain separate from
Sway tests. The library explicitly exposes unsupported integrations. See
[CPP.md](CPP.md).

## Source distributions and native Xorg on November 18, 2026

Two external projects exercise the user-facing archives. The Rust client built
with Cargo and rustc 1.85.0 and again with 1.99.1. Each run opened a Wayland
surface, presented an opaque green CPU frame from the extracted crate and
explicitly closed the window after the compositor capture confirmed its pixels.

The C++17 client (GCC 16.2.1, CMake 4.3.0) uses the public CMake target and
compiles the bridge from the full archive. The scene checks lines, filled
circles and triangles, visible glyph strokes, replacement and reset of the clip
rectangle, and two layers of 50% alpha over black. Red alone measured
`(128, 0, 0)`, blue alone `(0, 0, 128)`, and blue over red `(64, 0, 128)`.
The test checks the interior colors and the background around the geometry,
including all four sides of the clip.

| Private display | 100% | 125% |
|---|---|---|
| Sway, pixman compositor, lavapipe client | 640 × 480 output, scale 1 | 800 × 600 output, negotiated scale 1.25, identical logical scene |
| Xvfb, picom XRender compositor, lavapipe client | 640 × 480, native screen pixels | 800 × 600, application multiplies coordinates, stroke widths and font size by 1.25 |

Wayland captures use grim at the output scale. X11 captures read the composited
root window. X11 canvas coordinates are screen pixels, so the 125% case checks
application scaling. The [distribution procedure](DEVELOPMENT.md#preparing-a-release)
reproduces these checks outside the checkout.

The Xorg run used Fedora 45 userspace and kernel 7.3.3 in a separate x86-64
QEMU guest with TCG, two virtual CPUs, 1200 MiB of RAM, standard VGA and the
Bochs DRM driver. Xorg 21.1.26 opened the guest's `/dev/dri/card0` with the
`modesetting` driver on a 640 × 480 virtual output. Acceleration was disabled.
The guest ran the CPU backend without a host display or network connection.

| Guest session | Tests passed | Observation |
|---|---|---|
| Bare Xorg | `opaque_upload_resize_and_cleanup`, `partial_updates_preserve_unchanged_pixels_and_resize` | CPU uploads, preserved pixels, resizing and cleanup |
| Xorg with picom (XRender) | `composed_alpha_matches_premultiplied_pixels`, `alpha_input_region_focus_and_cleanup` | Composited RGB values, ARGB readback, input regions and unchanged focus; the backend selected XPresent |
| Xorg with Openbox and picom | `keyboard_focus_and_fullscreen_policy` | Stacking, focus on click, delivery of raw keys and return of focus to the background |

To repeat in a disposable guest, configure a single 640 × 480 Xorg output.
Build the two test binaries with the following command and run the named tests
in the table in order, each with `--exact NAME --ignored --nocapture`, with
`DISPLAY` pointing to the guest Xorg server.

```sh
cargo test --locked --no-default-features --features x11 \
  --test x11_headless --test x11_protocol --no-run
```

The bare-server tests belong to `x11_headless`. For `x11_protocol`, start
`picom --config /dev/null --backend xrender` and set
`LIBOVERLAY_TEST_PRIVATE_COMPOSITOR=1`. For the last case, use the Openbox
configuration and startup readiness marker from `scripts/check_x11.py --wm`
and set `LIBOVERLAY_TEST_PRIVATE_WM=1`. Input events target only the test
windows on that guest. This run covers virtual KMS and CPU rendering. Physical
Xorg drivers, Vulkan on Xorg hardware and monitor hotplug remain unverified.
