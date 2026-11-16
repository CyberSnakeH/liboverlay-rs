#!/usr/bin/env python3
"""Own a simple GTK window only inside the validation runner's private display."""

import os
import gi

gi.require_version("Gtk", "4.0")
from gi.repository import GLib, Gtk

if os.environ.get("LIBOVERLAY_TEST_PRIVATE_FULLSCREEN") != "1":
    raise SystemExit("private fullscreen runner required")
Gtk.init()
window = Gtk.Window(title="liboverlay-fullscreen-background")
window.set_default_size(320, 240)
window.set_child(Gtk.Label(label="Private fullscreen validation"))
window.present()
loop = GLib.MainLoop()
window.connect("close-request", lambda _: loop.quit())
loop.run()
