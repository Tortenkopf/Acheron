# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

import pytest

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk

# Real widgets, no main loop: constructing/inspecting Gtk4 widgets and
# emitting their signals synchronously works fine against Gtk.init() alone
# (matching the ticket 09/12 prototypes' own "real widgets" testing style),
# so these tests exercise the actual widget tree rather than mocks.
Gtk.init()

from .widget_tree import toplevels  # noqa: E402


@pytest.fixture(autouse=True)
def _tray_icon_dir(tmp_path, monkeypatch):
    """Ticket 97: `TrayIcon` syncs the bundled status-dot SVGs to
    `$ACHERON_TRAY_ICON_DIR` (default `~/.local/share/acheron/tray-icons`)
    on construction. Redirect that to a per-test tmp dir so the suite never
    writes into the real user data dir."""
    monkeypatch.setenv("ACHERON_TRAY_ICON_DIR", str(tmp_path / "tray-icons"))


@pytest.fixture(autouse=True)
def _no_real_window_presentation(monkeypatch):
    """Tests reach an editor window by clicking its button, whose handler
    calls `Gtk.Window.present()`. With a compositor around, that would map
    a real (modal) window on the developer's desktop for every click, and
    without one its behaviour isn't something to depend on. Tests only ever
    read the window's content through `open_editor_window`, so presenting is
    a no-op here."""
    monkeypatch.setattr(Gtk.Window, "present", lambda self: None)


@pytest.fixture(autouse=True)
def _destroy_leftover_toplevels():
    """GTK keeps every top-level window in one process-wide list until it's
    destroyed, so a window one test leaves open would still be counted by
    the next test's `Gtk.Window.get_toplevels()` assertions. Destroy
    whatever is left once each test is done."""
    yield
    for window in toplevels():
        window.destroy()
