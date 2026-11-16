#!/usr/bin/env python3
"""Exercise an actual desktop IME on a private D-Bus session and Xvfb display."""

import argparse
import contextlib
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time

from cargo_fixture import build_test
from check_wayland import run_fixture, server as wayland_server
from check_x11 import server
from x11_test_input import selection_owner


@contextlib.contextmanager
def input_method(environment, framework, directory, backend, server_preedit):
    environment.update(
        XDG_CONFIG_HOME=str(directory / "config"),
        XDG_CACHE_HOME=str(directory / "cache"),
        XDG_DATA_HOME=str(directory / "data"),
        XDG_RUNTIME_DIR=str(directory),
        GSETTINGS_BACKEND="memory",
        GIO_USE_VFS="local",
    )
    if framework == "ibus":
        environment.update(
            # Share isolated settings with the separately spawned Anthy engine.
            # The memory backend is process-local, and Ubuntu defaults to Latin.
            GSETTINGS_BACKEND="keyfile",
            IBUS_ADDRESS=f"unix:path={directory}/ibus",
            XMODIFIERS="@im=ibus",
            LIBOVERLAY_TEST_IME_KEYS="57,31,43,32,57",
            LIBOVERLAY_TEST_IME_TEXT="にほん",
        )
        subprocess.run(
            [
                "gsettings",
                "set",
                "org.freedesktop.ibus.engine.anthy.common",
                "input-mode",
                "0",  # Hiragana
            ],
            env=environment,
            check=True,
        )
        command = [
            "ibus-daemon",
            "--single",
            "--xim",
            "--panel",
            "disable",
            "--config",
            "disable",
            "--emoji-extension",
            "disable",
            "--cache",
            "none",
            "--address",
            environment["IBUS_ADDRESS"],
        ]
        configure = [
            "gdbus",
            "call",
            "--address",
            environment["IBUS_ADDRESS"],
            "--dest",
            "org.freedesktop.IBus",
            "--object-path",
            "/org/freedesktop/IBus",
            "--method",
            "org.freedesktop.IBus.SetGlobalEngine",
            "anthy",
        ]
    else:
        environment.update(
            XMODIFIERS="@im=fcitx",
            LIBOVERLAY_TEST_IME_KEYS="42,45,39,27,58,41",
            LIBOVERLAY_TEST_IME_TEXT="한글",
        )
        profile = directory / "config" / "fcitx5" / "profile"
        profile.parent.mkdir(parents=True)
        profile.write_text(
            "[Groups/0]\nName=Default\nDefault Layout=us\nDefaultIM=hangul\n"
            "[Groups/0/Items/0]\nName=keyboard-us\nLayout=\n"
            "[Groups/0/Items/1]\nName=hangul\nLayout=\n[GroupOrder]\n0=Default\n"
        )
        (profile.parent / "conf").mkdir()
        (profile.parent / "conf" / "xim.conf").write_text(
            "UseOnTheSpot=" + ("False" if server_preedit else "True") + "\n"
        )
        environment["LIBOVERLAY_TEST_SERVER_PREEDIT"] = "1" if server_preedit else "0"
        (profile.parent / "config").write_text(
            "[Behavior]\nPreeditEnabledByDefault=True\n"
        )
        command = [
            os.environ.get("FCITX5", "fcitx5"),
            "--disable=all",
            "--enable="
            + ("xcb,xim" if backend == "x11" else "wayland,waylandim")
            + ",dbus,keyboard,hangul",
            "--ui=none",
        ]
        configure = [os.environ.get("FCITX5_REMOTE", "fcitx5-remote"), "-s", "hangul"]
    activation = [
        key
        for key in (
            "IBUS_ADDRESS",
            "DISPLAY",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_RUNTIME_DIR",
            "GSETTINGS_BACKEND",
            "GIO_USE_VFS",
        )
        if key in environment
    ]
    subprocess.run(
        ["dbus-update-activation-environment", *activation], env=environment, check=True
    )
    with (directory / "daemon.log").open("w+") as log:
        process = subprocess.Popen(
            command,
            env=environment,
            stdout=log,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            deadline = time.monotonic() + 15
            while True:
                if framework == "fcitx5":
                    ready = subprocess.run(
                        [
                            "gdbus",
                            "call",
                            "--session",
                            "--dest",
                            "org.freedesktop.DBus",
                            "--object-path",
                            "/org/freedesktop/DBus",
                            "--method",
                            "org.freedesktop.DBus.GetNameOwner",
                            "org.fcitx.Fcitx5",
                        ],
                        env=environment,
                        stdout=subprocess.PIPE,
                        stderr=subprocess.PIPE,
                    )
                    if ready.returncode:
                        if process.poll() is not None or time.monotonic() >= deadline:
                            raise RuntimeError(
                                "Fcitx5 did not acquire its private bus name"
                            )
                        time.sleep(0.05)
                        continue
                result = subprocess.run(
                    configure,
                    env=environment,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                )
                if result.returncode == 0:
                    # The XIM child can publish its selection after the daemon
                    # already answers D-Bus calls. Start the overlay only once
                    # its actual X11 input-method endpoint exists.
                    name = "@server=ibus" if framework == "ibus" else "@server=fcitx"
                    if backend != "x11" or selection_owner(environment, name):
                        break
                if process.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError(
                        f"{framework} did not become ready: {result.stderr.decode()}"
                    )
                time.sleep(0.05)
            yield configure
        except Exception:
            log.seek(0)
            print(log.read(), flush=True)
            raise
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--framework", choices=("ibus", "fcitx5"), required=True)
    parser.add_argument("--backend", choices=("x11", "wayland"), default="x11")
    parser.add_argument(
        "--server-preedit",
        action="store_true",
        help="exercise Fcitx XIM server-rendered preedit",
    )
    parser.add_argument("--session", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.server_preedit and (args.backend != "x11" or args.framework != "fcitx5"):
        parser.error("server-preedit is specific to the Fcitx XIM scenario")
    if args.backend == "wayland" and args.framework == "ibus":
        parser.error(
            "this runner requires an input-method-v2 server on Wayland; use Fcitx5 or test IBus through XIM"
        )
    if not args.session:
        subprocess.run(
            [
                "dbus-run-session",
                "--",
                sys.executable,
                __file__,
                "--framework",
                args.framework,
                "--backend",
                args.backend,
                "--session",
                *(["--server-preedit"] if args.server_preedit else []),
            ],
            check=True,
        )
        return
    test = build_test(
        "x11_input" if args.backend == "x11" else "wayland_input", args.backend
    )
    display = server([]) if args.backend == "x11" else wayland_server()
    with (
        display as resource,
        tempfile.TemporaryDirectory(prefix="liboverlay-ime-") as name,
    ):
        environment = resource[1]
        environment["LIBOVERLAY_TEST_PRIVATE_INPUT"] = "1"
        directory = Path(name)
        with input_method(
            environment, args.framework, directory, args.backend, args.server_preedit
        ) as configure:

            def action(stage):
                if stage != "desktop-ime":
                    raise RuntimeError(f"unexpected IME fixture stage: {stage}")
                subprocess.run(configure, env=environment, check=True)
                return "configured"

            run_fixture(
                test, "desktop_input_method_composes_and_commits", environment, action
            )


if __name__ == "__main__":
    main()
