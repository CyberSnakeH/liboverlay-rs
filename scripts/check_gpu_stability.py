#!/usr/bin/env python3
"""Run a transparent hardware GPU soak on an owned headless Vulkan Sway session."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary", type=Path, default=Path("target/debug/examples/gpu_stability")
    )
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--seconds", type=int, default=7200)
    parser.add_argument("--width", type=int, default=1920)
    parser.add_argument("--height", type=int, default=1080)
    parser.add_argument("--render-node", default="/dev/dri/renderD128")
    parser.add_argument(
        "--unsupported-gpu",
        action="store_true",
        help="pass Sway's explicit opt-in for proprietary GPU drivers",
    )
    args = parser.parse_args()
    sway = os.environ.get("SWAY") or shutil.which("sway")
    if not sway or not args.binary.is_file():
        parser.error("Sway and a compiled gpu_stability example are required")
    if not 1 <= args.seconds <= 86400 or not all(
        16 <= n <= 16384 for n in (args.width, args.height)
    ):
        parser.error("seconds must be 1..86400 and dimensions must be 16..16384")
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve()
    manifest = {
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "source_sha256": {
            str(p): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(Path("src").rglob("*.rs"))
        },
        "build_input_sha256": {
            str(p): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in (
                Path("Cargo.toml"),
                Path("Cargo.lock"),
                Path("examples/gpu_stability.rs"),
            )
        },
        "requested_seconds": args.seconds,
        "logical_size": [args.width, args.height],
        "render_node": args.render_node,
    }
    with tempfile.TemporaryDirectory(prefix="liboverlay-gpu-") as directory:
        root = Path(directory)
        root.chmod(0o700)
        config = root / "config"
        config.write_text(
            f"xwayland disable\noutput * resolution {args.width}x{args.height} scale 1\n"
            "seat seat0 fallback true\n"
        )
        environment = os.environ.copy()
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SWAYSOCK"):
            environment.pop(name, None)
        environment.update(
            XDG_RUNTIME_DIR=directory,
            WLR_BACKENDS="headless",
            WLR_RENDERER="vulkan",
            WLR_HEADLESS_OUTPUTS="1",
            WLR_RENDER_DRM_DEVICE=args.render_node,
        )
        command = [sway, "-c", str(config)]
        if args.unsupported_gpu:
            command.append("--unsupported-gpu")
        with (output / "sway.log").open("w+") as log:
            compositor = subprocess.Popen(
                command, env=environment, stdout=log, stderr=subprocess.STDOUT
            )
            try:
                deadline = time.monotonic() + 15
                while True:
                    displays = [p for p in root.glob("wayland-*") if p.is_socket()]
                    if displays:
                        break
                    if compositor.poll() is not None or time.monotonic() >= deadline:
                        log.seek(0)
                        raise RuntimeError(
                            f"private Vulkan Sway did not start: {log.read()}"
                        )
                    time.sleep(0.02)
                environment["WAYLAND_DISPLAY"] = str(displays[0])
                (output / "display.json").write_text(
                    json.dumps(
                        {
                            name: environment[name]
                            for name in ("XDG_RUNTIME_DIR", "WAYLAND_DISPLAY")
                        },
                        indent=2,
                    )
                    + "\n"
                )
                (output / "manifest.json").write_text(
                    json.dumps(manifest, indent=2) + "\n"
                )
                measure = str(Path(__file__).with_name("measure_process.py"))
                with (output / "soak.log").open("w") as results:
                    result = subprocess.run(
                        [
                            sys.executable,
                            measure,
                            "--output",
                            str(output / "process.tsv"),
                            "--",
                            str(binary),
                            "soak",
                            str(args.seconds),
                            str(output / "soak.tsv"),
                            str(args.width),
                            str(args.height),
                        ],
                        env=environment,
                        stdout=results,
                        stderr=subprocess.STDOUT,
                        timeout=args.seconds + 60,
                    )
                manifest.update(
                    finished_utc=datetime.now(timezone.utc).isoformat(),
                    exit_code=result.returncode,
                )
                (output / "manifest.json").write_text(
                    json.dumps(manifest, indent=2) + "\n"
                )
                if result.returncode:
                    raise RuntimeError(f"GPU soak failed; see {output / 'soak.log'}")
            finally:
                if compositor.poll() is None:
                    compositor.terminate()
                try:
                    compositor.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    compositor.kill()
                    compositor.wait()


if __name__ == "__main__":
    main()
