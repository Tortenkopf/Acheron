# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Soak tests for the GUI memory leak across Device Overview rebuilds
and Binding-editor opens (`.scratch/gui-memory-leak`). They drive the
app's real rebuild path — `build_status_wrapped_view` into one persistent
box, through a `rebuild` closure shaped like `app.py`'s own (clear the box,
rebuild into it, pass `rebuild` itself as `on_change`) — many times over,
pumping the main context in between the way a running main loop would.

"Freed" is checked with GObject weak references (`GObject.Object.weak_ref`,
on the widget itself, not its short-lived Python wrapper) to every widget of
a tree, taken while it was still built: after `gc.collect()`, none may still
resolve. A widget kept alive by a reference cycle that crosses GTK's own
C-side ownership (a signal handler capturing its widget's ancestor) never
does."""

import gc

import pytest
from gi.repository import GLib, Gtk

from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.device_overview import build_status_wrapped_view, open_editor_window
from acheron_gui.gtk_utils import clear_children
from acheron_gui.inputs import ACTION_TYPES, input_label

from .widget_tree import button_labeled, colour_buttons, find_all, find_one, toplevels, walk

SOAK_REBUILDS = 15


def _pump_main_context() -> None:
    """Runs whatever the rebuilt widgets queued on the main context, as the
    app's running main loop would between two rebuilds, so anything only
    freed or destroyed from an idle callback is gone before counting."""
    context = GLib.MainContext.default()
    while context.iteration(False):
        pass


def _weak_widgets(root: Gtk.Widget) -> list:
    """A GObject weak reference to every widget currently in `root`'s tree."""
    return [w.weak_ref() for w in walk(root)]


def _settle() -> None:
    """Pumps the main context and runs the cycle collector to a fixed point —
    one collection can free a widget whose disposal only then lets GTK drop
    the last references to others."""
    _pump_main_context()
    while gc.collect():
        _pump_main_context()


def _survivors(refs: list) -> list[str]:
    """The type names of every widget in `refs` still alive once `_settle()`
    has freed everything it can."""
    _settle()
    return sorted(type(w).__name__ for w in (r() for r in refs) if w is not None)


def _action_dropdown(root: Gtk.Widget) -> Gtk.DropDown:
    """The (first) Action dropdown in a Binding editor: the plain editor
    has one, the grid key's dual-stage panel one per stage."""
    row = find_all(
        root,
        lambda w: isinstance(w, Gtk.Box)
        and isinstance(w.get_first_child(), Gtk.Label)
        and w.get_first_child().get_label() == "Action",
    )[0]
    return find_one(row, lambda w: isinstance(w, Gtk.DropDown))


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

    def colour_button(self) -> Gtk.Button:
        return colour_buttons(self.content_box)[0]


def test_rebuilding_the_grid_view_leaves_no_toplevel_window_but_the_main_one():
    app = _App()

    for _ in range(SOAK_REBUILDS):
        app.rebuild()

    assert toplevels() == [app.window]


def _lighting_app() -> _App:
    """An `_App` on the Lighting tab, with an effect that has a colour
    swatch so the tab builds a colour button."""
    app = _App({"dest": "lighting", "selected_layer": "base"})
    app.stub.set_lighting({"type": "fixed_effect", "effect": {"type": "static", "colour": {"r": 1, "g": 1, "b": 1}}}, 50)
    app.rebuild()
    return app


def test_rebuilding_the_lighting_view_leaves_no_toplevel_window_but_the_main_one():
    app = _lighting_app()

    for _ in range(SOAK_REBUILDS):
        app.rebuild()

    assert toplevels() == [app.window]


def test_rebuilding_while_a_lighting_colour_picker_is_open_leaves_no_toplevel_window_behind():
    app = _lighting_app()

    for _ in range(SOAK_REBUILDS):
        app.colour_button().emit("clicked")
        assert len(toplevels()) == 2
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


def test_rebuilding_while_an_editor_is_open_frees_that_editor():
    app = _App()
    refs = []

    for _ in range(SOAK_REBUILDS):
        app.input_button("1").emit("clicked")
        refs += _weak_widgets(open_editor_window(app.input_button("1")))
        app.rebuild()

    assert _survivors(refs) == []


@pytest.mark.parametrize("inp", ["grid_r1c1", "mode_key"])
def test_opening_and_closing_an_editor_frees_it(inp):
    app = _App()
    btn = app.input_button(input_label(inp))
    refs = []

    for _ in range(SOAK_REBUILDS):
        btn.emit("clicked")
        window = open_editor_window(btn)
        refs += _weak_widgets(window)
        window.emit("close-request")
        del window

    assert _survivors(refs) == []
    assert toplevels() == [app.window]


@pytest.mark.parametrize("inp", ["grid_r1c1", "mode_key"])
def test_saving_and_closing_an_editor_frees_it(inp):
    app = _App()
    refs = []

    for _ in range(SOAK_REBUILDS):
        btn = app.input_button(input_label(inp))
        btn.emit("clicked")
        window = open_editor_window(btn)
        refs += _weak_widgets(window)
        button_labeled(window, "Save").emit("clicked")
        del window, btn

    assert _survivors(refs) == []
    assert toplevels() == [app.window]


def test_applying_in_a_grid_keys_editor_frees_the_panel_it_replaces():
    app = _App()
    btn = app.input_button("1")
    btn.emit("clicked")
    window = open_editor_window(btn)
    refs = []

    for _ in range(SOAK_REBUILDS):
        refs += _weak_widgets(window)
        button_labeled(window, "Apply").emit("clicked")
    refs += _weak_widgets(window)
    window.emit("close-request")
    del window

    assert _survivors(refs) == []
    assert toplevels() == [app.window]


_EDITOR_KINDS = [
    (inp, kind)
    for inp in ("grid_r1c1", "mode_key")
    for kind, _ in ACTION_TYPES
    if kind != "axis" or inp == "grid_r1c1"  # Axis is offered on grid keys only
]


@pytest.mark.parametrize(("inp", "kind"), _EDITOR_KINDS)
def test_each_action_kinds_editor_fields_are_freed(inp, kind):
    app = _App()
    btn = app.input_button(input_label(inp))
    refs = []

    for _ in range(SOAK_REBUILDS // 3):
        btn.emit("clicked")
        window = open_editor_window(btn)
        action_dd = _action_dropdown(window)
        offered = [action_dd.get_model().get_string(i) for i in range(action_dd.get_model().get_n_items())]
        chosen = offered.index(dict(ACTION_TYPES)[kind])
        action_dd.set_selected(chosen)
        refs += _weak_widgets(window)
        # On to another kind, so the chosen kind's fields leave the tree
        # while the editor is still open.
        action_dd.set_selected(1 if chosen == 0 else 0)
        refs += _weak_widgets(window)
        window.emit("close-request")
        del window, action_dd

    assert _survivors(refs) == []
    assert toplevels() == [app.window]


def _live_widget_wrappers() -> int:
    """How many `Gtk.Widget` Python wrappers are alive right now — the
    leak's own symptom, which grew by about 558 per rebuild before this
    effort's fixes."""
    _settle()
    return sum(1 for o in gc.get_objects() if isinstance(o, Gtk.Widget))


def _library_app(tab: str) -> _App:
    """An `_App` on the Library destination's `tab`, with one Macro and one
    Stepper, each with a step/item, so its browse list and editor columns
    are built."""
    app = _App({"dest": "library", "library_tab": tab, "selected_layer": "base"})
    app.stub.create_macro("Combo", [{"type": "key_down", "key": "KEY_A"}])
    app.stub.create_stepper("Wheel", [{"type": "key", "key": "KEY_1"}])
    app.rebuild()
    return app


_DESTINATIONS = {
    "grid": _App,
    "library-macros": lambda: _library_app("macros"),
    "library-steppers": lambda: _library_app("steppers"),
    "lighting": _lighting_app,
}


@pytest.mark.parametrize("dest", _DESTINATIONS)
def test_rebuilding_frees_every_widget_of_the_previous_build(dest):
    app = _DESTINATIONS[dest]()
    first_build = _weak_widgets(app.content_box.get_first_child())

    for _ in range(SOAK_REBUILDS):
        app.rebuild()

    assert _survivors(first_build) == []
    assert toplevels() == [app.window]


@pytest.mark.parametrize("dest", _DESTINATIONS)
def test_rebuilding_does_not_grow_the_live_widget_wrapper_count(dest):
    app = _DESTINATIONS[dest]()
    for _ in range(3):
        app.rebuild()
    settled = _live_widget_wrappers()

    for _ in range(SOAK_REBUILDS):
        app.rebuild()

    assert _live_widget_wrappers() <= settled
