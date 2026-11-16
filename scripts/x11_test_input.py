"""Pointer input restricted to an owned private X11 test display."""

import ctypes
import time


def selection_owner(environment, name):
    """Observe readiness of an IM server on the owned X11 display."""
    if environment.get("LIBOVERLAY_TEST_PRIVATE_X11") != "1":
        raise RuntimeError("Selection queries are restricted to the private fixture")
    xlib = ctypes.CDLL("libX11.so.6")
    xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]
    xlib.XOpenDisplay.restype = ctypes.c_void_p
    xlib.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    xlib.XInternAtom.restype = ctypes.c_ulong
    xlib.XGetSelectionOwner.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
    xlib.XGetSelectionOwner.restype = ctypes.c_ulong
    xlib.XCloseDisplay.argtypes = [ctypes.c_void_p]
    display = xlib.XOpenDisplay(environment["DISPLAY"].encode())
    if not display:
        raise RuntimeError("Cannot open private X11 display")
    try:
        atom = xlib.XInternAtom(display, name.encode("ascii"), 1)
        return xlib.XGetSelectionOwner(display, atom) if atom else 0
    finally:
        xlib.XCloseDisplay(display)


def wait_for_selection(environment, name, process, timeout=10):
    """Wait for a live private server to acquire its advertised X11 endpoint."""
    deadline = time.monotonic() + timeout
    while True:
        code = process.poll()
        if code is not None:
            raise RuntimeError(f"private server exited before owning {name} (exit={code})")
        owner = selection_owner(environment, name)
        if owner:
            return owner
        if time.monotonic() >= deadline:
            raise RuntimeError(f"private server did not acquire {name} within {timeout}s")
        time.sleep(0.02)


def click(environment, x, y):
    """Send a bounded pointer click only to the explicitly supplied private display."""
    import time

    if environment.get("LIBOVERLAY_TEST_PRIVATE_X11") != "1":
        raise RuntimeError("Virtual input is restricted to the private fixture")
    xlib = ctypes.CDLL("libX11.so.6")
    xtest = ctypes.CDLL("libXtst.so.6")
    xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]
    xlib.XOpenDisplay.restype = ctypes.c_void_p
    xlib.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
    xlib.XCloseDisplay.argtypes = [ctypes.c_void_p]
    xtest.XTestFakeMotionEvent.argtypes = [
        ctypes.c_void_p,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_ulong,
    ]
    xtest.XTestFakeButtonEvent.argtypes = [
        ctypes.c_void_p,
        ctypes.c_uint,
        ctypes.c_int,
        ctypes.c_ulong,
    ]
    display = xlib.XOpenDisplay(environment["DISPLAY"].encode())
    if not display:
        raise RuntimeError("Cannot open private X11 display")
    try:
        if not xtest.XTestFakeMotionEvent(display, -1, x, y, 0):
            raise RuntimeError("Private pointer move failed")
        xlib.XSync(display, 0)
        time.sleep(0.08)
        for pressed in [1, 0]:
            if not xtest.XTestFakeButtonEvent(display, 1, pressed, 0):
                raise RuntimeError("Private click failed")
            xlib.XSync(display, 0)
            time.sleep(0.08)
    finally:
        xlib.XCloseDisplay(display)
