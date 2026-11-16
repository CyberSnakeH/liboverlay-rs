#!/usr/bin/env python3
"""Compare CPU uploads and pacing on a private, composited 4K Xvfb display."""

import argparse
from pathlib import Path
import os
import shutil
import subprocess
import sys
import tempfile

from check_x11 import server
from cargo_fixture import build_test
from x11_proxy import mask_extensions


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary", type=Path, default=Path("target/release/examples/benchmark")
    )
    parser.add_argument("--output-dir", required=True, type=Path)
    parser.add_argument("--seconds", type=int, default=10)
    parser.add_argument("--fps", type=int, default=60)
    args = parser.parse_args()
    if not args.binary.is_file():
        parser.error("build the benchmark example first, or pass --binary")
    if not 1 <= args.seconds <= 86400 or not 1 <= args.fps <= 1000:
        parser.error("seconds must be 1..86400 and fps must be 1..1000")
    picom = os.environ.get("PICOM") or shutil.which("picom")
    if not picom:
        parser.error("picom is required (or set PICOM)")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    binary = str(args.binary.resolve())
    measure = str(Path(__file__).with_name("measure_process.py"))
    protocol_test = build_test("x11_protocol", "x11")
    for disabled in ([], ["Present"], ["MIT-SHM"], ["Present", "MIT-SHM"]):
        transport = "putimage" if "MIT-SHM" in disabled else "shm"
        pacing = "timer" if "Present" in disabled else "present"
        with (
            server([], size="3840x2160") as (xserver, environment),
            mask_extensions(xserver.args[1][1:], disabled) as proxy,
            tempfile.TemporaryFile() as errors,
        ):
            client_env = {**environment, "DISPLAY": proxy}
            compositor = subprocess.Popen(
                [picom, "--backend", "xrender", "--config", "/dev/null"],
                env=environment,
                stdout=errors,
                stderr=subprocess.STDOUT,
            )
            try:
                # This fixture waits for compositor readiness and checks its
                # selection before any measured process starts.
                subprocess.run(
                    [
                        protocol_test,
                        "alpha_input_region_focus_and_cleanup",
                        "--exact",
                        "--ignored",
                        "--nocapture",
                    ],
                    env=client_env,
                    check=True,
                    timeout=15,
                )
                for size in ("640x360", "1920x1080", "3840x2160"):
                    name = f"{transport}-{pacing}-{size}"
                    print(name, flush=True)
                    command = [
                        sys.executable,
                        measure,
                        "--interval",
                        "1",
                        "--output",
                        str(args.output_dir / f"{name}-process.tsv"),
                        "--",
                        binary,
                        "--backend",
                        "x11",
                        "--renderer",
                        "cpu",
                        "--size",
                        size,
                        "--fps",
                        str(args.fps),
                        "--seconds",
                        str(args.seconds),
                        "--output",
                        str(args.output_dir / f"{name}-frames.tsv"),
                    ]
                    with (args.output_dir / f"{name}.log").open("w") as log:
                        subprocess.run(
                            command,
                            env=client_env,
                            stdout=log,
                            stderr=subprocess.STDOUT,
                            check=True,
                            timeout=args.seconds + 30,
                        )
                    if compositor.poll() is not None:
                        raise RuntimeError("private picom stopped during the benchmark")
            except Exception:
                errors.seek(0)
                print(errors.read().decode(errors="replace"), file=sys.stderr)
                raise
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
