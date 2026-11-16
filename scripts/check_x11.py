#!/usr/bin/env python3
"""Exercise owned Xvfb servers with and without MIT-SHM and Present."""

import contextlib
import argparse
import os
import secrets
import shlex
import time
from pathlib import Path
import selectors
import shutil
import subprocess
import tempfile
from x11_proxy import mask_extensions
from cargo_fixture import build_test


@contextlib.contextmanager
def server(disabled, *, size="640x480"):
    xvfb = os.environ.get("XVFB") or shutil.which("Xvfb")
    if not xvfb:
        raise RuntimeError("Xvfb is required (or set XVFB to its executable)")
    for _ in range(100):
        display = str(secrets.randbelow(40_000) + 10_000)
        display_path = Path(f"/tmp/.X11-unix/X{display}")
        if not display_path.exists() and not Path(f"/tmp/.X{display}-lock").exists():
            break
    else:
        raise RuntimeError("could not reserve a private Xvfb display")
    args = [
        xvfb,
        f":{display}",
        "-screen",
        "0",
        f"{size}x24",
        "-nolisten",
        "tcp",
        "-ac",
        "-noreset",
    ]
    for extension in disabled:
        if extension == "Present":
            continue  # Present is built in and cannot be toggled by Xvfb.
        args += ["-extension", extension]
    with tempfile.TemporaryFile() as errors:
        process = subprocess.Popen(args, stderr=errors)
        try:
            deadline = time.monotonic() + 15
            while not display_path.is_socket():
                if process.poll() is not None or time.monotonic() > deadline:
                    errors.seek(0)
                    raise RuntimeError(f"Xvfb startup failed: {errors.read().decode()}")
                time.sleep(0.02)
            environment = os.environ.copy()
            for name in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET", "XAUTHORITY"):
                environment.pop(name, None)
            environment["DISPLAY"] = f":{display}"
            environment["LIBOVERLAY_TEST_PRIVATE_X11"] = "1"
            if "Present" in disabled:
                environment["LIBOVERLAY_TEST_NO_PRESENT"] = "1"
            if "MIT-SHM" in disabled:
                environment["LIBOVERLAY_TEST_NO_SHM"] = "1"
            if "Present" in disabled:
                with mask_extensions(display, ["Present"]) as proxy_display:
                    environment["DISPLAY"] = proxy_display
                    yield process, environment
            else:
                yield process, environment
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--composited",
        action="store_true",
        help="also exercise a private picom compositor",
    )
    parser.add_argument(
        "--wm",
        action="store_true",
        help="also exercise focus and fullscreen with private Openbox",
    )
    args = parser.parse_args()
    test = build_test("x11_headless", "x11")
    input_test = build_test("x11_input", "x11")
    for disabled in ([], ["Present"], ["MIT-SHM"], ["Present", "MIT-SHM"]):
        print(f"Xvfb disabled={disabled}", flush=True)
        with server(disabled) as (_, environment):
            subprocess.run(
                [
                    test,
                    "opaque_upload_resize_and_cleanup",
                    "--exact",
                    "--ignored",
                    "--nocapture",
                ],
                env=environment,
                check=True,
                timeout=30,
            )
            subprocess.run(
                [
                    test,
                    "partial_updates_preserve_unchanged_pixels_and_resize",
                    "--exact",
                    "--ignored",
                    "--nocapture",
                ],
                env=environment,
                check=True,
                timeout=30,
            )
            if not disabled:
                subprocess.run(
                    [
                        input_test,
                        "custom_regions_restore_and_follow_surface_extent",
                        "--exact",
                        "--ignored",
                        "--nocapture",
                    ],
                    env=environment,
                    check=True,
                    timeout=30,
                )
                subprocess.run(
                    [
                        test,
                        "monitor_relocation_and_removal",
                        "--exact",
                        "--ignored",
                        "--nocapture",
                    ],
                    env=environment,
                    check=True,
                    timeout=30,
                )
                subprocess.run(
                    [
                        test,
                        "scroll_axes_preserve_direction_and_units",
                        "--exact",
                        "--ignored",
                        "--nocapture",
                    ],
                    env=environment,
                    check=True,
                    timeout=15,
                )
    with server([]) as (xserver, environment):
        child = subprocess.Popen(
            [
                test,
                "server_disconnect_is_terminal",
                "--exact",
                "--ignored",
                "--nocapture",
            ],
            env=environment,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        try:
            with selectors.DefaultSelector() as selector:
                selector.register(child.stdout, selectors.EVENT_READ)
                while True:
                    if not selector.select(15):
                        raise RuntimeError("disconnection fixture timed out")
                    chunk = os.read(child.stdout.fileno(), 4096).decode()
                    print(chunk, end="", flush=True)
                    if not chunk:
                        raise RuntimeError("fixture stopped before READY")
                    if "READY disconnect" in chunk:
                        break
            xserver.terminate()
            xserver.wait(timeout=5)
            output, _ = child.communicate("disconnected\n", timeout=15)
            print(output, end="")
            if child.returncode:
                raise RuntimeError("disconnection fixture failed")
        finally:
            if child.poll() is None:
                child.kill()
                child.wait()
    if args.composited:
        picom = os.environ.get("PICOM") or shutil.which("picom")
        if not picom:
            raise RuntimeError("--composited needs picom (or set PICOM)")
        protocol_test = build_test("x11_protocol", "x11")
        with server([]) as (_, environment), tempfile.TemporaryFile() as log:
            environment["LIBOVERLAY_TEST_PRIVATE_COMPOSITOR"] = "1"
            compositor = subprocess.Popen(
                [picom, "--backend", "xrender", "--config", "/dev/null"],
                env=environment,
                stdout=log,
                stderr=subprocess.STDOUT,
            )
            try:
                for name in (
                    "alpha_input_region_focus_and_cleanup",
                    "composed_alpha_matches_premultiplied_pixels",
                ):
                    subprocess.run(
                        [protocol_test, name, "--exact", "--ignored", "--nocapture"],
                        env=environment,
                        check=True,
                        timeout=15,
                    )
            except Exception:
                log.seek(0)
                print(log.read().decode(errors="replace"))
                raise
            finally:
                if compositor.poll() is None:
                    compositor.terminate()
                try:
                    compositor.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    compositor.kill()
                    compositor.wait()
    if args.wm:
        openbox = os.environ.get("OPENBOX") or shutil.which("openbox")
        if not openbox:
            raise RuntimeError("--wm needs Openbox (or set OPENBOX)")
        protocol_test = build_test("x11_protocol", "x11")
        with server([]) as (_, environment), tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            ready = root / "ready"
            config = root / "rc.xml"
            config.write_text("""<openbox_config xmlns="http://openbox.org/3.4/rc">
  <focus><focusNew>yes</focusNew><followMouse>no</followMouse></focus>
  <theme><name>Clearlooks</name></theme>
  <mouse><context name="Client"><mousebind button="Left" action="Press">
    <action name="Focus"/><action name="Raise"/>
  </mousebind></context></mouse>
</openbox_config>""")
            environment = {
                **environment,
                "XDG_CONFIG_HOME": directory,
                "LIBOVERLAY_TEST_PRIVATE_WM": "1",
                "XMODIFIERS": "@im=liboverlay-fixture",
            }
            with tempfile.TemporaryFile() as log:
                manager = subprocess.Popen(
                    [
                        openbox,
                        "--sm-disable",
                        "--config-file",
                        str(config),
                        "--startup",
                        f"touch {shlex.quote(str(ready))}",
                    ],
                    env=environment,
                    stdout=log,
                    stderr=subprocess.STDOUT,
                )
                try:
                    deadline = time.monotonic() + 15
                    while not ready.exists():
                        if manager.poll() is not None or time.monotonic() >= deadline:
                            raise RuntimeError("private Openbox did not become ready")
                        time.sleep(0.02)
                    subprocess.run(
                        [
                            protocol_test,
                            "keyboard_focus_and_fullscreen_policy",
                            "--exact",
                            "--ignored",
                            "--nocapture",
                        ],
                        env=environment,
                        check=True,
                        timeout=30,
                    )
                    for name in (
                        "xkb_layout_changes_and_local_composition",
                        "xim_preedit_commit_forwarding_and_server_loss",
                    ):
                        subprocess.run(
                            [input_test, name, "--exact", "--ignored", "--nocapture"],
                            env=environment,
                            check=True,
                            timeout=30,
                        )
                except Exception:
                    log.seek(0)
                    print(log.read().decode(errors="replace"))
                    raise
                finally:
                    if manager.poll() is None:
                        manager.terminate()
                    try:
                        manager.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        manager.kill()
                        manager.wait()


if __name__ == "__main__":
    main()
