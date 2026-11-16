#!/usr/bin/env python3
"""Inventory display/touch hardware and optionally observe physical connector changes."""

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path
import time


def read(path):
    try:
        return path.read_text().strip()
    except (FileNotFoundError, PermissionError, OSError):
        return None


def connectors():
    return {
        path.parent.name: read(path)
        for path in sorted(Path("/sys/class/drm").glob("card*-*/status"))
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--watch-seconds", type=int, default=0)
    args = parser.parse_args()
    if not 0 <= args.watch_seconds <= 3600:
        parser.error("watch-seconds must be 0..3600")
    displays = []
    for device in sorted(Path("/sys/bus/pci/devices").glob("*")):
        kind = read(device / "class")
        if kind and int(kind, 16) >> 16 == 3:
            displays.append(
                {
                    "pci": device.name,
                    "vendor": read(device / "vendor"),
                    "device": read(device / "device"),
                    "class": kind,
                    "driver": (device / "driver").resolve().name
                    if (device / "driver").exists()
                    else None,
                }
            )
    touches = []
    for device in sorted(Path("/sys/class/input").glob("event*/device")):
        absolute = read(device / "capabilities/abs")
        # Linux x86_64 bitmap text lists the lowest 64-bit word last. MT position
        # X/Y are bits 53/54 and are not advertised by a mouse or joystick alone.
        low = int(absolute.split()[-1], 16) if absolute else 0
        if low & (1 << 53) and low & (1 << 54):
            touches.append({"event": device.parent.name, "name": read(device / "name")})
    initial = connectors()
    changes = []
    start = time.monotonic()
    previous = initial
    while time.monotonic() - start < args.watch_seconds:
        current = connectors()
        if current != previous:
            changes.append(
                {"elapsed_s": round(time.monotonic() - start, 3), "connectors": current}
            )
            previous = current
        time.sleep(0.1)
    result = {
        "observed_utc": datetime.now(timezone.utc).isoformat(),
        "pci_displays": displays,
        "multitouch_devices": touches,
        "initial_connectors": initial,
        "observed_connector_changes": changes,
        "watch_seconds": args.watch_seconds,
        "validation_note": "Inventory and connector observation do not establish rendering, hotplug recovery or optical latency.",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
