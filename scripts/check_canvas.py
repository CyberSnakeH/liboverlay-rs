#!/usr/bin/env python3
"""Exercise the generic C/C++ canvas API on owned Wayland/X11 displays."""

import argparse
import contextlib
import os
import subprocess
import time
from pathlib import Path

from check_cpp import read_ack, stop
from check_wayland import ipc
from check_wayland import server as wayland_server
from check_x11 import server as x11_server
from x11_test_input import click as x11_click
from vulkan_fixture import lavapipe_icd


def exercise(build, output, env, click, backend):
    result = subprocess.run(
        [str(build / "overlay_canvas_runtime")],
        env=env,
        check=False,
        capture_output=True,
        text=True,
        timeout=15,
    )
    (output / f"{backend}-abi.log").write_text(result.stdout + result.stderr)
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    log_path = output / f"{backend}-widgets.log"
    with log_path.open("w") as log:
        child = subprocess.Popen(
            [
                str(build / "overlay_canvas"),
                f"--{backend}",
                "--allow-software",
                "--seconds",
                "10",
            ],
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        try:
            deadline = time.monotonic() + 8
            while "canvas ready" not in log_path.read_text():
                if child.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError(log_path.read_text())
                time.sleep(0.05)
            time.sleep(0.3)
            if backend == "x11":
                try:
                    from PIL import ImageGrab
                except ImportError:
                    pass
                else:
                    ImageGrab.grab(xdisplay=env["DISPLAY"]).save(
                        output / "generic-canvas.png"
                    )
            click(110, 140)
            click(160, 160)
            click(110, 182)
            code = child.wait(timeout=4)
            text = log_path.read_text()
            if (
                code
                or "enabled=0" not in text
                or "opacity=" not in text
                or "canvas clean shutdown" not in text
            ):
                raise RuntimeError(f"{backend} generic widget actions failed: {text}")
            print(backend + ": " + text.strip())
        finally:
            stop(child)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build-dir", type=Path, default=Path("target/cpp"))
    parser.add_argument(
        "--output-dir", type=Path, default=Path("target/validation/canvas")
    )
    parser.add_argument("--backend", choices=["x11", "wayland", "both"], default="both")
    args = parser.parse_args()
    build = args.build_dir.resolve()
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if args.backend in ("x11", "both"):
        with x11_server([]) as (_, env), contextlib.ExitStack() as cleanup:
            env["VK_DRIVER_FILES"] = str(lavapipe_icd())
            env["VK_ICD_FILENAMES"] = env["VK_DRIVER_FILES"]
            log = cleanup.enter_context((output / "picom.log").open("w"))
            compositor = subprocess.Popen(
                [
                    os.environ.get("PICOM", "picom"),
                    "--config",
                    "/dev/null",
                    "--backend",
                    "xrender",
                ],
                env=env,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
            cleanup.callback(stop, compositor)
            time.sleep(0.4)
            if compositor.poll() is not None:
                raise RuntimeError("private picom failed")
            exercise(build, output, env, lambda x, y: x11_click(env, x, y), "x11")
    if args.backend in ("wayland", "both"):
        subprocess.run(
            ["cargo", "build", "--locked", "--example", "egui_input"], check=True
        )
        with (
            wayland_server(
                renderer=os.environ.get("LIBOVERLAY_TEST_COMPOSITOR", "vulkan"),
                unsupported_gpu=os.environ.get("LIBOVERLAY_TEST_UNSUPPORTED_GPU")
                == "1",
            ) as (_, env, control),
            contextlib.ExitStack() as cleanup,
        ):
            ipc(control, "output HEADLESS-2 disable")
            env["LIBOVERLAY_TEST_PRIVATE_INPUT"] = "1"
            pointer = subprocess.Popen(
                [str(Path("target/debug/examples/egui_input").resolve())],
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
                time.sleep(0.08)
                command("down")
                time.sleep(0.08)
                command("up")
                time.sleep(0.08)

            exercise(build, output, env, click, "wayland")


if __name__ == "__main__":
    main()
