"""Read back public Rust and C++ drawings on private compositor outputs."""

import io
import os
import subprocess
import time
from functools import partial

from check_cpp import stop
from check_wayland import server as wayland_server
from check_x11 import server as x11_server
from PIL import Image, ImageChops, ImageGrab
from vulkan_fixture import lavapipe_icd
from x11_test_input import wait_for_selection


def solid(image, box, color, name, tolerance=2):
    region = image.crop(box)
    difference = ImageChops.difference(region, Image.new("RGB", region.size, color))
    if any(high > tolerance for _, high in difference.getextrema()):
        raise AssertionError(
            f"{name} at {box}: expected {color}, got ranges {region.getextrema()}"
        )


def check_scene(image, scale):
    def box(x1, y1, x2, y2):
        return tuple(round(v * scale) for v in (x1, y1, x2, y2))

    if image.size != box(0, 0, 640, 480)[2:]:
        raise AssertionError(f"Unexpected output dimensions {image.size}")

    for name, bounds, color in (
        ("line", (12, 11, 44, 13), (255, 0, 0)),
        ("above line", (12, 6, 44, 8), (0, 0, 0)),
        ("circle", (68, 16, 76, 24), (0, 255, 0)),
        ("circle corner", (60, 8, 63, 11), (0, 0, 0)),
        ("triangle", (108, 22, 116, 29), (0, 0, 255)),
        ("triangle corner", (96, 9, 100, 16), (0, 0, 0)),
        ("clipped fill", (17, 57, 47, 79), (255, 255, 0)),
        ("red only", (84, 60, 100, 92), (128, 0, 0)),
        ("blue over red", (108, 72, 124, 92), (64, 0, 128)),
        ("blue only", (132, 72, 148, 104), (0, 0, 128)),
    ):
        solid(image, box(*bounds), color, name)

    # Check all four sides throughout the area of the unclipped rectangle.
    for outside in (
        (4, 44, 60, 56),
        (4, 80, 60, 92),
        (4, 56, 16, 80),
        (48, 56, 60, 80),
    ):
        solid(image, box(*outside), (0, 0, 0), "outside clip")

    text = (
        image.crop(box(144, 8, 230, 38))
        .convert("L")
        .point(lambda v: 255 if v > 100 else 0)
    )
    ink = text.getbbox()
    if ink is None or ink[2] - ink[0] < 35 * scale or ink[3] - ink[1] < 8 * scale:
        raise AssertionError("Missing text at its expected position")
    # Glyph strokes leave background between and inside the letters.
    coverage = text.histogram()[255] / ((ink[2] - ink[0]) * (ink[3] - ink[1]))
    if not 0.12 < coverage < 0.8:
        raise AssertionError(f"Unexpected text coverage: {coverage:.3f}")


def observe(command, env, capture, verify):
    with subprocess.Popen(command, env=env, stdin=subprocess.PIPE) as app:
        try:
            deadline = time.monotonic() + 15
            while True:
                if app.poll() is not None:
                    raise RuntimeError(
                        f"Client exited before readback: {app.returncode}"
                    )
                try:
                    verify(capture())
                    break
                except AssertionError:
                    if time.monotonic() >= deadline:
                        raise
                time.sleep(0.08)
            app.communicate(input=b"\n", timeout=5)
            if app.returncode:
                raise RuntimeError(f"Client failed to close: {app.returncode}")
        finally:
            stop(app)


def wayland_image(env, output, scale):
    shot = subprocess.run(
        ["grim", "-s", str(scale), "-t", "ppm", "-o", output, "-"],
        env=env,
        check=True,
        stdout=subprocess.PIPE,
        timeout=5,
    )
    with Image.open(io.BytesIO(shot.stdout)) as image:
        return image.convert("RGB")


def check_frames(rust_client, cpp_client):
    with wayland_server(renderer="pixman") as (_, env, _):
        observe(
            [rust_client, "--draw"],
            env,
            lambda: wayland_image(env, "HEADLESS-1", 1),
            lambda image: solid(
                image, (276, 212, 364, 268), (20, 220, 100), "Rust CPU frame"
            ),
        )
        print("Rust package: CPU pixels and explicit close verified", flush=True)
        if cpp_client is None:
            return
        driver = str(lavapipe_icd())
        env["VK_DRIVER_FILES"] = env["VK_ICD_FILENAMES"] = driver
        for scale, output in ((1, "HEADLESS-1"), (1.25, "HEADLESS-2")):
            observe(
                [cpp_client, "wayland", str(scale)],
                env,
                partial(wayland_image, env, output, scale),
                partial(check_scene, scale=scale),
            )
            print(f"C++ Wayland output scale {scale}: scene pixels passed", flush=True)

    for scale, size in ((1, "640x480"), (1.25, "800x600")):
        with x11_server([], size=size) as (_, env):
            env["VK_DRIVER_FILES"] = env["VK_ICD_FILENAMES"] = driver
            with subprocess.Popen(
                [
                    os.environ.get("PICOM", "picom"),
                    "--config",
                    "/dev/null",
                    "--backend",
                    "xrender",
                ],
                env=env,
            ) as compositor:
                try:
                    wait_for_selection(env, "_NET_WM_CM_S0", compositor)
                    observe(
                        [cpp_client, "x11", str(scale)],
                        env,
                        lambda: ImageGrab.grab(xdisplay=env["DISPLAY"]).convert("RGB"),
                        partial(check_scene, scale=scale),
                    )
                finally:
                    stop(compositor)
            print(f"C++ X11 application scale {scale}: scene pixels passed", flush=True)
