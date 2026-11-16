#!/usr/bin/env python3
"""Run CPU Wayland tests on owned headless Sway instances and filtered sockets."""

import contextlib
import argparse
import json
import os
from pathlib import Path
import selectors
import shutil
import socket
import struct
import subprocess
import tempfile
import time
from unix_transport import read_exact
from wayland_proxy import mask_globals
from wayland_input import TouchProbe
from cargo_fixture import build_test
from vulkan_fixture import lavapipe_icd


def ipc(path, command):
    payload = command.encode()
    with socket.socket(socket.AF_UNIX) as peer:
        peer.settimeout(5)
        peer.connect(str(path))
        peer.sendall(b"i3-ipc" + struct.pack("<II", len(payload), 0) + payload)
        header = read_exact(peer, 14)
        length, _ = struct.unpack_from("<II", header, 6)
        replies = json.loads(read_exact(peer, length))
        if not all(reply.get("success") for reply in replies):
            raise RuntimeError(f"Sway command failed: {command}: {replies}")


@contextlib.contextmanager
def server(*, size="640x480", renderer="pixman", unsupported_gpu=False):
    sway = os.environ.get("SWAY") or shutil.which("sway")
    if not sway:
        raise RuntimeError("Sway is required (or set SWAY to its executable)")
    with tempfile.TemporaryDirectory(prefix="liboverlay-sway-") as directory:
        root = Path(directory)
        root.chmod(0o700)
        config = root / "config"
        config.write_text(
            "xwayland disable\n"
            f"output HEADLESS-1 resolution {size} scale 1 position 0 0\n"
            "output HEADLESS-2 resolution 800x600 scale 1.25 position -640 0\n"
            "input * repeat_delay 40\ninput * repeat_rate 25\n"
            "seat seat0 fallback true\n"
        )
        environment = os.environ.copy()
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SWAYSOCK"):
            environment.pop(name, None)
        environment.update(
            XDG_RUNTIME_DIR=directory,
            WLR_BACKENDS="headless",
            WLR_RENDERER=renderer,
            WLR_HEADLESS_OUTPUTS="2",
        )
        with (root / "sway.log").open("w+") as log:
            process = subprocess.Popen(
                [sway, "-c", str(config)]
                + (["--unsupported-gpu"] if unsupported_gpu else []),
                env=environment,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
            try:
                deadline = time.monotonic() + 15
                while True:
                    displays = [p for p in root.glob("wayland-*") if p.is_socket()]
                    controls = list(root.glob("sway-ipc*.sock"))
                    if displays and controls:
                        break
                    if process.poll() is not None or time.monotonic() > deadline:
                        log.seek(0)
                        raise RuntimeError(f"Sway startup failed: {log.read()}")
                    time.sleep(0.02)
                environment["WAYLAND_DISPLAY"] = str(displays[0])
                yield process, environment, controls[0]
            except Exception:
                log.seek(0)
                print(log.read(), flush=True)
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def run_fixture(test, name, environment, action):
    child = subprocess.Popen(
        [test, name, "--exact", "--ignored", "--nocapture"],
        env=environment,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    pending = b""
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(child.stdout, selectors.EVENT_READ)
            while child.poll() is None:
                if not selector.select(15):
                    raise RuntimeError(f"{name}: fixture timed out")
                chunk = os.read(child.stdout.fileno(), 4096)
                if not chunk:
                    break
                print(chunk.decode(), end="", flush=True)
                pending += chunk
                while b"\n" in pending:
                    line, pending = pending.split(b"\n", 1)
                    if line.startswith(b"READY "):
                        reply = action(line[6:].decode())
                        child.stdin.write((reply + "\n").encode())
                        child.stdin.flush()
        if child.wait(timeout=5):
            raise RuntimeError(f"{name}: fixture failed")
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()


def geometry_change(control, stage):
    if stage == "remove-output":
        ipc(control, "output HEADLESS-2 disable")
        return "removed"
    index = int(stage.removeprefix("scale-"))
    scale, numerator = [(1.5, 180), (2, 240), (1.25, 150)][index]
    commands = [
        f"output HEADLESS-2 scale {value}" for value in (1, 2, 1.25, 1.5, scale)
    ]
    commands.append(
        f"output HEADLESS-2 resolution {1024 + index * 40}x768 position -800 0"
    )
    ipc(control, "; ".join(commands))
    return str(numerator)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--gpu",
        action="store_true",
        help="also exercise missing and software Vulkan drivers",
    )
    args = parser.parse_args()
    test = build_test("wayland_headless", "wayland,gpu" if args.gpu else "wayland")
    input_test = build_test("wayland_input", "wayland,gpu" if args.gpu else "wayland")
    with server() as (_, environment, control):
        for hidden in (
            [],
            ["wp_fractional_scale_manager_v1"],
            ["wp_viewporter"],
            ["wp_fractional_scale_manager_v1", "wp_viewporter"],
            ["wp_presentation"],
            ["zwlr_layer_shell_v1"],
        ):
            print(f"Wayland hidden={hidden}", flush=True)
            with mask_globals(environment["WAYLAND_DISPLAY"], hidden) as display:
                selected = {**environment, "WAYLAND_DISPLAY": display}
                if "zwlr_layer_shell_v1" in hidden:
                    selected["LIBOVERLAY_TEST_NO_LAYER_SHELL"] = "1"
                elif "wp_presentation" in hidden:
                    selected["LIBOVERLAY_TEST_NO_PRESENTATION"] = "1"
                elif hidden:
                    selected["LIBOVERLAY_TEST_NO_FRACTIONAL"] = "1"
                subprocess.run(
                    [
                        test,
                        "protocol_negotiation_and_frames",
                        "--exact",
                        "--ignored",
                        "--nocapture",
                    ],
                    env=selected,
                    check=True,
                    timeout=15,
                )
        with mask_globals(
            environment["WAYLAND_DISPLAY"], [], block_frames=True
        ) as display:
            subprocess.run(
                [
                    test,
                    "withheld_callbacks_do_not_block_input_or_close",
                    "--exact",
                    "--ignored",
                    "--nocapture",
                ],
                env={**environment, "WAYLAND_DISPLAY": display},
                check=True,
                timeout=15,
            )

        run_fixture(
            test,
            "geometry_and_output_removal",
            environment,
            lambda stage: geometry_change(control, stage),
        )
    with server() as (_, environment, _):
        probe = TouchProbe()
        with mask_globals(
            environment["WAYLAND_DISPLAY"], [], block_frames=True, input_probe=probe
        ) as display:
            run_fixture(
                test,
                "touch_contacts_seat_removal_and_stalled_input_regions",
                {**environment, "WAYLAND_DISPLAY": display},
                probe.action,
            )
    pixels = build_test("wayland_pixels", "wayland")
    with server() as (_, environment, control):

        def pixels_change(stage):
            command = {
                "pixels-3": "resolution 800x600",
                "pixels-5": "scale 2",
                "pixels-7": "resolution 640x480 scale 1",
            }[stage]
            ipc(control, "output HEADLESS-1 " + command)
            return "changed"

        run_fixture(
            pixels,
            "partial_updates_and_presentation_preserve_composed_pixels",
            {**environment, "LIBOVERLAY_TEST_PRIVATE_CAPTURE": "1"},
            pixels_change,
        )
    with server() as (_, environment, input_control):

        def input_change(stage):
            scale = {"input-scale": 2, "input-scale-reset": 1}.get(stage)
            if scale is None:
                raise RuntimeError(f"unexpected input fixture stage: {stage}")
            ipc(input_control, f"output HEADLESS-1 scale {scale}")
            return "changed"

        run_fixture(
            input_test,
            "keyboard_regions_and_input_method",
            {**environment, "LIBOVERLAY_TEST_PRIVATE_INPUT": "1"},
            input_change,
        )
    with server() as (_, environment, _):
        subprocess.run(
            [
                input_test,
                "virtual_input_to_presentation",
                "--exact",
                "--ignored",
                "--nocapture",
            ],
            env={**environment, "LIBOVERLAY_TEST_PRIVATE_INPUT": "1"},
            check=True,
            timeout=30,
        )
    if args.gpu:
        with server() as (_, environment, gpu_control):
            drivers = [("missing", "/nonexistent/liboverlay-vulkan-driver.json")]
            drivers.append(("software", str(lavapipe_icd())))
            for mode, driver in drivers:
                selected = {
                    **environment,
                    "VK_DRIVER_FILES": driver,
                    "VK_ICD_FILENAMES": driver,
                    "LIBOVERLAY_TEST_VULKAN": mode,
                }
                subprocess.run(
                    [
                        test,
                        "gpu_adapter_policies",
                        "--exact",
                        "--ignored",
                        "--nocapture",
                    ],
                    env=selected,
                    check=True,
                    timeout=30,
                )
                if mode == "software":
                    with mask_globals(
                        environment["WAYLAND_DISPLAY"], [], block_frames=True
                    ) as display:
                        subprocess.run(
                            [
                                test,
                                "device_loss_while_waiting_for_a_callback_is_terminal",
                                "--exact",
                                "--ignored",
                                "--nocapture",
                            ],
                            env={**selected, "WAYLAND_DISPLAY": display},
                            check=True,
                            timeout=15,
                        )
                    run_fixture(
                        test,
                        "geometry_and_output_removal",
                        {**selected, "LIBOVERLAY_TEST_GPU_GEOMETRY": "1"},
                        lambda stage: geometry_change(gpu_control, stage),
                    )
    with server() as (process, environment, _):

        def disconnect(stage):
            if stage != "disconnect":
                raise RuntimeError(f"unexpected fixture stage: {stage}")
            process.terminate()
            process.wait(timeout=5)
            return "disconnected"

        run_fixture(test, "server_disconnect_is_terminal", environment, disconnect)
    # Fresh creation after the stopped session also exercises compositor restart.
    with server() as (_, environment, _):
        subprocess.run(
            [
                test,
                "protocol_negotiation_and_frames",
                "--exact",
                "--ignored",
                "--nocapture",
            ],
            env=environment,
            check=True,
            timeout=15,
        )


if __name__ == "__main__":
    main()
