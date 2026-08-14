import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk

# Real widgets, no main loop: constructing/inspecting Gtk4 widgets and
# emitting their signals synchronously works fine against Gtk.init() alone
# (matching the ticket 09/12 prototypes' own "real widgets" testing style),
# so these tests exercise the actual widget tree rather than mocks.
Gtk.init()
