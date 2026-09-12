# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

from acheron_gui.binding_draft import BindingDraft


def _keypress(trigger="hold_to_repeat", key="KEY_F1", modifiers=None):
    return {"trigger": trigger, "type": "keypress", "key": key, "modifiers": modifiers or []}


# --- from_wire: seeding the active kind ---


def test_from_wire_seeds_keypress_from_starting():
    draft = BindingDraft.from_wire(
        _keypress(modifiers=["ctrl"]), inp="grid_r1c1", profile="Default"
    )
    assert draft.kind == "keypress"
    assert draft.trigger == "hold_to_repeat"
    assert draft.keypress == {"key": "KEY_F1", "modifiers": ["ctrl"]}


def test_from_wire_seeds_macro_from_starting():
    starting = {"trigger": "fire_once", "type": "macro", "macro_id": "m1"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert draft.kind == "macro"
    assert draft.macro == {"macro_id": "m1"}


def test_from_wire_seeds_step_from_starting():
    starting = {"trigger": "fire_once", "type": "step", "stepper_id": "s1", "direction": "backward"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert draft.kind == "step"
    assert draft.step == {"stepper_id": "s1", "direction": "backward"}


def test_from_wire_seeds_profile_switch_from_starting():
    starting = {"trigger": "fire_once", "type": "profile_switch", "target": "Gaming"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert draft.kind == "profile_switch"
    assert draft.profile_switch == {"target": "Gaming"}


def test_from_wire_seeds_controller_button_from_starting():
    starting = {"trigger": "hold_to_repeat", "type": "controller_button", "button": "BTN_EAST"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert draft.kind == "controller_button"
    assert draft.controller_button == {"button": "BTN_EAST"}


def test_from_wire_seeds_axis_from_starting():
    starting = {"trigger": "fire_once", "type": "axis", "target": "left_trigger"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert draft.kind == "axis"
    assert draft.axis == {"target": "left_trigger"}


def test_from_wire_tolerates_an_axis_starting_dict_with_no_trigger_key():
    # Ticket 28: an Axis-kind `starting` is this class's own `to_wire()`
    # output round-tripped back in (the dual-stage panel's `stage_starting`,
    # after an unsaved Axis edit survives a stage swap) — and `to_wire()`
    # never puts a "trigger" key on an Axis dict at all. Must not KeyError.
    starting = {"type": "axis", "target": "left_trigger"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert draft.kind == "axis"
    assert draft.axis == {"target": "left_trigger"}
    assert draft.trigger == "hold_to_repeat"


def test_from_wire_defaults_the_missing_trigger_per_input_same_as_a_fresh_binding():
    # The scroll-wheel directions default Fire-once everywhere else
    # (`default_trigger_for`) — the same fallback applies here.
    starting = {"type": "axis", "target": None}
    draft = BindingDraft.from_wire(starting, inp="wheel_scroll_up", profile="Default")
    assert draft.trigger == "fire_once"


# --- from_wire: default-seeding every kind that isn't the active one ---


def test_from_wire_default_seeds_the_inactive_kinds_alongside_a_keypress_starting():
    draft = BindingDraft.from_wire(_keypress(), inp="grid_r1c1", profile="Default")
    assert draft.macro == {"macro_id": None}
    assert draft.step == {"stepper_id": None, "direction": "forward"}
    assert draft.profile_switch == {"target": "Default"}
    assert draft.controller_button == {"button": "BTN_SOUTH"}
    assert draft.axis == {"target": None}


def test_from_wire_default_seeds_keypress_from_the_inputs_own_passthrough_default():
    # grid_r1c2's passthrough default is KEY_2 (see inputs.INPUT_DEFAULT_KEY_CODE)
    # — not a hardcoded "KEY_A" — when Keypress isn't the active kind.
    starting = {"trigger": "fire_once", "type": "macro", "macro_id": "m1"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c2", profile="Default")
    assert draft.keypress == {"key": "KEY_2", "modifiers": []}


def test_from_wire_default_seeds_keypress_key_a_for_an_input_with_no_default_code():
    # wheel_scroll_up has no discrete default keycode — falls back to KEY_A.
    starting = {"trigger": "fire_once", "type": "macro", "macro_id": "m1"}
    draft = BindingDraft.from_wire(starting, inp="wheel_scroll_up", profile="Default")
    assert draft.keypress == {"key": "KEY_A", "modifiers": []}


def test_from_wire_default_seeds_keypress_key_a_for_the_chord_case_inp_none():
    starting = {"trigger": "fire_once", "type": "macro", "macro_id": "m1"}
    draft = BindingDraft.from_wire(starting, inp=None, profile="Default")
    assert draft.kind == "macro"
    assert draft.keypress == {"key": "KEY_A", "modifiers": []}


def test_from_wire_default_seeds_profile_switch_target_to_the_current_profile():
    draft = BindingDraft.from_wire(_keypress(), inp="grid_r1c1", profile="Gaming")
    assert draft.profile_switch == {"target": "Gaming"}


# --- from_wire: starting=None (a fresh, unbound Binding) ---
#
# Ticket 29: the single seed every "what does an unbound Binding look like"
# call site now asks for — the dual-stage panel's synthetic primary stage and
# fresh deep stage, the plain editor's fresh-binding fallback, and the Chord
# dialog's fresh-binding fallback.


def test_from_wire_with_no_starting_seeds_a_fresh_keypress_on_a_grid_key():
    # grid_r1c1's passthrough default is KEY_1 (see inputs.INPUT_DEFAULT_KEY_CODE)
    # — not a hardcoded "KEY_A".
    draft = BindingDraft.from_wire(None, inp="grid_r1c1", profile="Default")
    assert draft.kind == "keypress"
    assert draft.trigger == "hold_to_repeat"
    assert draft.keypress == {"key": "KEY_1", "modifiers": []}


def test_from_wire_with_no_starting_falls_back_to_key_a_for_the_scroll_wheel():
    # wheel_scroll_up has no discrete default keycode, and is Fire-once
    # rather than Hold-to-repeat (`default_trigger_for`).
    draft = BindingDraft.from_wire(None, inp="wheel_scroll_up", profile="Default")
    assert draft.trigger == "fire_once"
    assert draft.keypress == {"key": "KEY_A", "modifiers": []}


def test_from_wire_with_no_starting_falls_back_to_key_a_for_the_chord_case():
    # A Chord's own Binding has no single Input to take a passthrough
    # default from.
    draft = BindingDraft.from_wire(None, inp=None, profile="Default")
    assert draft.kind == "keypress"
    assert draft.trigger == "hold_to_repeat"
    assert draft.keypress == {"key": "KEY_A", "modifiers": []}


def test_from_wire_with_no_starting_seeds_every_inactive_kind_too():
    draft = BindingDraft.from_wire(None, inp="grid_r1c1", profile="Gaming")
    assert draft.macro == {"macro_id": None}
    assert draft.step == {"stepper_id": None, "direction": "forward"}
    assert draft.profile_switch == {"target": "Gaming"}
    assert draft.controller_button == {"button": "BTN_SOUTH"}
    assert draft.axis == {"target": None}


# --- to_wire: per-kind wire shape, byte-for-byte ---


def test_to_wire_keypress():
    draft = BindingDraft.from_wire(
        _keypress(trigger="toggle", key="KEY_Q", modifiers=["ctrl", "shift"]),
        inp="grid_r1c1",
        profile="Default",
    )
    assert draft.to_wire() == {
        "trigger": "toggle",
        "type": "keypress",
        "key": "KEY_Q",
        "modifiers": ["ctrl", "shift"],
    }


def test_to_wire_macro():
    starting = {"trigger": "hold_to_repeat", "type": "macro", "macro_id": "m1"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert draft.to_wire() == {"trigger": "hold_to_repeat", "type": "macro", "macro_id": "m1"}


def test_to_wire_step():
    starting = {"trigger": "hold_to_repeat", "type": "step", "stepper_id": "s1", "direction": "forward"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert draft.to_wire() == {
        "trigger": "hold_to_repeat",
        "type": "step",
        "stepper_id": "s1",
        "direction": "forward",
    }


def test_to_wire_controller_button():
    starting = {"trigger": "toggle", "type": "controller_button", "button": "BTN_SOUTH"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert draft.to_wire() == {"trigger": "toggle", "type": "controller_button", "button": "BTN_SOUTH"}


def test_to_wire_profile_switch_is_always_fire_once_regardless_of_trigger():
    starting = {"trigger": "toggle", "type": "profile_switch", "target": "Gaming"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert draft.to_wire() == {"trigger": "fire_once", "type": "profile_switch", "target": "Gaming"}


def test_to_wire_axis_has_no_trigger_key_at_all():
    starting = {"trigger": "fire_once", "type": "axis", "target": "left_trigger"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert draft.to_wire() == {"type": "axis", "target": "left_trigger"}


# --- is_valid transitions ---


def test_keypress_controller_button_and_profile_switch_are_always_valid():
    for starting in (
        _keypress(),
        {"trigger": "toggle", "type": "controller_button", "button": "BTN_SOUTH"},
        {"trigger": "fire_once", "type": "profile_switch", "target": "Default"},
    ):
        draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
        assert draft.is_valid()


def test_macro_is_invalid_with_no_macro_id_and_valid_once_one_is_set():
    starting = {"trigger": "fire_once", "type": "macro", "macro_id": None}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert not draft.is_valid()
    draft.set_macro_id("m1")
    assert draft.is_valid()


def test_step_is_invalid_with_no_stepper_id_and_valid_once_one_is_set():
    starting = {"trigger": "fire_once", "type": "step", "stepper_id": None, "direction": "forward"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert not draft.is_valid()
    draft.set_stepper("s1", "forward")
    assert draft.is_valid()


def test_axis_is_invalid_with_no_target_and_valid_once_one_is_set():
    starting = {"trigger": "fire_once", "type": "axis", "target": None}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    assert not draft.is_valid()
    draft.set_axis_target("left_trigger")
    assert draft.is_valid()


def test_is_valid_tracks_a_kind_change_via_set_kind():
    draft = BindingDraft.from_wire(_keypress(), inp="grid_r1c1", profile="Default")
    assert draft.is_valid()
    draft.set_kind("macro")
    assert not draft.is_valid()
    draft.set_macro_id("m1")
    assert draft.is_valid()
    draft.set_kind("keypress")
    assert draft.is_valid()


# --- every setter ---


def test_set_trigger():
    draft = BindingDraft.from_wire(_keypress(trigger="fire_once"), inp="grid_r1c1", profile="Default")
    draft.set_trigger("toggle")
    assert draft.trigger == "toggle"
    assert draft.to_wire()["trigger"] == "toggle"


def test_set_keypress_key():
    draft = BindingDraft.from_wire(_keypress(), inp="grid_r1c1", profile="Default")
    draft.set_keypress_key("KEY_Z")
    assert draft.keypress["key"] == "KEY_Z"


def test_set_keypress_modifiers_sorts_the_stored_list():
    draft = BindingDraft.from_wire(_keypress(), inp="grid_r1c1", profile="Default")
    draft.set_keypress_modifiers({"shift", "ctrl"})
    assert draft.keypress["modifiers"] == ["ctrl", "shift"]


def test_set_macro_id():
    starting = {"trigger": "fire_once", "type": "macro", "macro_id": None}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    draft.set_macro_id("m2")
    assert draft.macro == {"macro_id": "m2"}


def test_set_stepper_sets_both_fields_together():
    starting = {"trigger": "fire_once", "type": "step", "stepper_id": None, "direction": "forward"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    draft.set_stepper("s1", "backward")
    assert draft.step == {"stepper_id": "s1", "direction": "backward"}


def test_set_profile_switch_target():
    starting = {"trigger": "fire_once", "type": "profile_switch", "target": "Default"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    draft.set_profile_switch_target("Gaming")
    assert draft.profile_switch == {"target": "Gaming"}


def test_set_controller_button():
    starting = {"trigger": "toggle", "type": "controller_button", "button": "BTN_SOUTH"}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    draft.set_controller_button("BTN_EAST")
    assert draft.controller_button == {"button": "BTN_EAST"}


def test_set_axis_target():
    starting = {"trigger": "fire_once", "type": "axis", "target": None}
    draft = BindingDraft.from_wire(starting, inp="grid_r1c1", profile="Default")
    draft.set_axis_target("left_trigger")
    assert draft.axis == {"target": "left_trigger"}


def test_set_kind():
    draft = BindingDraft.from_wire(_keypress(), inp="grid_r1c1", profile="Default")
    draft.set_kind("macro")
    assert draft.kind == "macro"


# --- on_change (post-release ticket 31) ---


def test_on_change_defaults_to_a_noop():
    # Every setter must be callable with the default hook in place — this is
    # exactly what every caller except `build_action_and_trigger_fields` does.
    draft = BindingDraft.from_wire(_keypress(), inp="grid_r1c1", profile="Default")
    draft.set_keypress_key("KEY_Z")
    assert draft.keypress["key"] == "KEY_Z"


def test_on_change_fires_at_the_end_of_every_setter():
    draft = BindingDraft.from_wire(_keypress(), inp="grid_r1c1", profile="Default")
    calls = []
    draft.on_change = lambda: calls.append(1)

    draft.set_kind("macro")
    draft.set_trigger("toggle")
    draft.set_keypress_key("KEY_Z")
    draft.set_keypress_modifiers({"ctrl"})
    draft.set_macro_id("m1")
    draft.set_stepper("s1", "backward")
    draft.set_profile_switch_target("Gaming")
    draft.set_controller_button("BTN_EAST")
    draft.set_axis_target("left_trigger")

    assert len(calls) == 9


def test_on_change_sees_the_field_already_updated():
    # The hook fires *after* the field write, not before — a caller that
    # reads the draft from inside it (e.g. `save_btn.set_sensitive(draft.
    # is_valid())`) must see the new value.
    draft = BindingDraft.from_wire(
        {"trigger": "fire_once", "type": "macro", "macro_id": None}, inp="grid_r1c1", profile="Default"
    )
    seen = []
    draft.on_change = lambda: seen.append(draft.is_valid())

    draft.set_macro_id("m1")

    assert seen == [True]
