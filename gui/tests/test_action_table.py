from gi.repository import Gtk

from acheron_gui.action_table import build_action_table
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.inputs import ALL_INPUTS

from .widget_tree import find_all, find_one


def _listbox(root):
    return find_one(root, lambda w: isinstance(w, Gtk.ListBox))


def _row_count(listbox):
    return len(find_all(listbox, lambda w: isinstance(w, Gtk.Expander)))


def test_only_bound_inputs_show_by_default():
    stub = DaemonStub()
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []})

    table = build_action_table(stub, stub.get_config(), "Default", "base", lambda: None, {"expanded_rows": set()})

    assert _row_count(_listbox(table)) == 1


def test_show_all_checkbox_reveals_passthrough_rows():
    stub = DaemonStub()
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []})

    table = build_action_table(stub, stub.get_config(), "Default", "base", lambda: None, {"expanded_rows": set()})
    show_all = find_one(table, lambda w: isinstance(w, Gtk.CheckButton))
    show_all.set_active(True)

    assert _row_count(_listbox(table)) == len(ALL_INPUTS)

    show_all.set_active(False)
    assert _row_count(_listbox(table)) == 1


def test_expanded_row_state_survives_a_rebuild():
    stub = DaemonStub()
    stub.set_binding("grid_r1c1", "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []})
    ui_state = {"expanded_rows": set()}

    table = build_action_table(stub, stub.get_config(), "Default", "base", lambda: None, ui_state)
    expander = find_one(_listbox(table), lambda w: isinstance(w, Gtk.Expander))
    expander.set_expanded(True)

    assert ui_state["expanded_rows"] == {"grid_r1c1"}

    # Simulate a rebuild (a fresh Gtk.Expander tree, which defaults
    # collapsed) — the state dict carrying expanded_rows across it is what
    # should keep the row open.
    rebuilt = build_action_table(stub, stub.get_config(), "Default", "base", lambda: None, ui_state)
    rebuilt_expander = find_one(_listbox(rebuilt), lambda w: isinstance(w, Gtk.Expander))
    assert rebuilt_expander.get_expanded() is True


def test_held_layer_is_shown_independently_of_base():
    stub = DaemonStub()
    stub.set_binding("grid_r1c1", "held", {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []})

    base_table = build_action_table(stub, stub.get_config(), "Default", "base", lambda: None, {"expanded_rows": set()})
    held_table = build_action_table(stub, stub.get_config(), "Default", "held", lambda: None, {"expanded_rows": set()})

    assert _row_count(_listbox(base_table)) == 0
    assert _row_count(_listbox(held_table)) == 1
