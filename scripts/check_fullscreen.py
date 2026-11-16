#!/usr/bin/env python3
"""Exercise real GTK fullscreen transitions beneath an owned layer-shell overlay."""

import os
import signal
from pathlib import Path
import subprocess
import sys
import time
from cargo_fixture import build_test
from check_wayland import server, ipc, run_fixture
from unix_transport import read_exact
import json
import socket
import struct


def tree(control):
    with socket.socket(socket.AF_UNIX) as peer:
        peer.settimeout(5)
        peer.connect(str(control))
        peer.sendall(b"i3-ipc" + struct.pack("<II", 0, 4))
        header = read_exact(peer, 14)
        length, _ = struct.unpack_from("<II", header, 6)
        return json.loads(read_exact(peer, length))


def background(node):
    if node.get("name") == "liboverlay-fullscreen-background":
        return node
    for child in node.get("nodes", []) + node.get("floating_nodes", []):
        found = background(child)
        if found:
            return found
    return None


def main():
    test = build_test("wayland_input", "wayland")
    with server(
        renderer=os.environ.get("LIBOVERLAY_TEST_COMPOSITOR", "pixman"),
        unsupported_gpu=os.environ.get("LIBOVERLAY_TEST_UNSUPPORTED_GPU") == "1",
    ) as (_, environment, control):
        env = {
            **environment,
            "LIBOVERLAY_TEST_PRIVATE_FULLSCREEN": "1",
            "LIBOVERLAY_TEST_PRIVATE_INPUT": "1",
            "GSETTINGS_BACKEND": "memory",
            "GIO_USE_VFS": "local",
            "GTK_A11Y": "none",
            "XDG_CURRENT_DESKTOP": "sway",
            "GSK_RENDERER": os.environ.get("GSK_RENDERER", "cairo"),
        }
        env.pop("HYPRLAND_INSTANCE_SIGNATURE", None)
        with subprocess.Popen(
            [
                "dbus-run-session",
                "--",
                sys.executable,
                str(Path(__file__).with_name("fullscreen_background.py")),
            ],
            env=env,
            start_new_session=True,
        ) as client:
            try:
                deadline = time.monotonic() + 15
                while not (window := background(tree(control))):
                    if client.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError("private GTK background did not map")
                    time.sleep(0.02)
                identifier = window["id"]

                def action(stage):
                    index = int(stage.removeprefix("fullscreen-"))
                    enabled = index % 2 == 0
                    ipc(
                        control,
                        f"[con_id={identifier}] fullscreen {'enable' if enabled else 'disable'}",
                    )
                    node = background(tree(control))
                    if not node or bool(node["fullscreen_mode"]) != enabled:
                        raise RuntimeError(
                            "owned background did not change fullscreen state"
                        )
                    return "changed"

                run_fixture(
                    test,
                    "fullscreen_transitions_keep_layer_input_and_focus",
                    env,
                    action,
                )
            finally:
                if client.poll() is None:
                    os.killpg(client.pid, signal.SIGTERM)
                try:
                    client.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(client.pid, signal.SIGKILL)
                    client.wait()


if __name__ == "__main__":
    main()
