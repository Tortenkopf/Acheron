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


@pytest.fixture(autouse=True)
def _tray_icon_dir(tmp_path, monkeypatch):
    """Ticket 97: `TrayIcon` syncs the bundled status-dot SVGs to
    `$ACHERON_TRAY_ICON_DIR` (default `~/.local/share/acheron/tray-icons`)
    on construction. Redirect that to a per-test tmp dir so the suite never
    writes into the real user data dir."""
    monkeypatch.setenv("ACHERON_TRAY_ICON_DIR", str(tmp_path / "tray-icons"))
