"""Screenshot harness for the About dialog (ticket 102).

Sibling of `shot_binding_editor.py` / `shot_library.py` — builds the real
`about_dialog.build_about_dialog` against hand-made `GetState()` snapshots
(no Daemon, no device needed), screenshots each from inside its own process
(toplevel GSK renderer -> `Gdk.Texture` -> PNG), and does the same for the
"View Licence" window.

Usage:  python3 gui/tools/shot_about.py OUTDIR
Writes  OUTDIR/about_*.png plus OUTDIR/about_states.txt (the label text of
each rendered variant, for a quick diff-able record).
"""

from __future__ import annotations

import sys
from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gdk, Gio, GLib, Gtk, Graphene  # noqa: E402

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from acheron_gui import __version__  # noqa: E402
from acheron_gui.app import CSS  # noqa: E402
from acheron_gui.about_dialog import (  # noqa: E402
    _license_text,
    build_about_dialog,
    build_license_window,
)

CONNECTED_STATE = {
    "daemon_version": "1.0.0-dev+abc1234",
    "firmware_version": "v1.2",
    "serial_number": "PM2443F36300141",
}
DISCONNECTED_STATE = {"daemon_version": "1.0.0-dev+abc1234"}


def _walk(widget):
    yield widget
    child = widget.get_first_child()
    while child is not None:
        yield from _walk(child)
        child = child.get_next_sibling()


def _label_texts(root):
    return [w.get_label() for w in _walk(root) if isinstance(w, Gtk.Label) and w.get_label()]


def _shot(win, path):
    native = win.get_native()
    renderer = native.get_renderer()
    w, h = win.get_width(), win.get_height()
    paintable = Gtk.WidgetPaintable.new(win)
    snapshot = Gtk.Snapshot()
    paintable.snapshot(snapshot, w, h)
    node = snapshot.to_node()
    if node is None:
        print(f"  !! snapshot node was None for {path}")
        return
    texture = renderer.render_texture(node, Graphene.Rect().init(0, 0, w, h))
    texture.save_to_png(str(path))
    print(f"  saved {path.name} ({w}x{h})")


def main() -> None:
    outdir = Path(sys.argv[1])
    outdir.mkdir(parents=True, exist_ok=True)
    dump = open(outdir / "about_states.txt", "w")

    def log(*a):
        line = " ".join(str(x) for x in a)
        print(line)
        print(line, file=dump)

    app = Gtk.Application(application_id="com.acheron.gui.shot102")
    app.set_flags(Gio.ApplicationFlags.NON_UNIQUE)

    def on_activate(_a):
        provider = Gtk.CssProvider()
        provider.load_from_string(CSS)
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )
        log(f"acheron_gui __version__ = {__version__}")

        # A real mapped anchor toplevel so the harness has a parent to make
        # the (modal) dialogs transient-for, matching how the app opens them.
        anchor = Gtk.ApplicationWindow(application=app, title="shot_about anchor")
        anchor.set_default_size(400, 200)
        anchor.present()

        variants = [
            ("about_01_connected", lambda: build_about_dialog(anchor, state=CONNECTED_STATE)),
            ("about_02_disconnected", lambda: build_about_dialog(anchor, state=DISCONNECTED_STATE)),
            ("about_03_daemon_down", lambda: build_about_dialog(anchor, state=None)),
            ("about_04_licence", lambda: build_license_window(anchor, license_text=_license_text())),
        ]
        steps: list = []
        for name, make in variants:
            steps.append(("open", name, make))
            steps.append(("shot", name))
            steps.append(("close", name))
        steps.append(("done", None, None))

        held: dict = {}

        def pump():
            kind, *rest = steps.pop(0)
            if kind == "open":
                name, make = rest
                held["win"] = make()
                held["win"].present()
            elif kind == "shot":
                (name,) = rest
                win = held["win"]
                log(f"--- {name} ---")
                for text in _label_texts(win):
                    log("   ", repr(text))
                _shot(win, outdir / f"{name}.png")
            elif kind == "close":
                held["win"].close()
            else:
                dump.close()
                anchor.close()
                app.quit()
                return False
            GLib.timeout_add(450, pump)
            return False

        GLib.timeout_add(700, pump)

    app.connect("activate", on_activate)
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
