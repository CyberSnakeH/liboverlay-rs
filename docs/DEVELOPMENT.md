# Development

## Environment

The project targets Linux with Rust 1.85 or newer. Validation scripts use Python
3.10 or newer. CPU backends use Rust transports and do not require X11 or Wayland
headers. The `gpu` feature requires the Vulkan loader and, depending on the
backend, dynamically loaded Wayland or XCB system libraries. CI installs
`libwayland-dev` and `pkg-config` to check this profile.

The repository retains `Cargo.lock` to reproduce validation. All eight
combinations of `wayland`, `x11`, and `gpu`, including no features, can be built.
A configuration without a backend can check the API and its errors but cannot
create a window.

## Checks

```sh
sh scripts/check.sh
python3 scripts/check_x11.py --composited --wm
python3 scripts/check_wayland.py --gpu
python3 scripts/check_desktop_ime.py --framework ibus
python3 scripts/check_desktop_ime.py --framework fcitx5
python3 scripts/check_desktop_ime.py --framework fcitx5 --server-preedit
python3 scripts/check_desktop_ime.py --framework fcitx5 --backend wayland
python3 scripts/check_fullscreen.py
```

The first script checks formatting, tests, examples, documentation, Clippy, error
paths, and the Cargo package. The X11 and Wayland scripts respectively require
Xvfb/picom/Openbox with `setxkbmap`, and Sway; `--gpu` also requires a lavapipe ICD.
`XVFB`, `PICOM`, `OPENBOX`, `SWAY`, and `LAVAPIPE_ICD` select specific executables
or an ICD file.

Desktop scenarios require IBus/Anthy, Fcitx5/Hangul, `gdbus`, `dbus-run-session`,
Python GObject, and GTK 4 for the fullscreen background. They create a temporary
bus and profiles; they do not replace the user session's input daemons or
settings. `FCITX5` and `FCITX5_REMOTE` select executables. An extracted tree can
provide `FCITX_ADDON_DIRS`, `FCITX_DATA_DIRS`, and `LD_LIBRARY_PATH` without a
system installation.

Test servers use high private X11 display numbers or a temporary Wayland
directory. They do not reuse the current session's compositor. Graphical tests
remain ignored by default in Cargo. The scripts execute automated scenarios on
their private displays, including input injection. Manual observations are run
individually, as described in [TESTING.md](TESTING.md).

For the minimum compiler:

```sh
rustup toolchain install 1.85.0 --profile minimal --component rustfmt,clippy
RUSTUP_TOOLCHAIN=1.85.0 sh scripts/check.sh
```

The [CI configuration](../.github/workflows/ci.yml) runs these checks and builds
static musl CPU examples. A local run does not constitute a remote workflow run.
The desktop input job uses Ubuntu 22.04 because Ubuntu 24.04 does not package
IBus/Anthy; other jobs use Ubuntu 24.04.

## Contracts to preserve

- Native connections and objects belong to the display thread.
- Text editing remains the application's responsibility; preedit and commits
  must not be confused or applied twice.
- Any loss of input transitions is reported through an explicitly scoped reset.
- Queues, retained allocations, and busy buffers remain bounded.
- A frame belongs to one window and one geometry generation.
- Shared bytes are rewritten only after release by the server.
- Strict transparency, renderer, and stacking requirements fail explicitly when
  they cannot be met.
- A GPU surface is released before its native window and connection.
- Shutdown and input commands remain available without frame callbacks.
- A validation report specifies its hardware, environment, and the limits of what
  its measurements establish.

`unsafe` blocks explain the lifetime, ownership, and synchronization that make
the operation valid. Public item documentation and these comments are checked by
the manifest's lints.

## Preparing a release

1. Run the checks and update [TESTING.md](TESTING.md),
   [PERFORMANCE.md](PERFORMANCE.md), and the [changelog](../CHANGELOG.md).
2. Review public changes and their migration instructions.
3. Build the package and inspect included paths:

   ```sh
   cargo package --locked --allow-dirty --all-features
   cargo package --locked --allow-dirty --list
   ```

4. Repeat validation on the revision intended for distribution, then create the
   corresponding tag and publish using the project owner's account.

A verified package build creates neither a tag nor a crates.io publication.
Validation requiring other GPUs or a hardware Xorg session remains identified
in the roadmap.
