#!/usr/bin/env python3
"""Check bounded failure paths without using the user's real display server.

Run `cargo build --example diagnose`, then `python3 scripts/check_failure_paths.py`.
Only the Python standard library is needed. A private temporary Unix socket is
used to simulate an unresponsive Wayland server; no system settings are changed.
"""
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time

PROGRAM = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).resolve().parents[1] / "target/debug/examples/diagnose"


def check_case(name, environment, arguments, expected):
    started = time.monotonic()
    result = subprocess.run([str(PROGRAM), *arguments], env=environment,
                            capture_output=True, text=True, timeout=7)
    output = result.stdout + result.stderr
    assert result.returncode == 1 and any(word in output for word in expected), (name, output)
    assert "panicked" not in output, (name, output)
    print(f"{name}: typed failure, exit=1, elapsed={time.monotonic() - started:.3f}s")


def main():
    environment = os.environ.copy()
    for key in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DISPLAY"):
        environment.pop(key, None)
    check_case("no display", environment, ["auto"], ("NoDisplayServer",))
    invalid = {**environment, "DISPLAY": ":65535"}
    check_case("invalid X11 display number", invalid, ["x11"], ("out of range",))
    with tempfile.TemporaryDirectory(prefix="liboverlay-timeout-") as directory:
        path = Path(directory) / "wayland.sock"
        stop = threading.Event()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
            listener.bind(str(path))
            path.chmod(0o600)
            listener.listen(1)
            listener.settimeout(7)

            def hold_connection():
                peer, _ = listener.accept()
                with peer:
                    stop.wait(7)

            worker = threading.Thread(target=hold_connection, daemon=True)
            worker.start()
            try:
                check_case("unresponsive Wayland peer", {**environment, "WAYLAND_DISPLAY": str(path)},
                           ["wayland"], ("Timeout", "timed out"))
            finally:
                stop.set()
                worker.join(timeout=1)


if __name__ == "__main__":
    main()
