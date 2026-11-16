# Hardware validation and optical measurement

## Available coverage

The inventory from November 15, 2026 identifies an NVIDIA graphics controller
`10de:2c05` (RTX 5070 Ti), its `nvidia` driver, two active DisplayPort connectors,
and no device advertising multitouch X/Y axes. No Intel/AMD graphics GPU or
touch panel is available for this campaign. An Intel processor does not
constitute validation of an Intel GPU.

```sh
python3 scripts/inspect_hardware.py --output /tmp/hardware.json
```

The script reads sysfs without changing drivers or configuration. Its result is
an inventory, not a successful rendering test.
`target/validation/desktop/hardware.json` retains this campaign's inventory.

## Additional displays and drivers

On each Intel/AMD test machine, retain the model, driver, session type, and
capability report, then run the CPU and Vulkan diagnostics in the corresponding
graphical session:

```sh
cargo run --example diagnose -- wayland
cargo run --features gpu --example diagnose -- wayland gpu
cargo run --example diagnose -- x11
cargo run --features gpu --example diagnose -- x11 prefer-gpu
```

The report must identify the renderer actually selected and its transparency
limits. A CPU fallback does not validate the GPU. For hardware Xorg, use a real
Xorg session; Xvfb and XWayland do not replace it. Private suites remain useful
for regressions, but passing them does not complete these items.

Current reproducible fullscreen transitions use Openbox/Xvfb, then a real GTK
window with eight transitions beneath a Sway layer-shell overlay. The second
scenario passed with the private NVIDIA Vulkan renderer: focus, clicks, and keys
were checked at every step. Other desktops' and applications' policies must be
recorded in their own test sessions.

## Touch panel

Run `cargo run --example input_events` on an equipped machine. Check multiple
simultaneous contacts, movement, leaving the region, termination, and
cancellation when removing the device. Retain events and the backend report.
Contacts from distinct sources may share an identifier; a new press may reuse
an identifier from a completed contact.

Protocol tests already cover fractional coordinates, cancellations, storage
limits, and seat removal. They do not validate a physical panel's controller,
firmware, or driver.

## Monitor hotplug

Connector tracking is available without emulating a connection:

```sh
python3 scripts/inspect_hardware.py --watch-seconds 60 --output /tmp/connectors.json
```

During this interval, perform actual hotplug in the test session and retain its
state changes. An overlay bound through `OutputSelector::Name` must close if
that output disappears. With automatic selection, separately record the
server's chosen repositioning and the new geometry. A sysfs change alone does
not prove the overlay's reaction. This campaign performed no physical monitor
disconnection or driver change.

## Server-reported presentation

```sh
cargo run --release --example presentation_timing -- \
  --backend wayland --monitor DP-3 --seconds 10 --fps 240 --output /tmp/presentation.tsv
```

Adapt the output name to the session. The surface is small, transparent, and
passive. Each row retains the clock, timestamp, sequence, advertised period,
and hardware flags. Delays between queuing and presentation are calculated only
when the server advertises `CLOCK_MONOTONIC`, the observer's clock. X11 UST does
not advertise its clock, so this delay remains empty for X11.

The rate of frames actually reported and the output's advertised period are two
distinct measurements. The tool waits for feedback before the next frame; this
discipline may limit its rate below the advertised refresh rate.
`hardware_completion` does not imply `hardware_clock`: retain both values.
A virtual output can provide timestamps without any physical scanout.

## Input to light

Physical latency requires an independent trigger reference and optical
observation. The following example provides a manual target:

```sh
cargo run --release --example input_latency -- /tmp/input-latency.tsv
```

A click or a non-repeated Space press toggles the rectangle between black and
white. The example exits after sixty seconds. It generates no input events.
The TSV distinguishes application input receipt from presentation feedback.
Only one transition is tracked at a time; the target waits for its feedback
before accepting the next one.

With a high-speed camera, observe the trigger reference and luminance change
simultaneously. With a photodiode, record the electrical trigger and luminance
on the same time base. Specify sampling frequency, sensor position, threshold,
sample count, and uncertainty. Camera resolution, rolling shutter, scan position,
and pixel response must remain distinct from software delay.

The `virtual_input_to_presentation` test already measures virtual keyboard →
application event → new drawing → server presentation. It excludes the physical
device and optical response. No result from this campaign constitutes a
button-to-light measurement or an optical frame-rate measurement.
