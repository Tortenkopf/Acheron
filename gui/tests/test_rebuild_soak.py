# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Soak tests for the GUI memory leak across Device Overview rebuilds
(`.scratch/gui-memory-leak`). They drive the app's real rebuild path —
`build_status_wrapped_view` into one persistent box, through a `rebuild`
closure shaped like `app.py`'s own (clear the box, rebuild into it, pass
`rebuild` itself as `on_change`) — many times over, pumping the main
context in between the way a running main loop would."""

from gi.repository import GLib, Gtk

from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.device_overview import build_status_wrapped_view, open_editor_window
from acheron_gui.gtk_utils import clear_children

from .widget_tree import find_one, toplevels

SOAK_REBUILDS = 15


def _pump_main_context() -> None:
    """Runs whatever the rebuilt widgets queued on the main context, as the
    app's running main loop would between two rebuilds, so anything only
    freed or destroyed from an idle callback is gone before counting."""
    context = GLib.MainContext.default()
    while context.iteration(False):
        pass


class _App:
    """The app's main window and its rebuild closure, minus the Daemon
    plumbing: `content_box` lives inside a real (never presented) window,
    as it does in `app.py`, so its widgets have a root to leave."""

    def __init__(self, ui_state: dict | None = None):
        self.stub = DaemonStub()
        self.ui_state = ui_state if ui_state is not None else {"dest": "grid", "selected_layer": "base"}
        self.window = Gtk.Window()
        self.content_box = Gtk.Box()
        self.window.set_child(self.content_box)
        self.rebuild()

    def rebuild(self) -> None:
        clear_children(self.content_box)
        state = self.stub.get_state()
        self.content_box.append(
            build_status_wrapped_view(
                self.stub,
                self.stub.get_config(),
                state["profile"],
                state["layer"],
                "running_connected",
                self.rebuild,
                self.ui_state,
            )
        )
        _pump_main_context()

    def input_button(self, label: str) -> Gtk.Button:
        face = f"<b>{GLib.markup_escape_text(label)}</b>\n"
        return find_one(
            self.content_box,
            lambda w: isinstance(w, Gtk.Button)
            and isinstance(w.get_child(), Gtk.Label)
            and w.get_child().get_label().startswith(face),
        )


def test_rebuilding_the_grid_view_leaves_no_toplevel_window_but_the_main_one():
    app = _App()

    for _ in range(SOAK_REBUILDS):
        app.rebuild()

    assert toplevels() == [app.window]


def test_rebuilding_while_an_editor_is_open_leaves_no_toplevel_window_behind():
    app = _App()

    for _ in range(SOAK_REBUILDS):
        app.input_button("1").emit("clicked")
        assert len(toplevels()) == 2
        app.rebuild()

    assert toplevels() == [app.window]


def test_opening_and_closing_an_editor_leaves_no_toplevel_window_behind():
    app = _App()
    btn = app.input_button("1")

    for _ in range(SOAK_REBUILDS):
        btn.emit("clicked")
        open_editor_window(btn).emit("close-request")
        _pump_main_context()

    assert toplevels() == [app.window]
