#!/usr/bin/env python3
"""Exercise C++/Rust rendering, actions and lifecycle on a private headless Sway."""

import argparse
import contextlib
import os
import select
import subprocess
import time
from pathlib import Path

from check_wayland import ipc, server


def stop(child):
    if child.poll() is None:
        child.terminate()
    try:
        child.wait(timeout=5)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait()


def read_ack(child, expected):
    if not select.select([child.stdout], [], [], 5)[0]:
        raise RuntimeError("virtual input fixture timed out")
    if child.stdout.readline().strip() != expected:
        raise RuntimeError("virtual input fixture failed")


def wait_ready(child, path):
    deadline = time.monotonic() + 10
    while "overlay ready" not in path.read_text():
        if child.poll() is not None or time.monotonic() > deadline:
            raise RuntimeError(f"dashboard did not become ready: {path.read_text()}")
        time.sleep(0.02)
    time.sleep(0.2)  # let the compositor present the first submitted frame


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, default=Path("target/cpp"))
    parser.add_argument(
        "--output-dir", type=Path, default=Path("target/validation/cpp")
    )
    parser.add_argument(
        "--x11", action="store_true", help="also exercise private Xvfb/picom"
    )
    args = parser.parse_args()
    binary = (args.build_dir / "overlay_monitor").resolve()
    if not binary.is_file():
        parser.error("build the C++ example with CMake first")
    subprocess.run(
        ["cargo", "build", "--locked", "--example", "egui_input"], check=True
    )
    helper = Path("target/debug/examples/egui_input").resolve()
    args.output_dir.mkdir(parents=True, exist_ok=True)
    with (
        server(
            renderer=os.environ.get("LIBOVERLAY_TEST_COMPOSITOR", "vulkan"),
            unsupported_gpu=os.environ.get("LIBOVERLAY_TEST_UNSUPPORTED_GPU") == "1",
        ) as (_, env, control),
        contextlib.ExitStack() as cleanup,
    ):
        ipc(control, "output HEADLESS-2 disable")
        env["LIBOVERLAY_TEST_PRIVATE_INPUT"] = "1"
        pointer = subprocess.Popen(
            [str(helper)],
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        cleanup.callback(stop, pointer)
        read_ack(pointer, "ready")

        def command(value):
            pointer.stdin.write(value + "\n")
            pointer.stdin.flush()
            read_ack(pointer, "done")

        def click(x, y):
            command(f"move {x} {y}")
            time.sleep(0.05)
            command("down")
            time.sleep(0.1)
            command("up")
            time.sleep(0.1)

        log_path = args.output_dir / "wayland.log"
        with log_path.open("w+") as log:
            child = subprocess.Popen(
                [
                    str(binary),
                    "--backend",
                    "wayland",
                    "--seconds",
                    "15",
                    "--simulate",
                    "--allow-software",
                ],
                env=env,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
            cleanup.callback(stop, child)
            wait_ready(child, log_path)
            subprocess.run(
                [
                    "grim",
                    "-o",
                    "HEADLESS-1",
                    str((args.output_dir / "dashboard.png").resolve()),
                ],
                env=env,
                check=True,
                timeout=5,
            )
            click(125, 209)  # checkbox: Rust action must change C++ state
            click(155, 230)  # slider
            click(150, 251)  # close button: teardown initiated by Rust
            code = child.wait(timeout=5)
            log.seek(0)
            text = log.read()
            print(text, end="")
            if (
                code
                or "clean shutdown" not in text
                or "alerts=0" not in text
                or text.count("settings:") < 2
                or "close requested" not in text
            ):
                raise RuntimeError(
                    f"dashboard rendering/lifecycle failed (exit={code})"
                )
        result = subprocess.run(
            [str((args.build_dir / "overlay_ffi_runtime").resolve())],
            check=False,
            env=env,
            capture_output=True,
            text=True,
            timeout=15,
        )
        (args.output_dir / "ffi-runtime.log").write_text(result.stdout + result.stderr)
        print(result.stdout, end="")
        if result.returncode:
            raise RuntimeError(f"live ABI checks failed: {result.stderr}")
        scale_log = args.output_dir / "fractional-scale.log"
        with scale_log.open("w") as log:
            scaled = subprocess.Popen(
                [
                    str(binary),
                    "--backend",
                    "wayland",
                    "--seconds",
                    "3",
                    "--simulate",
                    "--allow-software",
                ],
                env=env,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
            cleanup.callback(stop, scaled)
            wait_ready(scaled, scale_log)
            ipc(control, "output HEADLESS-1 scale 1.25")
            time.sleep(0.3)
            ipc(control, "output HEADLESS-1 resolution 800x600")
            code = scaled.wait(timeout=10)
        if code or "clean shutdown" not in scale_log.read_text():
            raise RuntimeError(f"fractional scale failed: {scale_log.read_text()}")
    if args.x11:
        import shutil

        from check_x11 import server as x11_server

        picom = os.environ.get("PICOM") or shutil.which("picom")
        if not picom:
            parser.error("--x11 requires picom (or PICOM=path)")
        with x11_server([]) as (_, env), contextlib.ExitStack() as cleanup:
            icd = Path(
                os.environ.get(
                    "LAVAPIPE_ICD", "/usr/share/vulkan/icd.d/lvp_icd.x86_64.json"
                )
            )
            if not icd.is_file():
                parser.error("--x11 requires lavapipe (or LAVAPIPE_ICD=path)")
            env["VK_DRIVER_FILES"] = str(icd)
            env["VK_ICD_FILENAMES"] = str(icd)
            with (args.output_dir / "picom.log").open("w") as log:
                compositor = subprocess.Popen(
                    [picom, "--config", "/dev/null", "--backend", "xrender"],
                    env=env,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                )
                cleanup.callback(stop, compositor)
                time.sleep(0.5)
                if compositor.poll() is not None:
                    raise RuntimeError("private picom did not start")
                result = subprocess.run(
                    [
                        str(binary),
                        "--backend",
                        "x11",
                        "--seconds",
                        "2",
                        "--simulate",
                        "--allow-software",
                    ],
                    check=False,
                    env=env,
                    capture_output=True,
                    text=True,
                    timeout=15,
                )
                (args.output_dir / "x11.log").write_text(result.stdout + result.stderr)
                if result.returncode or "clean shutdown" not in result.stdout:
                    raise RuntimeError(f"X11 rendering failed: {result.stderr}")
                print("X11 egui rendering and cleanup passed")


if __name__ == "__main__":
    main()
