"""Screenshot harness for the per-key Binding editor + key picker (ticket 95).

Sibling of `shot_library.py` — drives the real `AcheronApplication` against a
`DaemonStub`, opens `device_overview.make_input_button`'s modal Binding-editor
`Gtk.Window` for a chosen Input, drives its Action / Trigger dropdowns, and
screenshots the window from inside its own process (toplevel GSK renderer →
`Gdk.Texture` → PNG). No external screenshot tool or portal.

The real GUI holds the `com.acheron.gui` application id on the session bus, so
this harness forces a NON_UNIQUE private id (ticket 94's note for 95+).

Usage:  python3 gui/tools/shot_binding_editor.py OUTDIR
Writes  OUTDIR/be_*.png plus OUTDIR/be_models.txt (every dropdown's items).
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


def _dropdowns(root):
    return [w for w in _walk(root) if isinstance(w, Gtk.DropDown) and w.get_model() is not None]


def _items(dd):
    m = dd.get_model()
    return [m.get_string(i) for i in range(m.get_n_items())]


def _editor_window(app):
    """The single modal Binding-editor Gtk.Window (make_input_button exposes
    it on every grid button as `.binding_editor_window`)."""
    for w in _walk(app.get_active_window()):
        bew = getattr(w, "binding_editor_window", None)
        if bew is not None:
            return bew
    raise AssertionError("no binding_editor_window found in the widget tree")


def _input_button(app, title_suffix: str):
    for w in _walk(app.get_active_window()):
        bew = getattr(w, "binding_editor_window", None)
        if bew is not None and bew.get_title().split("/")[-1].strip() == title_suffix:
            return w
    raise AssertionError(f"no input button whose editor title ends {title_suffix!r}")


def _action_trigger_dd(win):
    add = tdd = None
    for dd in _dropdowns(win):
        its = _items(dd)
        if "Keypress" in its:
            add = dd
        elif "Hold-to-repeat" in its:
            tdd = dd
    return add, tdd


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
    dump = open(outdir / "be_models.txt", "w")

    def log(*a):
        line = " ".join(str(x) for x in a)
        print(line)
        print(line, file=dump)

    stub = DaemonStub()
    app = AcheronApplication(client=stub, systemd_client=_FakeSystemd())
    app.set_application_id("com.acheron.gui.shot95")
    app.set_flags(Gio.ApplicationFlags.NON_UNIQUE)

    def open_editor(title_suffix):
        btn = _input_button(app, title_suffix)
        bew = btn.binding_editor_window
        bew.set_transient_for(app.get_active_window())
        bew.present()
        return bew

    def select_action(bew, label):
        add, _ = _action_trigger_dd(bew)
        add.set_selected(_items(add).index(label))

    def report(bew, tag):
        add, tdd = _action_trigger_dd(bew)
        log(f"--- {tag} ---")
        log("  Action  :", _items(add) if add else "NONE")
        log("  Trigger :", _items(tdd) if tdd else "NONE")
        if tdd:
            log("  Trigger selected index:", tdd.get_selected(),
                "sensitive:", tdd.get_sensitive())

    held = {}

    def s_open_grid():
        held["bew"] = open_editor("1")

    def s_grid_default_shot():
        bew = held["bew"]
        report(bew, "grid Key 1, fresh binding (Action defaults Keypress, Trigger Hold-to-repeat)")
        _shot(bew, outdir / "be_01_grid_keypress_default.png")

    def s_keypress_picker_shot():
        bew = held["bew"]
        log("  window natural size:", bew.get_width(), "x", bew.get_height(),
            "(screen 1920x1080)")
        _shot(bew, outdir / "be_02_grid_keypress_picker.png")

    def s_controller():
        select_action(held["bew"], "Controller Button")

    def s_controller_shot():
        bew = held["bew"]
        report(bew, "grid Key 1, Action = Controller Button (Fire-once must be gone)")
        _shot(bew, outdir / "be_03_grid_controller_button.png")

    def s_switch_profile():
        select_action(held["bew"], "Switch Profile")

    def s_switch_profile_shot():
        bew = held["bew"]
        report(bew, "grid Key 1, Action = Switch Profile (Trigger locked Fire-once)")
        _shot(bew, outdir / "be_04_grid_switch_profile.png")

    def s_open_wheel():
        held["bew"].close()
        held["bew"] = open_editor("Wheel ▲")

    def s_wheel_shot():
        bew = held["bew"]
        report(bew, "wheel_scroll_up (non-grid): no Axis, no Analog-repeat; Trigger defaults Fire-once")
        _shot(bew, outdir / "be_05_wheel_scroll_up.png")

    def s_chord():
        from acheron_gui.binding_editor import build_chord_binding_dialog

        cfg = stub.get_config()
        dlg = build_chord_binding_dialog(
            stub, cfg, "Default", "base",
            ["grid_r1c1", "grid_r1c2"], None,
            lambda: None, app.get_active_window(),
        )
        dlg.present()
        held["dlg"] = dlg

    def s_chord_shot():
        dlg = held["dlg"]
        add, tdd = _action_trigger_dd(dlg)
        log("--- Chord binding dialog (members grid_r1c1+grid_r1c2) ---")
        log("  Action  :", _items(add) if add else "NONE",
            "(no Switch Profile, no Axis)")
        log("  Trigger :", _items(tdd) if tdd else "NONE",
            "(no Analog-repeat); selected", tdd.get_selected() if tdd else "-")
        _shot(dlg, outdir / "be_06_chord_binding_dialog.png")

    steps = [
        s_open_grid,
        s_grid_default_shot,
        s_keypress_picker_shot,
        s_controller,
        s_controller_shot,
        s_switch_profile,
        s_switch_profile_shot,
        s_open_wheel,
        s_wheel_shot,
        s_chord,
        s_chord_shot,
        app.quit,
    ]

    def pump():
        if not steps:
            return False
        try:
            steps.pop(0)()
        except Exception as exc:  # noqa: BLE001
            log("STEP ERROR:", repr(exc))
            app.quit()
            return False
        GLib.timeout_add(600, pump)
        return False

    app.connect("activate", lambda _a: GLib.timeout_add(1100, pump))
    app.run([sys.argv[0]])
    dump.close()


if __name__ == "__main__":
    main()
