# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Screenshot harness for the Library view (ticket 91, reusable for 92-95).

Drives the real `AcheronApplication` against a `DaemonStub`, clicks through
to each Library tab, and screenshots the running window **from inside its
own process** — the toplevel's GSK renderer renders the widget tree to a
`Gdk.Texture`, saved as PNG. No external screenshot tool, portal permission,
or compositor cooperation is involved, so it works headlessly under the
session's own Wayland/X display.

Usage:  python3 gui/tools/shot_library.py OUTDIR
Writes  OUTDIR/{grid,macros,macros_delay,steppers}.png
"""

from __future__ import annotations

import sys
from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import GLib, Gtk, Graphene  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import acheron_gui.app as _app_mod  # noqa: E402
from acheron_gui.app import AcheronApplication  # noqa: E402
from acheron_gui.daemon_stub import DaemonStub  # noqa: E402


class _NoTray:
    """The real `TrayIcon` needs an `org.kde.StatusNotifierWatcher` on the
    session bus, which a headless screenshot run has no reason to provide —
    stub it out so the window still builds."""

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


def _find_button(root, label):
    for w in _walk(root):
        if isinstance(w, Gtk.Button) and w.get_label() == label:
            return w
    return None


def _shot(win, path):
    native = win.get_native()
    renderer = native.get_renderer()
    w = win.get_width()
    h = win.get_height()
    paintable = Gtk.WidgetPaintable.new(win)
    snapshot = Gtk.Snapshot()
    paintable.snapshot(snapshot, w, h)
    node = snapshot.to_node()
    if node is None:
        print("  !! snapshot node was None")
        return
    texture = renderer.render_texture(node, Graphene.Rect().init(0, 0, w, h))
    texture.save_to_png(path)
    print(f"  saved {path} ({w}x{h})")


def main() -> None:
    outdir = Path(sys.argv[1])
    outdir.mkdir(parents=True, exist_ok=True)
    which = sys.argv[2] if len(sys.argv) > 2 else "both"

    stub = DaemonStub()
    stub.create_macro(
        "Reload + Melee",
        [
            {"type": "key_down", "key": "KEY_R"},
            {"type": "delay_ms", "ms": 40},
            {"type": "key_up", "key": "KEY_R"},
            {"type": "key_down", "key": "KEY_V"},
            {"type": "key_up", "key": "KEY_V"},
        ],
    )
    sid = stub.create_stepper(
        "Weapon Wheel",
        [
            {"type": "key", "key": "KEY_1", "modifiers": []},
            {"type": "key", "key": "KEY_2", "modifiers": []},
            {"type": "key", "key": "KEY_3", "modifiers": ["ctrl"]},
        ],
    )
    stub.set_binding(
        "grid_r1c1", "base",
        {"trigger": "fire_once", "type": "step", "stepper_id": sid, "direction": "forward"},
    )

    app = AcheronApplication(client=stub, systemd_client=_FakeSystemd())

    def click(label):
        win = app.get_active_window()
        btn = _find_button(win, label)
        assert btn is not None, f"button {label!r} not found"
        btn.emit("clicked")

    def shot(name):
        _shot(app.get_active_window(), str(outdir / f"{name}.png"))

    def select_delay():
        win = app.get_active_window()
        for w in _walk(win):
            if isinstance(w, Gtk.DropDown) and w.get_model() is not None:
                m = w.get_model()
                if m.get_n_items() == 3 and m.get_string(2) == "Delay (ms)":
                    w.set_selected(2)
                    return

    steps: list = [
        lambda: shot("grid"),
        lambda: click("Library"),
        lambda: click("Macros"),
        lambda: shot("macros"),
        lambda: select_delay(),
        lambda: shot("macros_delay"),
        lambda: click("Steppers"),
        lambda: shot("steppers"),
        # Ticket 92/93: the keyboard↔controller switcher on both editors.
        lambda: click("Controller"),
        lambda: shot("steppers_controller"),
        lambda: click("Macros"),
        lambda: shot("macros_controller"),
        lambda: app.quit(),
    ]

    def pump():
        if not steps:
            return False
        steps.pop(0)()
        GLib.timeout_add(450, pump)
        return False

    def on_activate(_a):
        GLib.timeout_add(700, pump)

    app.connect("activate", on_activate)
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
