#!/usr/bin/env python3
"""Capture only the caller's small test fixture using KWin's normal DBus API.

Requires distro Python with PyGObject and Pillow. Does not alter permissions,
start Spectacle, capture the full desktop, or fall back after an access denial.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import time

from gi.repository import Gio, GLib
from PIL import Image


def capture(args):
    if not (0 < args.width <= 500 and 0 < args.height <= 400):
        raise ValueError("capture must be limited to a small owned fixture")
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    read_fd, write_fd = os.pipe()
    try:
        descriptors = Gio.UnixFDList.new()
        index = descriptors.append(write_fd)
        options = {key: GLib.Variant("b", value) for key, value in {
            "include-cursor": False,
            "include-shadow": False,
            "include-decoration": False,
            "native-resolution": False,
            "hide-caller-windows": False,
        }.items()}
        result, _ = bus.call_with_unix_fd_list_sync(
            "org.kde.KWin.ScreenShot2", "/org/kde/KWin/ScreenShot2",
            "org.kde.KWin.ScreenShot2", "CaptureArea",
            GLib.Variant("(iiuua{sv}h)", (args.x, args.y, args.width, args.height, options, index)),
            GLib.VariantType.new("(a{sv})"), Gio.DBusCallFlags.NONE,
            10000, descriptors, None)
        metadata = result.unpack()[0]
        expected = metadata["stride"] * metadata["height"]
        if expected > 16 * 1024 * 1024:
            raise ValueError("unexpected capture size")
        data = bytearray()
        deadline = time.monotonic() + 10
        while len(data) < expected:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([read_fd], [], [], remaining)[0]:
                raise TimeoutError("KWin screenshot pipe timed out")
            chunk = os.read(read_fd, min(65536, expected - len(data)))
            if not chunk:
                raise EOFError("KWin screenshot pipe closed early")
            data.extend(chunk)
        fmt = metadata["format"]
        if fmt in (5, 6):  # QImage ARGB32 / ARGB32 premultiplied, native LE
            raw_mode, mode = "BGRA", "RGBA"
        elif fmt == 4:  # QImage RGB32; format 3 is palette-based Indexed8
            raw_mode, mode = "BGRX", "RGB"
        elif fmt in (16, 17, 18):  # RGBX8888 / RGBA8888 / RGBA8888 premultiplied
            raw_mode, mode = ("RGBX", "RGB") if fmt == 16 else ("RGBA", "RGBA")
        else:
            raise ValueError(f"unsupported QImage format: {fmt}")
        image = Image.frombytes(mode, (metadata["width"], metadata["height"]), bytes(data),
                                "raw", raw_mode, metadata["stride"]).convert("RGB")
        destination = Path(args.output)
        destination.parent.mkdir(parents=True, exist_ok=True)
        image.save(destination)
        report = {"api": "org.kde.KWin.ScreenShot2.CaptureArea",
                  "requested_rectangle": [args.x, args.y, args.width, args.height],
                  "metadata": metadata,
                  "png_sha256": hashlib.sha256(destination.read_bytes()).hexdigest()}
        if args.reference:
            reference = Image.open(args.reference).convert("RGB")
            assert reference.size == image.size, "capture geometry changed"
            sx, sy = image.width / 320, image.height / 240
            sample = lambda source, x, y: source.getpixel((round(x * sx), round(y * sy)))
            margin_before, margin_after = sample(reference, 10, 120), sample(image, 10, 120)
            background = sample(reference, 70, 120)
            opaque = sample(image, 160, 120)
            alpha = sample(image, 70, 120)
            assert max(abs(a-b) for a,b in zip(margin_before, margin_after)) <= 2, (
                "fully transparent margin changed its background", margin_before, margin_after)
            assert max(abs(a-b) for a,b in zip(opaque, (220,40,80))) <= 8, (
                "opaque reference rectangle did not appear", opaque)
            # This checks composition without assuming whether KWin blends in
            # linear light or encoded RGB. The green source must mix with the
            # controlled background rather than becoming opaque or disappearing.
            assert background[1] + 10 < alpha[1] < 210, ("alpha green channel was not blended", background, alpha)
            assert alpha != background and alpha != (40,220,100), ("semi-transparent patch not blended", alpha)
            report["samples"] = {"margin_before": margin_before, "margin_after": margin_after,
                                 "background": background, "opaque": opaque, "half_alpha": alpha}
            report["composition_assertions"] = "passed"
        destination.with_suffix(".json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report))
    finally:
        os.close(read_fd)
        os.close(write_fd)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("x", "y", "width", "height"):
        parser.add_argument(name, type=int)
    parser.add_argument("output")
    parser.add_argument("--reference")
    arguments = parser.parse_args()
    try:
        capture(arguments)
    except Exception as error:
        print(json.dumps({"capture": "failed", "error": str(error)}))
        raise SystemExit(2)
