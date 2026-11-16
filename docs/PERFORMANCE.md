# Performance measurements

## Method

The `benchmark` example submits fully transparent images. The CPU fills a buffer
with zeros; Vulkan clears the swapchain to transparent. Only one application
frame waits for its `FrameSubmitted` event at a time. This workload primarily
measures transfer and presentation, without a text renderer or complex scene.

```sh
cargo build --locked --release --features gpu --example benchmark
python3 scripts/measure_process.py --interval 1 --output /tmp/resources.tsv -- \
  target/release/examples/benchmark --backend wayland --renderer gpu \
  --size 1920x1080 --fps 60 --seconds 10 --output /tmp/frames.tsv
```

`--renderer cpu`, `gpu`, or `prefer-gpu` selects the policy. The report always
identifies the renderer actually used. The frame file contains elapsed time,
submission number, delay between queuing and receiving `FrameSubmitted`, and
physical dimensions. The observation loop waits about one millisecond, limiting
the precision of short delays.

The second file contains CPU time, utilization relative to one logical core,
RSS, descriptors, threads, and, when available, the process's NVIDIA framebuffer
memory. An empty cell means an unavailable measurement, not zero.
`nvidia-smi pmon` may limit measurement to the first adapters and does not provide
these counters on all hardware.

A compositor can suspend callbacks for a hidden surface. The benchmark then
interrupts the sample and counts a presentation pause. A result without usable
submissions fails explicitly. Runtime errors remain propagated.

## Results from November 14, 2026

`--release` build, Rust 1.99.1, Intel Core Ultra 9 285K, NVIDIA GeForce RTX 5070 Ti,
driver 620.70.06. The Sway 1.12 session uses wlroots 0.20, a hardware Vulkan
renderer, and a virtual 3840×2160 output at 100% scale. Each case lasts ten
seconds. Mean CPU and maximum RSS/VRAM exclude the first two seconds. GPU
stability tests were running simultaneously.

These values are short observations in this environment. Compositor, Python
observer, and other process costs are excluded from the measured process's
counters. Rendering memory owned by the compositor for a CPU surface may
therefore exist without appearing as client VRAM.

| Renderer | Physical dimensions | Requested (frames/s) | Submissions/s | CPU (% of one core) | Max RSS (MiB) | Max NVIDIA VRAM (MiB) | Mean submission delay (ms) |
|---|---|---:|---:|---:|---:|---:|---:|
| CPU | 640×360 | 30 | 29.6 | 0.9 | 6.2 | — | 1.15 |
| CPU | 640×360 | 60 | 59.0 | 1.1 | 6.0 | — | 1.11 |
| CPU | 640×360 | 144 | 62.0 | 1.0 | 6.0 | — | 16.10 |
| CPU | 1920×1080 | 30 | 29.4 | 3.6 | 20.1 | — | 1.72 |
| CPU | 1920×1080 | 60 | 58.0 | 5.9 | 20.2 | — | 1.57 |
| CPU | 1920×1080 | 144 | 61.9 | 6.1 | 19.9 | — | 16.14 |
| CPU | 3840×2160 | 30 | 29.5 | 12.7 | 67.5 | — | 5.36 |
| CPU | 3840×2160 | 60 | 58.5 | 25.4 | 67.6 | — | 5.05 |
| CPU | 3840×2160 | 144 | 61.8 | 27.9 | 67.6 | — | 16.15 |
| Vulkan | 640×360 | 30 | 29.6 | 0.9 | 117.6 | 18 | 1.11 |
| Vulkan | 640×360 | 60 | 58.8 | 1.1 | 117.6 | 18 | 1.12 |
| Vulkan | 640×360 | 144 | 61.9 | 1.2 | 117.5 | 18 | 16.13 |
| Vulkan | 1920×1080 | 30 | 29.7 | 0.8 | 117.6 | 46 | 1.09 |
| Vulkan | 1920×1080 | 60 | 59.1 | 1.2 | 117.5 | 46 | 1.38 |
| Vulkan | 1920×1080 | 144 | 61.8 | 1.2 | 117.6 | 46 | 16.16 |
| Vulkan | 3840×2160 | 30 | 29.6 | 0.9 | 117.7 | 112 | 1.11 |
| Vulkan | 3840×2160 | 60 | 59.0 | 1.2 | 117.6 | 112 | 1.30 |
| Vulkan | 3840×2160 | 144 | 61.9 | 1.1 | 117.4 | 112 | 16.14 |

All 18 cases completed without discarded frames or presentation pauses. A request
for 144 frames/s exceeds the rate supplied by this virtual session: the library
waits for the presentation protocol, and submissions settle around 62/s. The
increased delay in these cases includes that wait.

## X11 CPU transfer comparison

The twelve cases below request 60 frames/s for ten seconds, using the same
optimized executable and sampling rules. Xvfb 21.1.26 provides a virtual
3840×2160 screen; picom 13 composites through XRender. A Unix relay shared by all
twelve cases hides selected extensions only from the measured client, preserving
MIT-SHM descriptors. Picom connects directly to the server. The relay adds copying
cost to `PutImage` requests; these results describe this comparison setup and do
not measure Xorg without a relay.

```sh
cargo build --locked --release --features gpu --example benchmark
python3 scripts/benchmark_x11.py --output-dir /tmp/x11-measures
```

Xvfb and picom are required; `XVFB` and `PICOM` select their executables.
`--binary` selects an already built example. `--seconds` and `--fps` change the
duration and requested rate for each case. Each case produces a log and two TSV
files in the requested directory; rerunning in the same directory replaces
these files.

| Transfer | Pacing | Dimensions | Submissions/s | CPU (% of one core) | Max RSS (MiB) | Mean submission delay (ms) |
|---|---|---|---:|---:|---:|---:|
| MIT-SHM | Present | 640×360 | 58.5 | 1.6 | 5.8 | 3.18 |
| MIT-SHM | Present | 1920×1080 | 58.0 | 7.9 | 20.1 | 4.69 |
| MIT-SHM | Present | 3840×2160 | 33.0 | 16.8 | 67.4 | 30.14 |
| MIT-SHM | Timer | 640×360 | 58.5 | 1.6 | 6.0 | 1.12 |
| MIT-SHM | Timer | 1920×1080 | 57.5 | 7.9 | 19.9 | 8.43 |
| MIT-SHM | Timer | 3840×2160 | 52.3 | 23.3 | 67.2 | 19.10 |
| PutImage | Present | 640×360 | 58.7 | 2.3 | 5.9 | 4.31 |
| PutImage | Present | 1920×1080 | 41.8 | 13.7 | 19.7 | 20.64 |
| PutImage | Present | 3840×2160 | 22.2 | 26.0 | 52.0 | 45.01 |
| PutImage | Timer | 640×360 | 57.1 | 2.0 | 6.1 | 16.81 |
| PutImage | Timer | 1920×1080 | 47.1 | 14.2 | 19.9 | 21.16 |
| PutImage | Timer | 3840×2160 | 31.1 | 32.3 | 51.6 | 32.11 |

No case discarded frames or encountered presentation pauses. CPU and RSS counters
cover the client; the relay, Xvfb, and picom are excluded. MIT-SHM achieves a higher
rate than `PutImage` at the two larger dimensions here. The `PutImage` fallback
remains functional, with a higher transfer cost in this setup. Without Present,
the runtime timer is selected; its rate also depends on transfer and server
processing time.

## Scope and follow-up

`FrameSubmitted` describes sending to the server, not scanout. The values above
measure neither input-to-screen latency nor a monitor's physical refresh rate.
Those measurements require external equipment or suitable presentation
timestamps. The additional campaign below separately measures partial updates
in counter and widget scenes.

CPU allocation reuse is active in these measurements. The Wayland and X11
comparisons use different servers and cannot rank the protocols against each
other. The [validation documentation](TESTING.md) describes stability tests and
their hardware limits.

## Partial updates: November 15, 2026 campaign

`with_cpu_damage_tracking(true)` is compared with full transfer across 54 cases:
three sizes (640×360, 1920×1080, and 3840×2160), three scenes, two modes, and three
transports. The scenes are a three-digit seven-segment counter, a moving 96×48
pixel widget, and a changing color across the entire surface. Every frame still
contains the complete scene, with prior clearing; tracking does not reduce the
application's drawing work.

Each case lasts ten seconds and requests 60 frames/s, using an optimized build
with Rust 1.99.1. CPU/RSS counters exclude the first two seconds. CPU percentage
represents one logical core; CPU time per submission is normalized by the
achieved rate to compare cases reaching different rates. Graphics server,
compositor, and relay processes are excluded from these counters.

Sway 1.12 uses the NVIDIA Vulkan renderer and a private 3840×2160 output at 100%
scale. The surface is explicitly bound to `HEADLESS-1`. X11 measurements use
Xvfb 21.1.26 and picom/XRender. A relay identical in both modes hides MIT-SHM only
from the PutImage client; picom retains its own extensions. Captures and colored
patterns remain limited to these private displays.

```sh
cargo build --locked --release --example benchmark
python3 scripts/benchmark_damage.py --output-dir /tmp/damage-measures --seconds 10
python3 scripts/summarize_damage.py /tmp/damage-measures --require-current-sources
```

By default, the runner uses a pixman compositor, suitable for environments
without a GPU. To reproduce the hardware campaign, add `--compositor vulkan`
and, if Sway requires it for this driver, `--unsupported-gpu`. `SWAY`, `XVFB`,
and `PICOM` select the executables. The output directory must be new.

The manifest retains digests of the binary, sources, build inputs, and runner,
along with start and end times. An interrupted case or one deprived of callbacks
is unusable for comparison. The summary treats neither a missing case nor a
missing measurement as zero.

Tracking retains an additional CPU image: 31.6 MiB of pixels at 3840×2160. Its
scan adds cost when almost the entire image changes. The results support an
explicit choice per scene and transport; full transfer remains the default.
An identical Wayland frame emits an empty state commit without new damage or
a frame callback; it does not block future changes.

### 3840×2160 results

Each cell shows “full / partial.” Values are rounded; all 54 complete cases and
their CPU counters are retained in `summary.tsv`.

| Transport | Scene | Submissions/s | CPU per submission (ms) | Max RSS (MiB) |
|---|---|---:|---:|---:|
| Wayland SHM | Counter | 58.6 / 58.5 | 4.39 / 4.10 | 67.0 / 98.5 |
| Wayland SHM | Moving widget | 58.9 / 58.7 | 2.99 / 2.70 | 67.1 / 98.7 |
| Wayland SHM | Entire surface | 58.3 / 58.8 | 6.62 / 8.56 | 66.9 / 98.6 |
| X11 MIT-SHM | Counter | 31.5 / 58.1 | 3.97 / 4.69 | 67.0 / 98.6 |
| X11 MIT-SHM | Moving widget | 31.4 / 58.0 | 5.09 / 4.09 | 67.0 / 98.5 |
| X11 MIT-SHM | Entire surface | 30.8 / 30.6 | 7.10 / 9.03 | 67.1 / 98.6 |
| X11 PutImage | Counter | 20.3 / 58.0 | 14.16 / 4.25 | 51.4 / 67.1 |
| X11 PutImage | Moving widget | 20.2 / 58.0 | 14.91 / 4.74 | 51.4 / 67.7 |
| X11 PutImage | Entire surface | 19.8 / 21.5 | 17.86 / 19.07 | 51.2 / 83.0 |

For the X11 PutImage counter, the rate rises here from 20.3 to 58.0 submissions/s,
with client cost falling from 14.16 to 4.25 ms CPU per submission. On Wayland,
the counter changes from 4.39 to 4.10 ms CPU per submission. The dense Wayland
scene instead rises from 6.62 to 8.57 ms: scanning and caching add overhead.
RSS also depends on temporary transfer buffers, particularly with PutImage.

All 54 cases completed without discards or pauses. The manifest at
`target/validation/damage-release/manifest.json` matches the sources and inputs
used; the executed binary is retained in that directory. The summary was checked
with `--require-current-sources`. These observations quantify this setup and its
scenes; they do not guarantee physical frame rates on all hardware.

## Presentation on a real output

The `presentation_timing` example ran for ten seconds per renderer on output
DP-3 of the Hyprland 0.57.2/NVIDIA session. Each surface measured 64×32 pixels,
was fully transparent, and requested no input. The advertised period is
4,167,187 ns, or about 239.970 Hz. Feedback reports `vsync=true`,
`hardware_completion=true`, and `hardware_clock=false`.

| Renderer | Feedback received | Reported frame rate (s⁻¹) | Median queuing → presentation delay (ms) | P95 (ms) |
|---|---:|---:|---:|---:|
| CPU | 1 164 | 116.51 | 7.199 | 7.722 |
| Vulkan | 1 199 | 119.96 | 7.479 | 7.727 |

The observer waits for feedback before the next frame. This discipline and
session activity limit its rate and the interpretation of renderer rankings.
The frame rates above remain distinct from the monitor's advertised period.
Delays use the server-advertised `CLOCK_MONOTONIC` clock. They end at server
completion and exclude the panel's optical response. Hardware flags are retained
as advertised.

## Virtual input latency

Thirty samples on private Sway measure virtual keyboard → application receipt →
CPU drawing → server presentation. The observed mean is 14.997 ms, median
15.025 ms, and P95 15.868 ms; values range from 14.055 to 15.939 ms. The test
checks that presentation timestamps follow the input and new drawing in the same
clock. The three flags `vsync`, `hardware_clock`, and `hardware_completion` remain
false on this virtual output.

Reference files are `target/validation/desktop/physical-cpu.tsv`,
`physical-gpu.tsv`, `physical-timing.json`, `virtual-latency.tsv`, and
`timing-summary.json`. The `input_latency` example also provides a manual
black/white target and timestamps for an external camera or photodiode. No such
equipment is available; the [hardware procedure](HARDWARE_VALIDATION.md) therefore
retains the outstanding physical and optical measurements.
