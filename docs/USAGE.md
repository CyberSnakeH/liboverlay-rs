# Usage

These examples assume a local Linux desktop session. The Cargo package is
`liboverlay-rs` and the imported module is `liboverlay`.

## Dependency

```toml
[dependencies]
liboverlay-rs = { git = "https://github.com/CyberSnakeH/liboverlay-rs", branch = "main" }
```

For Vulkan, add `features = ["gpu"]`. The `wayland` and `x11` features are enabled
by default; use `default-features = false` to select backends explicitly.

## Creating and closing a window

```rust,no_run
use liboverlay::{Anchor, OverlayBuilder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let overlay = OverlayBuilder::new()
        .with_title("Monitoring")
        .with_size(360, 120)
        .with_anchor(Anchor::TopRight)
        .with_margin(16)
        .with_click_through(true)
        .build()?;

    println!("{:#?}", overlay.backend_report());
    overlay.close()?;
    Ok(())
}
```

An application retains the handle throughout its rendering loop. The handle
can be cloned and shared between threads. `close()` closes the window for all
handles and waits for display-thread cleanup.

## CPU drawing

`draw()` receives a pixel array and physical dimensions. Allocations may be
recycled, but every new frame starts transparent. Pixels use `0xAARRGGBB` with
premultiplied alpha; `premultiply()` performs the conversion. Zero pixels are
transparent.

```rust,ignore
overlay.draw(|pixels, width, height| {
    pixels.fill(0);
    for y in 12..height.min(80) {
        for x in 12..width.min(240) {
            pixels[(y * width + x) as usize] = liboverlay::premultiply(30, 200, 100, 160);
        }
    }
})?;
```

To control allocation and submission separately, use `frame()` followed by
`present(frame)`. A successful submission means the frame was queued.
`FrameSubmitted` reports delivery to the display server without confirming
physical display.

The queue may be full and geometry may change between allocation and submission.
A rendering loop handles `QueueFull` and `OutdatedFrame`, retrieves the current
information and redraws. The [CPU HUD](../examples/basic_hud.rs) demonstrates
this behavior.

## GPU drawing

Build with `gpu`, then select `RendererPreference::GpuRequired` or `GpuPreferred`.
The former requires Vulkan; the latter can fall back to the CPU and records the
reason in the report. Check the selected renderer before calling CPU- or
GPU-specific methods.

`gpu_context()` supplies the device, queue, format and alpha mode. Create shaders
and pipelines before drawing. `draw_gpu()` takes a callback that encodes one
frame's commands through `GpuFrame`. This callback runs on the private thread
and must remain short, without synchronously calling `close()`,
`set_click_through()`, `set_input_region()` or `set_text_input()`.

A callback that needs to reference the window can capture `overlay.downgrade()`
and call `upgrade()` when it runs. This weak reference does not extend the
runtime's lifetime. Capturing an owning clone retains it while the callback is
pending, including when the compositor withholds callbacks.

The [Vulkan HUD](../examples/gpu_hud.rs) demonstrates a complete WGSL pipeline.
Colors must use premultiplied alpha. Software adapters are rejected by default;
`with_software_gpu(true)` explicitly permits their use.

## Presentation feedback

Enable `with_presentation_feedback(true)` during creation, then check
`capabilities.presentation_feedback`. `FramePresented(PresentationFeedback)`
associates a local submission ID and generation with the server timestamp,
sequence and quality flags. The counter starts at one for tracked frames after
initialization. It is local to each window.

Wayland supplies its POSIX clock ID and the `vsync`, `hardware_clock`,
`hardware_completion` and `zero_copy` flags. X11 supplies UST and MSC for the CPU
renderer with Present without negotiating the clock identity or these hardware
flags. The X11 GPU path does not expose this tracking. Comparing a timestamp
with an application timestamp requires knowing that both use the same clock;
an absent value or false flag must not be replaced with an estimate.

`PresentationDiscarded` means the server discarded the feedback, the limit of
64 pending Wayland requests was reached, or an unchanged CPU frame needed no
new content. Its empty state commit retains the `FrameSubmitted` notification
without waiting for another drawing callback. This differs from `FrameDiscarded`,
which describes a frame replaced or invalidated in the runtime. Notifications
remain subject to the usual public queue limit.

## Partial CPU updates

`with_cpu_damage_tracking(true)` enables comparison with the last image accepted
by the backend. Supply a complete frame even when only one widget changes:
recycled frames still start transparent. The runtime can replace a pending
frame; differences are calculated when a frame is accepted so that changes from
replaced frames are also retained.

The backend maintains one additional CPU image and calculates the changed pixel
bounds. X11 limits MIT-SHM or PutImage to that rectangle. Wayland uses these
physical bounds for damage, rewrites only the required rows when the buffer is
current, and restores a complete image when another buffer is reused. A new
generation forces a full update. An identical Wayland image does not wait for
an unnecessary rendering callback; later changes can still be processed.

This mode does not change GPU drawing. It adds an image scan and can be more
expensive for dense scenes. Full transfer remains the default; see the
[measured comparisons](PERFORMANCE.md).

## Input and geometry

`poll_events()` returns available notifications from a queue shared between handle
clones. Regions determine where pointer and touch sequences begin; they remain
independent of rendering and keyboard focus.

```rust,no_run
use liboverlay::{InputRectangle, InputRegion, OverlayBuilder, TextInputState};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let overlay = OverlayBuilder::new()
        .with_size(320, 120)
        .with_input_region(InputRegion::Rectangles(vec![
            InputRectangle::new(16, 16, 288, 88),
        ]))
        .with_text_input(TextInputState::enabled())
        .build()?;
    overlay.set_click_through(true)?;  // temporarily hide the region
    overlay.set_click_through(false)?; // restore the rectangles
    overlay.set_input_region(InputRegion::Full)?;
    overlay.set_text_input(TextInputState::default())?; // disable the field
    overlay.close()?;
    Ok(())
}
```

Rectangles are half-open, in surface-local logical coordinates on Wayland and
server pixels on X11. Outer margins remain click-through. The region retains its
coordinates after resizing or scaling. `set_input_region()` immediately enables
the new region by disabling click-through.

Keyboard input is not requested by default. `KeyboardMode::OnDemand` enables
`Key` and `KeyboardInput` without requiring a text field. `with_text_input()`
with an enabled field also selects OnDemand mode. Keyboard permission must be
present at creation to call `set_text_input()` later.

To edit a document, insert only `TextCommitted`. Do not also insert
`KeyboardInput.text`: that field describes the key before composition. Display
`TextPreedit` separately and apply `TextDeleteSurrounding` before the associated
commit. Cursor positions and deletion sizes use UTF-8 bytes. Wayland can receive
a `SurroundingText` of up to 4,000 bytes with valid UTF-8 boundaries; Password/Pin
fields reject this context. X11 provides XCompose and XIM without surrounding
context. Locale Compose tables are required for dead keys outside an input
method.

`Touch` associates a source and ID with every contact, using the `Started`,
`Moved`, `Ended` and `Cancelled` phases. `TouchFrame` ends a Wayland batch. On
`InputReset`, clear the state identified by `scope` for the specified source,
or all sources when none is specified. An overflow reset indicates that edits
may have been lost: resend text context after discarding preedit. See the
[complete contracts](INPUT_DESIGN.md).

`PointerAxis { axis, value, unit }` provides horizontal or vertical scrolling.
Positive values move down or right. `ScrollUnit::Pixels` means Wayland logical
pixels; `Steps` means X11 wheel steps. Conversion to content movement is the
application's responsibility.

`info()` returns logical dimensions, physical dimensions, scale factor and the
current generation. CPU frames use physical dimensions. `ScaleFactor::as_f64()`
returns the conventional factor and `numerator()` its exact value in 120ths.

`OutputSelector::Name` binds the window to that output: losing it closes the
overlay. X11 automatically recalculates placement and `Fill` dimensions after
monitor changes. Negative coordinates are supported. The capability report
remains a startup snapshot.

## Capabilities and errors

`backend_report()` describes the backend, renderer, transparency, input,
stacking, pacing, warnings and fallback attempts. `WaylandOnly` or `X11Only`
forces one backend. `Requirement::Required` preserves a strict requirement;
`Preferred` permits the documented limitations.

Errors are returned through `Result`. A fatal runtime error is reported by
`poll_events()`. Losing a connection closes the overlay. A synchronous command's
timeout does not cancel an already queued command; user callbacks must therefore
also avoid blocking. Clones share a consumable event queue; a fatal error is
returned once before pending notifications. With `panic=unwind`, a GPU callback
panic closes the runtime with `RuntimePanic`. A `panic=abort` profile stops the
process on a panic. CPU drawing panics occur on the calling thread and propagate
there.

## Available examples

| Example | Purpose |
|---|---|
| `basic_hud` | Animated CPU drawing and backend selection |
| `interactive_toggle` | Switching between click-through and interaction |
| `input_events` | Interactive region, keyboard translation, composition and console contact events |
| `diagnose` | Backend report, geometry and selected renderer |
| `gpu_hud` | WGSL pipeline and Vulkan presentation |
| `gpu_stability` | Create/close cycles and prolonged rendering with measurements |
| `benchmark` | CPU/Vulkan submission rate and latency; private damage-comparison scenes |
| `presentation_timing` | Server presentation timestamps and flags on a transparent surface |
| `input_latency` | Manual black/white target for an external camera or photodiode |
| `text_hud` | CPU fontdue text, supplied TTF/OTF font and premultiplied alpha |

Commands and reference results are collected in [TESTING.md](TESTING.md).
