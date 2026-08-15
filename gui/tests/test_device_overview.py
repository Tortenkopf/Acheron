from gi.repository import Gtk

from acheron_gui.daemon_client import DaemonError
from acheron_gui.daemon_stub import DaemonStub
from acheron_gui.device_overview import build_main_view
from acheron_gui.inputs import ALL_INPUTS

from .widget_tree import find_all, find_one


def _build(stub, ui_state):
    config = stub.get_config()
    profile, layer, _toggles, _connected = stub.get_state()
    return build_main_view(stub, config, profile, layer, lambda: None, ui_state)


def _action_table_toggle(root):
    return find_one(
        root, lambda w: isinstance(w, Gtk.ToggleButton) and w.get_label() in ("Action Table ▸", "Action Table ◂")
    )


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
    toggle = _action_table_toggle(root)

    toggle.set_active(True)
    assert ui_state["table_open"] is True

    # Simulate a rebuild (a fresh widget tree from a fresh Gtk.Revealer,
    # which defaults closed) — the *state dict* carrying table_open across
    # it is what should keep the sidebar open, per ticket 09.
    rebuilt_root = _build(stub, ui_state)
    rebuilt_revealer = find_all(rebuilt_root, lambda w: isinstance(w, Gtk.Revealer))[0]
    assert rebuilt_revealer.get_reveal_child() is True


def test_profile_controls_are_disabled_pending_a_later_ticket():
    stub = DaemonStub()

    root = _build(stub, {"table_open": False})

    profile_buttons = find_all(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Default")
    assert profile_buttons and all(not b.get_sensitive() for b in profile_buttons)


def test_clicking_the_held_tab_switches_which_layer_is_shown_and_edited():
    stub = DaemonStub()
    stub.set_binding("grid_r1c1", "held", {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": []})
    ui_state = {"table_open": False, "expanded_rows": set()}

    root = _build(stub, ui_state)
    held_btn = find_one(root, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Held")
    assert held_btn.get_sensitive(), "the Held tab itself must always be clickable"

    held_btn.emit("clicked")
    assert ui_state["selected_layer"] == "held"

    rebuilt = _build(stub, ui_state)
    grid_r1c1_btn = find_one(rebuilt, lambda w: "bound" in w.get_css_classes() if isinstance(w, Gtk.MenuButton) else False)
    heading = find_one(grid_r1c1_btn.get_popover(), lambda w: "heading" in w.get_css_classes())
    assert heading.get_label() == "Default / held / 1"


def test_mode_key_button_is_disabled_under_the_default_layer_switch_role():
    stub = DaemonStub()

    root = _build(stub, {"table_open": False})

    mode_btn = find_one(root, lambda w: isinstance(w, Gtk.MenuButton) and "mode-key" in w.get_css_classes())
    assert not mode_btn.get_sensitive()


def test_toggling_mode_key_role_to_bound_enables_its_binding_editor():
    stub = DaemonStub()
    ui_state = {"table_open": False}

    root = _build(stub, ui_state)
    role_btn = find_one(root, lambda w: isinstance(w, Gtk.ToggleButton) and w.get_label() == "Mode key: Layer-shift")
    role_btn.set_active(True)

    assert stub.get_config()["profiles"]["Default"]["mode_key_role"] == "bound"

    rebuilt = _build(stub, ui_state)
    mode_btn = find_one(rebuilt, lambda w: isinstance(w, Gtk.MenuButton) and "mode-key" in w.get_css_classes())
    assert mode_btn.get_sensitive()


class _RoleFailsDaemonStub(DaemonStub):
    def set_mode_key_role(self, role):
        raise DaemonError("dispatch task is not responding")


def test_a_failed_mode_key_role_change_reverts_the_toggle_button():
    stub = _RoleFailsDaemonStub()

    root = _build(stub, {"table_open": False})
    role_btn = find_one(root, lambda w: isinstance(w, Gtk.ToggleButton) and w.get_label() == "Mode key: Layer-shift")

    role_btn.set_active(True)

    # The Daemon call failed, so the visible toggle state must not disagree
    # with what the Daemon actually has (still layer_switch) — matching
    # build_binding_editor's Save/Clear error handling.
    assert role_btn.get_active() is False
    assert stub.get_config()["profiles"]["Default"]["mode_key_role"] == "layer_switch"
