from gi.repository import Gtk

from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.device_overview import build_main_view
from acheron_gui.inputs import ALL_INPUTS

from .widget_tree import find_all


def _build(stub, ui_state):
    config = stub.get_config()
    profile, layer, _toggles, _connected = stub.get_state()
    return build_main_view(stub, config, profile, layer, lambda: None, ui_state)


def test_device_overview_renders_one_button_per_input():
    stub = DaemonStub()

    root = _build(stub, {"table_open": False})

    # Filtered to "bound"/"empty"-classed buttons — the tray mock's own
    # "Quick switch" control is also a Gtk.MenuButton but isn't an Input.
    input_buttons = find_all(
        root,
        lambda w: isinstance(w, Gtk.MenuButton) and ("bound" in w.get_css_classes() or "empty" in w.get_css_classes()),
    )
    assert len(input_buttons) == len(ALL_INPUTS)


def test_action_table_sidebar_closed_by_default():
    stub = DaemonStub()

    root = _build(stub, {"table_open": False})

    revealer = find_all(root, lambda w: isinstance(w, Gtk.Revealer))[0]
    assert revealer.get_reveal_child() is False


def test_action_table_open_state_survives_a_rebuild():
    stub = DaemonStub()
    ui_state = {"table_open": False}
    root = _build(stub, ui_state)
    toggle = find_all(root, lambda w: isinstance(w, Gtk.ToggleButton))[0]

    toggle.set_active(True)
    assert ui_state["table_open"] is True

    # Simulate a rebuild (a fresh widget tree from a fresh Gtk.Revealer,
    # which defaults closed) — the *state dict* carrying table_open across
    # it is what should keep the sidebar open, per ticket 09.
    rebuilt_root = _build(stub, ui_state)
    rebuilt_revealer = find_all(rebuilt_root, lambda w: isinstance(w, Gtk.Revealer))[0]
    assert rebuilt_revealer.get_reveal_child() is True


def test_profile_and_layer_controls_are_disabled_pending_later_tickets():
    stub = DaemonStub()

    root = _build(stub, {"table_open": False})

    profile_buttons = find_all(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Default")
    assert profile_buttons and all(not b.get_sensitive() for b in profile_buttons)

    held_buttons = find_all(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Held")
    assert held_buttons and all(not b.get_sensitive() for b in held_buttons)
