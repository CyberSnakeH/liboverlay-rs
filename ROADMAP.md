# Remaining work

The 0.2 baseline includes Wayland layer-shell and X11/XWayland overlays,
configurable click-through, CPU rendering, optional Vulkan, X11 MIT-SHM and
Present, Wayland fractional scaling, and X11 monitor tracking. Checked items
represent completed implementation and validation within the scope documented
in [TESTING.md](docs/TESTING.md). Hardware coverage remains distinct from virtual
server coverage.

## Priority 1: robustness and coverage

- [x] Validate two hours of Vulkan rendering on a 1920×1080 surface with resource tracking.
- [x] Measure VRAM separately and check resources after one thousand cycles.
- [x] Test connection loss during rendering and compositor restart in an isolated
  session; verify errors, shutdown, and resource release.
- [x] Test controlled Vulkan device loss and rejected surface configurations.
- [x] Exercise concurrent handle calls, including shutdown and drawing from multiple threads.
- [x] Verify rapid scale and size changes on Sway, along with hidden windows and
  windows deprived of frame callbacks.
- [x] Configure builds, tests, formatting, Clippy, rustdoc, musl, and isolated
  sessions in GitHub Actions. Remote execution follows publication of the revision.
- [x] Run the suite with the declared minimum compiler, Rust 1.85.

## Priority 2: multiple monitors and compatibility

- [x] Reposition the X11 overlay when monitors change, with virtual RandR validation.
- [x] Verify negative X11 coordinates and virtual Wayland outputs with mixed scales.
- [x] Define and test behavior when a selected output disappears.
- [x] Validate a server without Present and a non-composited display on Xvfb.
- [x] Extend testing to Sway, another NVIDIA generation, and lavapipe.
- [x] Test missing layer-shell, fractional-scale, or viewporter and their associated fallbacks.
- [x] Check missing, incompatible, and software Vulkan drivers while respecting
  transparency requirements and explicit permission to use software rendering.
- [x] Check focus in both X11 modes against a fullscreen background on private Openbox.
- [x] Check eight fullscreen transitions of a real GTK window beneath an interactive
  Sway overlay, using the private NVIDIA Vulkan renderer.
- [ ] Complete stacking and fullscreen transition checks on the other target
  hardware sessions, according to each desktop's policies.
- [x] Run the same CPU musl binary on Alpine 3.22 and Debian 12, connected to a
  private compositor on the host.
- [x] Run CPU rendering, composited alpha, focus and fullscreen tests on native Xorg in a QEMU guest.
- [ ] Complete testing on a hardware Xorg session and physical Intel/AMD GPUs.
- [ ] Check physical monitor hotplug and driver transitions not reproduced by
  virtual outputs.

## Priority 3: performance

- [x] Add MIT-SHM for X11 CPU transfer; retain `PutImage` as a fallback.
- [x] Measure CPU, RAM, VRAM, and submission delays/rates for both renderers.
- [x] Reuse CPU frame allocations while retaining initially transparent frames.
- [x] Compare three dimensions and three requested frame rates in optimized builds.
- [x] Expose and measure server presentation feedback, its clock, and hardware flags,
  separately from submissions.
- [x] Measure virtual input → drawing → presentation in a private session.
- [ ] Measure physical frame rate and device-to-light latency optically; the
  measurement equipment is unavailable for this campaign.
- [x] Compare MIT-SHM and PutImage, with and without Present, at three virtual dimensions.
- [x] Evaluate partial updates for counters, moving widgets, and dense scenes;
  provide an optional CPU mode and a reproducible 54-case comparison.

## Possible extensions

- [x] Add an optional egui adapter on the existing wgpu surface.
- [x] Provide a C ABI, a C++ RAII wrapper, and a dashboard example whose calculations
  remain in C++ while its interface is built in Rust.
- [x] Expose a generic C/C++ drawing and widget API without application-specific
  Rust code; integrate window tracking and Hyprland shortcuts.
- [ ] Extend global shortcuts to other desktops (portal and/or X11).

- [x] Expose mouse scroll axes with explicit units and sign.
- [x] Add XKB translation, local composition, and Wayland/X11 IME input.
- [x] Define touch and custom region contracts in [INPUT_DESIGN.md](docs/INPUT_DESIGN.md).
- [x] Implement custom regions and touch, with isolated protocol validation.
- [x] Test IBus/Anthy through XIM and Fcitx5/Hangul through XIM and text-input-v3,
  including intermediate commits and preedit styles.
- [ ] Assess physical touch panels for a future version. Touch is outside 0.2 support.
- [x] Add a text HUD example with fontdue, an application-supplied font, and scaling.

## Release preparation

- [x] Document public contracts, breaking changes, and validation limits.
- [x] Add a changelog and a release preparation procedure.
- [x] Verify final package contents, compilation, and documentation links.
- [x] Define the Linux support scope and the boundaries of the hardware results.
- [x] Exercise an external Rust application with the unpacked Cargo package and
  a C++17 application with the full source archive and its public CMake target.
- [ ] Tagging or publication is a separate maintainer decision.

Compatibility remains determined by capabilities negotiated with the graphics
server and driver. Display above every fullscreen application or above a lock
screen is outside the current guarantees.

The procedures and inventory in [HARDWARE_VALIDATION.md](docs/HARDWARE_VALIDATION.md)
detail the remaining physical tests. Intel/AMD GPUs, a hardware Xorg session, a
touch panel, and optical instrumentation are unavailable in this campaign's
environment; simulations do not replace them.
