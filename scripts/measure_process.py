#!/usr/bin/env python3
"""Measure an owned command's CPU, RSS and optional NVIDIA graphics VRAM.

CPU use is relative to one logical core. Missing NVIDIA measurements are empty
cells, never zero. Other applications' process rows are discarded immediately.
"""

import argparse
import csv
import os
from pathlib import Path
import shutil
import subprocess
import time


def process_sample(pid):
    root = Path(f"/proc/{pid}")
    status = {}
    for line in (root / "status").read_text().splitlines():
        name, _, value = line.partition(":")
        status[name] = value.strip().split()[0] if value.strip() else ""
    # comm may contain spaces and parentheses. The final ')' ends that field.
    fields = (root / "stat").read_text().rpartition(")")[2].split()
    ticks = int(fields[11]) + int(fields[12])
    return (
        ticks / os.sysconf("SC_CLK_TCK"),
        status.get("VmRSS", ""),
        len(list((root / "fd").iterdir())),
        status["Threads"],
    )


def graphics_vram(pid, nvidia_smi):
    if not nvidia_smi:
        return ""
    try:
        result = subprocess.run(
            [nvidia_smi, "pmon", "-s", "m", "-c", "1"],
            capture_output=True,
            text=True,
            timeout=4,
            check=True,
        )
    except (OSError, subprocess.SubprocessError):
        return ""
    samples = []
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) >= 4 and fields[0].isdigit() and fields[1] == str(pid):
            # pmon -s m: gpu, pid, type, framebuffer MiB, protected MiB, command.
            if not fields[3].isdigit():
                return ""
            samples.append(int(fields[3]))
    return sum(samples) if samples else ""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--interval", type=float, default=10)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command or not 0.1 <= args.interval <= 3600:
        parser.error("provide a command and an interval in 0.1..=3600 seconds")
    with args.output.open("w", newline="") as output:
        writer = csv.writer(output, delimiter="\t")
        writer.writerow(
            [
                "elapsed_s",
                "pid",
                "cpu_s",
                "cpu_percent",
                "rss_kib",
                "fds",
                "threads",
                "nvidia_fb_mib",
            ]
        )
        child = subprocess.Popen(command)
        start = time.monotonic()
        previous = None
        try:
            while child.poll() is None:
                now = time.monotonic()
                try:
                    cpu, rss, fds, threads = process_sample(child.pid)
                except (FileNotFoundError, ProcessLookupError):
                    break
                percent = (
                    ""
                    if previous is None
                    else round(100 * (cpu - previous[1]) / (now - previous[0]), 3)
                )
                vram = graphics_vram(child.pid, shutil.which("nvidia-smi"))
                writer.writerow(
                    [
                        round(now - start, 3),
                        child.pid,
                        cpu,
                        percent,
                        rss,
                        fds,
                        threads,
                        vram,
                    ]
                )
                output.flush()
                previous = now, cpu
                deadline = now + args.interval
                while child.poll() is None and time.monotonic() < deadline:
                    time.sleep(min(0.1, max(0, deadline - time.monotonic())))
            raise SystemExit(child.wait())
        finally:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()


if __name__ == "__main__":
    main()
