#!/usr/bin/env python3
"""Compare full uploads with automatic CPU damage on owned graphical servers."""

import argparse
import contextlib
import os
from pathlib import Path
import subprocess
import shutil
import tempfile
import sys

from cargo_fixture import build_test
from check_wayland import server as wayland_server
from check_x11 import server as x11_server
from x11_proxy import mask_extensions
import hashlib
import json
from datetime import datetime, timezone


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument(
        "--binary", type=Path, default=Path("target/release/examples/benchmark")
    )
    parser.add_argument("--seconds", type=int, default=10)
    parser.add_argument("--fps", type=int, default=60)
    parser.add_argument("--compositor", choices=("pixman", "vulkan"), default="pixman")
    parser.add_argument("--unsupported-gpu", action="store_true")
    args = parser.parse_args()
    if not 3 <= args.seconds <= 3600 or not 1 <= args.fps <= 1000:
        parser.error("seconds must be 3..3600 and fps 1..1000")
    if not args.binary.is_file():
        parser.error("compile the benchmark example first")
    args.output_dir.mkdir(parents=True, exist_ok=False)
    recorded_binary = args.output_dir / "benchmark"
    shutil.copy2(args.binary, recorded_binary)
    manifest = {
        "started_utc": datetime.now(timezone.utc).isoformat(),
        "seconds_per_case": args.seconds,
        "fps": args.fps,
        "compositor": args.compositor,
        "binary_sha256": hashlib.sha256(recorded_binary.read_bytes()).hexdigest(),
        "binary_path": "benchmark",
        "source_sha256": {
            str(p): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(Path("src").rglob("*.rs"))
        },
        "build_input_sha256": {
            str(p): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in (
                Path("Cargo.toml"),
                Path("Cargo.lock"),
                Path("examples/benchmark.rs"),
                Path("README.md"),
            )
        },
    }
    manifest["rustc"] = subprocess.check_output(
        ["rustc", "--version"], text=True
    ).strip()
    manifest["runner_sha256"] = {
        str(p): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in (
            Path(__file__),
            Path(__file__).with_name("measure_process.py"),
            Path(__file__).with_name("check_wayland.py"),
            Path(__file__).with_name("check_x11.py"),
            Path(__file__).with_name("x11_proxy.py"),
            Path(__file__).with_name("unix_transport.py"),
        )
    }
    (args.output_dir / "manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n"
    )
    measure = str(Path(__file__).with_name("measure_process.py"))
    protocol = build_test("x11_protocol", "x11")
    picom = os.environ.get("PICOM") or shutil.which("picom")
    if not picom:
        parser.error("picom is required for the X11 comparison")
    for backend, transport in (("wayland", "shm"), ("x11", "shm"), ("x11", "putimage")):
        display = (
            wayland_server(
                size="3840x2160",
                renderer=args.compositor,
                unsupported_gpu=args.unsupported_gpu,
            )
            if backend == "wayland"
            else x11_server([], size="3840x2160")
        )
        with display as resource, contextlib.ExitStack() as cleanup:
            if backend == "x11":
                log = cleanup.enter_context(tempfile.TemporaryFile())
                compositor = subprocess.Popen(
                    [picom, "--backend", "xrender", "--config", "/dev/null"],
                    env=resource[1],
                    stdout=log,
                    stderr=subprocess.STDOUT,
                )

                def stop():
                    if compositor.poll() is None:
                        compositor.terminate()
                    try:
                        compositor.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        compositor.kill()
                        compositor.wait()

                cleanup.callback(stop)
                subprocess.run(
                    [
                        protocol,
                        "alpha_input_region_focus_and_cleanup",
                        "--exact",
                        "--ignored",
                        "--nocapture",
                    ],
                    env=resource[1],
                    check=True,
                    timeout=15,
                )
            environment = {**resource[1], "LIBOVERLAY_TEST_PRIVATE_BENCHMARK": "1"}
            if backend == "x11":
                environment["DISPLAY"] = cleanup.enter_context(
                    mask_extensions(
                        resource[0].args[1][1:],
                        ["MIT-SHM"] if transport == "putimage" else [],
                    )
                )
            for size in ("640x360", "1920x1080", "3840x2160"):
                for scene in ("counter", "moving", "full"):
                    for mode in ("full", "partial"):
                        name = f"{backend}-{transport}-{size}-{scene}-{mode}"
                        print(name, flush=True)
                        command = [
                            sys.executable,
                            measure,
                            "--interval",
                            "0.5",
                            "--output",
                            str(args.output_dir / f"{name}-process.tsv"),
                            "--",
                            str(recorded_binary.resolve()),
                            "--backend",
                            backend,
                            "--renderer",
                            "cpu",
                            "--size",
                            size,
                            "--fps",
                            str(args.fps),
                            "--seconds",
                            str(args.seconds),
                            "--scene",
                            scene,
                            "--damage",
                            mode,
                            "--output",
                            str(args.output_dir / f"{name}-frames.tsv"),
                        ]
                        if backend == "wayland":
                            command += ["--monitor", "HEADLESS-1"]
                        with (args.output_dir / f"{name}.log").open("w") as log:
                            subprocess.run(
                                command,
                                env=environment,
                                stdout=log,
                                stderr=subprocess.STDOUT,
                                check=True,
                                timeout=args.seconds + 30,
                            )
                        if (
                            "presentation_pauses=0"
                            not in (args.output_dir / f"{name}.log").read_text()
                        ):
                            raise RuntimeError(
                                f"{name}: presentation paused; do not compare this sample"
                            )

    manifest.update(
        finished_utc=datetime.now(timezone.utc).isoformat(), exit_code=0, cases=54
    )
    (args.output_dir / "manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n"
    )


if __name__ == "__main__":
    main()
