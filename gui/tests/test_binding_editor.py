from gi.repository import Gtk

from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.device_overview import make_input_button

from .widget_tree import button_labeled, find_one


def _entry_labeled(root, label_text):
    row = find_one(root, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == label_text)
    return find_one(row, lambda w: isinstance(w, Gtk.Entry))


def _row_label_text(box):
    child = box.get_first_child()
    return child.get_label() if isinstance(child, Gtk.Label) else None


def test_clicking_an_unbound_key_opens_editor_defaulted_to_fire_once_keypress():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))

    assert "empty" in btn.get_css_classes()
    popover = btn.get_popover()
    heading = find_one(popover, lambda w: "heading" in w.get_css_classes())
    assert heading.get_label() == "Default / base / 1"


def test_saving_a_keypress_binding_calls_set_binding_and_closes_popover():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = btn.get_popover()

    key_entry = _entry_labeled(popover, "Key")
    key_entry.set_text("KEY_F1")
    key_entry.emit("changed")

    button_labeled(popover, "Save").emit("clicked")

    assert stub.calls == [
        (
            "set_binding",
            "grid_r1c1",
            "base",
            {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []},
        )
    ]
    assert changed == [1]


def test_clearing_an_existing_binding_calls_clear_binding():
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1", "base", {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []}
    )
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = btn.get_popover()

    button_labeled(popover, "Clear (passthrough)").emit("clicked")

    assert stub.calls[-1] == ("clear_binding", "grid_r1c1", "base")
    assert changed == [1]


def test_clearing_an_already_passthrough_input_is_a_noop_but_still_closes():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: changed.append(1))
    popover = btn.get_popover()

    button_labeled(popover, "Clear (passthrough)").emit("clicked")

    assert stub.calls == []
    assert changed == [1]


def test_bound_input_shows_bound_css_class_and_summary():
    stub = DaemonStub()
    stub.set_binding(
        "grid_r1c1",
        "base",
        {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": ["ctrl"]},
    )

    btn = make_input_button(stub, stub.get_config(), "Default", "base", "grid_r1c1", lambda: None)

    assert "bound" in btn.get_css_classes()
    label = btn.get_child()
    assert "Ctrl+F1" in label.get_label()


def test_editing_targets_the_held_layer_independently_of_base():
    stub = DaemonStub()
    changed = []

    btn = make_input_button(stub, stub.get_config(), "Default", "held", "grid_r1c1", lambda: changed.append(1))
    popover = btn.get_popover()
    heading = find_one(popover, lambda w: "heading" in w.get_css_classes())
    assert heading.get_label() == "Default / held / 1"

    key_entry = _entry_labeled(popover, "Key")
    key_entry.set_text("KEY_F1")
    key_entry.emit("changed")
    button_labeled(popover, "Save").emit("clicked")

    assert stub.calls == [
        (
            "set_binding",
            "grid_r1c1",
            "held",
            {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []},
        )
    ]
    assert "grid_r1c1" not in stub.get_config()["profiles"]["Default"]["base"]
