# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Screenshot harness for the Device Overview grid (ticket 96).

Sibling of `shot_library.py` / `shot_binding_editor.py` — drives the real
`AcheronApplication` against a `DaemonStub`, seeds a grid key that is *both*
a Chord member *and* individually bound (ticket 96's tooltip case), then
screenshots the running window and dumps every grid button's
`get_tooltip_text()` so the combined-tooltip fix is checkable without a
live hover.

The real GUI holds the `com.acheron.gui` application id on the session bus,
so this harness forces a NON_UNIQUE private id (ticket 94's note for 95+).

Usage:  python3 gui/tools/shot_device_overview.py OUTDIR
Writes  OUTDIR/device_overview.png plus OUTDIR/tooltips.txt
"""

from __future__ import annotations

import sys
from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gio, GLib, Gtk, Graphene  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import acheron_gui.app as _app_mod  # noqa: E402
from acheron_gui.app import AcheronApplication  # noqa: E402
from acheron_gui.daemon_stub import DaemonStub  # noqa: E402


class _NoTray:
    def __init__(self, *_a, **_kw) -> None:
        pass

    def update(self, *_a, **_kw) -> None:
        pass


_app_mod.TrayIcon = _NoTray


class _FakeSystemd:
    def ensure_daemon_started(self) -> None:
        pass

    def stop_daemon(self) -> None:
        pass

    def start_daemon(self) -> None:
        pass


def _walk(widget):
    yield widget
    child = widget.get_first_child()
    while child is not None:
        yield from _walk(child)
        child = child.get_next_sibling()


def _grid_buttons(root):
    return [
        w
        for w in _walk(root)
        if isinstance(w, Gtk.Button) and getattr(w, "binding_editor_window", None) is not None
    ]


def _shot(win, path):
    native = win.get_native()
    renderer = native.get_renderer()
    w, h = win.get_width(), win.get_height()
    paintable = Gtk.WidgetPaintable.new(win)
    snapshot = Gtk.Snapshot()
    paintable.snapshot(snapshot, w, h)
    node = snapshot.to_node()
    if node is None:
        print("  !! snapshot node was None")
        return
    texture = renderer.render_texture(node, Graphene.Rect().init(0, 0, w, h))
    texture.save_to_png(str(path))
    print(f"  saved {path} ({w}x{h})")


def main() -> None:
    outdir = Path(sys.argv[1])
    outdir.mkdir(parents=True, exist_ok=True)

    stub = DaemonStub()
    # grid_r1c1 (label "1"): a member of the {1,2} Chord AND its own long
    # modifier-combination Keypress — ticket 96's exact "readable nowhere"
    # case before the fix.
    stub.set_chord_binding(
        ["grid_r1c1", "grid_r1c2"],
        "base",
        {"trigger": "fire_once", "type": "keypress", "key": "KEY_C"},
    )
    stub.set_binding(
        "grid_r1c1",
        "base",
        {
            "trigger": "fire_once",
            "type": "keypress",
            "key": "KEY_F9",
            "modifiers": ["ctrl", "shift", "alt"],
        },
    )
    # grid_r1c3 (label "3"): an individual-only binding, no Chord.
    stub.set_binding(
        "grid_r1c3",
        "base",
        {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_K", "modifiers": ["super"]},
    )

    app = AcheronApplication(client=stub, systemd_client=_FakeSystemd())
    app.set_application_id("com.acheron.gui.shot96")
    app.set_flags(Gio.ApplicationFlags.NON_UNIQUE)

    def run():
        win = app.get_active_window()
        _shot(win, outdir / "device_overview.png")
        with open(outdir / "tooltips.txt", "w") as fh:
            for btn in sorted(
                _grid_buttons(win), key=lambda b: b.binding_editor_window.get_title()
            ):
                title = btn.binding_editor_window.get_title()
                tip = btn.get_tooltip_text()
                fh.write(f"{title}\n    tooltip: {tip!r}\n")
                print(f"{title} -> {tip!r}")
        app.quit()

    app.connect("activate", lambda _a: GLib.timeout_add(900, run))
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
