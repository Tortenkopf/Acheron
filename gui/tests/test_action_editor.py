# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Pure-construction unit tests for the `_ACTION_EDITORS` table (post-release
ticket 31) — one builder function per Action kind, called directly with a
fresh `BindingDraft` and inspected via `widget_tree` helpers, mirroring
`test_binding_draft.py`'s naming convention alongside `binding_draft.py`.
No full popover is ever built here (that's `test_binding_editor.py`'s job,
now trimmed to genuinely cross-kind behavior) — each test calls exactly one
`_build_*_editor` function and asserts the widget it hands back."""

from gi.repository import Gtk

from acheron_gui.binding_draft import BindingDraft
from acheron_gui.binding_editor import (
    _ACTION_EDITORS,
    _build_axis_editor,
    _build_controller_button_editor,
    _build_keypress_editor,
    _build_macro_editor,
    _build_profile_switch_editor,
    _build_step_editor,
    _build_new_library_entry_button,
)
from acheron_gui.daemon_stub import DaemonStub

from .widget_tree import button_labeled, find_all, find_one


def _draft(starting: dict, *, inp: str = "grid_r1c1", profile: str = "Default") -> BindingDraft:
    return BindingDraft.from_wire(starting, inp=inp, profile=profile)


def _trigger_dd(options: list[tuple[str, str]], selected_key: str) -> Gtk.DropDown:
    dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in options]))
    dd.set_selected([k for k, _ in options].index(selected_key))
    return dd


def _spy_listener():
    registered = []
    return (lambda cb: registered.append(cb)), registered


def _row_label_text(box):
    child = box.get_first_child()
    return child.get_label() if isinstance(child, Gtk.Label) else None


def _labeled_row(root, label_text):
    return find_one(root, lambda w: isinstance(w, Gtk.Box) and _row_label_text(w) == label_text)


_TRIGGER_OPTIONS = [("fire_once", "Fire-once"), ("hold_to_repeat", "Hold-to-repeat"), ("toggle", "Toggle")]


# --- keypress ---


def test_keypress_editor_returns_a_key_row_and_a_modifier_checkbox_row():
    stub = DaemonStub()
    draft = _draft({"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_F1", "modifiers": ["ctrl"]})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "hold_to_repeat")
    set_trigger_listener, registered = _spy_listener()

    widget = _build_keypress_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    key_row = _labeled_row(widget, "Key")
    assert key_row is not None
    mod_checks = find_all(widget, lambda w: isinstance(w, Gtk.CheckButton))
    assert {cb.get_label() for cb in mod_checks} == {"ctrl", "shift", "alt", "super"}
    assert next(cb for cb in mod_checks if cb.get_label() == "ctrl").get_active()
    # The modifier warning's Trigger-mode listener is registered through the
    # caller's disconnect-tracked slot, not connected directly (ticket 42).
    assert len(registered) == 1


def test_keypress_editor_applies_the_picker_css_class():
    stub = DaemonStub()
    draft = _draft({"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_A", "modifiers": []})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "hold_to_repeat")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_keypress_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        "deep-picker", set_trigger_listener, lambda: None,
    )

    assert find_all(widget, lambda w: "deep-picker" in w.get_css_classes()) != []


def test_keypress_editor_round_trips_a_mouse_button_code():
    # Ticket 42's picker makes a mouse-button one click away — clicking "Left"
    # in the Key row must set the draft's key to the raw BTN_ wire code.
    stub = DaemonStub()
    draft = _draft({"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_A", "modifiers": []})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "hold_to_repeat")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_keypress_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    button_labeled(widget, "Left").emit("clicked")

    assert draft.keypress["key"] == "BTN_LEFT"


def test_keypress_editor_modifier_checkbox_updates_the_draft_sorted():
    stub = DaemonStub()
    draft = _draft({"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_A", "modifiers": []})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "hold_to_repeat")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_keypress_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    for label in ("shift", "ctrl"):
        find_one(widget, lambda w, label=label: isinstance(w, Gtk.CheckButton) and w.get_label() == label).set_active(
            True
        )

    assert draft.keypress["modifiers"] == ["ctrl", "shift"]


# --- profile_switch ---


def test_profile_switch_editor_returns_a_target_profile_row_sorted_by_name():
    stub = DaemonStub()
    stub.create_profile("Zeta")
    stub.create_profile("Alpha")
    draft = _draft({"trigger": "fire_once", "type": "profile_switch", "target": "Default"})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, registered = _spy_listener()

    widget = _build_profile_switch_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    row = _labeled_row(widget, "Target Profile")
    dd = find_one(row, lambda w: isinstance(w, Gtk.DropDown))
    labels = [dd.get_model().get_string(i) for i in range(dd.get_model().get_n_items())]
    assert labels == ["Alpha", "Default", "Zeta"]
    assert registered == []  # only Keypress ever registers a trigger listener


def test_profile_switch_editor_changing_the_dropdown_updates_the_draft():
    stub = DaemonStub()
    stub.create_profile("Gaming")
    draft = _draft({"trigger": "fire_once", "type": "profile_switch", "target": "Default"})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_profile_switch_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    dd = find_one(widget, lambda w: isinstance(w, Gtk.DropDown))
    profile_names = sorted(stub.get_config()["profiles"].keys())
    dd.set_selected(profile_names.index("Gaming"))

    assert draft.profile_switch["target"] == "Gaming"


# --- controller_button ---


def test_controller_button_editor_returns_a_button_row_and_applies_the_picker_css_class():
    stub = DaemonStub()
    draft = _draft({"trigger": "hold_to_repeat", "type": "controller_button", "button": "BTN_SOUTH"})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "hold_to_repeat")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_controller_button_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        "deep-picker", set_trigger_listener, lambda: None,
    )

    assert _labeled_row(widget, "Button") is not None
    assert find_all(widget, lambda w: "deep-picker" in w.get_css_classes()) != []


def test_controller_button_editor_picking_a_button_updates_the_draft():
    stub = DaemonStub()
    draft = _draft({"trigger": "hold_to_repeat", "type": "controller_button", "button": "BTN_SOUTH"})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "hold_to_repeat")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_controller_button_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    button_labeled(widget, "B").emit("clicked")

    assert draft.controller_button["button"] == "BTN_EAST"


# --- axis ---


def test_axis_editor_returns_a_target_row_and_never_touches_save_btn():
    # Ticket 31: Axis used to call `save_btn.set_sensitive(...)` inline —
    # this builder never receives a `save_btn` at all any more, `BindingDraft
    # .on_change` resyncs it generically instead.
    stub = DaemonStub()
    draft = _draft({"type": "axis", "target": None})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_axis_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    assert _labeled_row(widget, "Target") is not None
    assert not draft.is_valid()


def test_axis_editor_picking_a_target_updates_the_draft_via_on_change():
    stub = DaemonStub()
    draft = _draft({"type": "axis", "target": None})
    seen = []
    draft.on_change = lambda: seen.append(draft.is_valid())
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_axis_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    find_one(widget, lambda w: isinstance(w, Gtk.Button) and w.get_tooltip_text() == "Left Trigger").emit("clicked")

    assert draft.axis["target"] == "left_trigger"
    assert seen[-1] is True


def test_axis_editor_excludes_the_current_input_from_claimed_by():
    # Ticket 60's cross-key toast is keyed off `claimed_by`, built from every
    # *other* Input on this Layer — a key must never be told its own current
    # target is "already claimed" by itself.
    stub = DaemonStub()
    stub.set_axis_assignment("grid_r1c1", "base", "left_trigger")
    stub.set_axis_assignment("grid_r1c2", "base", "right_trigger")
    draft = _draft({"type": "axis", "target": "left_trigger"})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_axis_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    find_one(widget, lambda w: isinstance(w, Gtk.Button) and w.get_tooltip_text() == "Left Trigger").emit("clicked")
    assert find_all(widget, lambda w: "toast" in w.get_css_classes()) == []

    find_one(widget, lambda w: isinstance(w, Gtk.Button) and w.get_tooltip_text() == "Right Trigger").emit("clicked")
    toast = find_one(widget, lambda w: "toast" in w.get_css_classes())
    assert "2" in toast.get_label()


# --- step ---


def test_step_editor_shows_the_empty_library_message_and_disables_nothing_itself():
    stub = DaemonStub()
    draft = _draft({"trigger": "fire_once", "type": "step", "stepper_id": None, "direction": "forward"})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_step_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    assert find_one(widget, lambda w: isinstance(w, Gtk.Label) and "No Steppers in the library yet" in w.get_label())
    assert find_one(widget, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New Stepper")
    assert not draft.is_valid()


def test_step_editor_defaults_to_the_first_entry_when_the_current_id_is_unknown():
    stub = DaemonStub()
    stub.create_stepper("Weapon Wheel", [])
    draft = _draft({"trigger": "fire_once", "type": "step", "stepper_id": None, "direction": "forward"})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_step_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    stepper_dd = find_one(_labeled_row(widget, "Stepper"), lambda w: isinstance(w, Gtk.DropDown))
    assert stepper_dd.get_model().get_string(stepper_dd.get_selected()) == "Weapon Wheel"
    assert draft.step["stepper_id"] is not None
    direction_dd = find_one(_labeled_row(widget, "Direction"), lambda w: isinstance(w, Gtk.DropDown))
    assert direction_dd.get_model().get_string(direction_dd.get_selected()) == "Forward"


def test_step_editor_direction_dropdown_updates_the_draft():
    stub = DaemonStub()
    stepper_id = stub.create_stepper("Weapon Wheel", [])
    draft = _draft({"trigger": "fire_once", "type": "step", "stepper_id": stepper_id, "direction": "forward"})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_step_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    direction_dd = find_one(_labeled_row(widget, "Direction"), lambda w: isinstance(w, Gtk.DropDown))
    direction_dd.set_selected(1)  # Backward

    assert draft.step == {"stepper_id": stepper_id, "direction": "backward"}


def test_step_editor_new_stepper_creates_assigns_and_rerenders():
    stub = DaemonStub()
    config = stub.get_config()
    draft = _draft({"trigger": "fire_once", "type": "step", "stepper_id": None, "direction": "forward"})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()
    rerender_calls = []

    widget = _build_step_editor(
        stub, config, "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: rerender_calls.append(1),
    )

    new_btn = find_one(widget, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New Stepper")
    popover = new_btn.get_popover()
    find_one(popover, lambda w: isinstance(w, Gtk.Entry)).set_text("Fresh Wheel")
    find_one(popover, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Create").emit("clicked")

    (stepper_id,) = [sid for sid, s in config["steppers"].items() if s["name"] == "Fresh Wheel"]
    assert draft.step == {"stepper_id": stepper_id, "direction": "forward"}
    assert rerender_calls == [1]


# --- macro ---


def test_macro_editor_shows_the_empty_library_message():
    stub = DaemonStub()
    draft = _draft({"trigger": "fire_once", "type": "macro", "macro_id": None})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_macro_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    assert find_one(widget, lambda w: isinstance(w, Gtk.Label) and "No Macros in the library yet" in w.get_label())
    assert find_one(widget, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New Macro")
    assert not draft.is_valid()


def test_macro_editor_defaults_to_the_first_entry_when_the_current_id_is_unknown():
    stub = DaemonStub()
    stub.create_macro("Screenshot Combo", [])
    draft = _draft({"trigger": "fire_once", "type": "macro", "macro_id": None})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_macro_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    macro_dd = find_one(_labeled_row(widget, "Macro"), lambda w: isinstance(w, Gtk.DropDown))
    assert macro_dd.get_model().get_string(macro_dd.get_selected()) == "Screenshot Combo"
    assert draft.macro["macro_id"] is not None
    assert draft.is_valid()


def test_macro_editor_preselects_the_existing_macro():
    stub = DaemonStub()
    stub.create_macro("Other Macro", [])
    macro_id = stub.create_macro("Test Macro", [])
    draft = _draft({"trigger": "fire_once", "type": "macro", "macro_id": macro_id})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()

    widget = _build_macro_editor(
        stub, stub.get_config(), "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: None,
    )

    macro_dd = find_one(_labeled_row(widget, "Macro"), lambda w: isinstance(w, Gtk.DropDown))
    assert macro_dd.get_model().get_string(macro_dd.get_selected()) == "Test Macro"


def test_macro_editor_new_macro_creates_assigns_and_rerenders():
    stub = DaemonStub()
    config = stub.get_config()
    draft = _draft({"trigger": "fire_once", "type": "macro", "macro_id": None})
    trigger_dd = _trigger_dd(_TRIGGER_OPTIONS, "fire_once")
    set_trigger_listener, _ = _spy_listener()
    rerender_calls = []

    widget = _build_macro_editor(
        stub, config, "Default", "base", "grid_r1c1", draft, trigger_dd, _TRIGGER_OPTIONS,
        None, set_trigger_listener, lambda: rerender_calls.append(1),
    )

    new_btn = find_one(widget, lambda w: isinstance(w, Gtk.MenuButton) and w.get_label() == "+ New Macro")
    popover = new_btn.get_popover()
    find_one(popover, lambda w: isinstance(w, Gtk.Entry)).set_text("Fresh Macro")
    find_one(popover, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Create").emit("clicked")

    (macro_id,) = [mid for mid, m in config["macros"].items() if m["name"] == "Fresh Macro"]
    assert draft.macro == {"macro_id": macro_id}
    assert rerender_calls == [1]


# --- the table itself ---


def test_action_editors_table_covers_every_action_type():
    from acheron_gui.inputs import ACTION_TYPES

    assert set(_ACTION_EDITORS.keys()) == {k for k, _ in ACTION_TYPES}


# --- shared "+ New …" helper (post-release ticket 31) ---


def test_new_library_entry_button_calls_create_with_the_client_and_submitted_name():
    stub = DaemonStub()
    created = []

    btn = _build_new_library_entry_button(
        stub, "+ New Thing", "Creating a Thing", lambda client, name: (client, name), created.append
    )

    popover = btn.get_popover()
    find_one(popover, lambda w: isinstance(w, Gtk.Entry)).set_text("A Thing")
    find_one(popover, lambda w: isinstance(w, Gtk.Button) and w.get_label() == "Create").emit("clicked")

    assert created == [(stub, "A Thing")]
