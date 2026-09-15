# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

import pytest

from acheron_gui import wire


def test_keypress_binding_round_trips_through_a_variant():
    binding = {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": ["ctrl", "shift"]}

    variant_dict = wire.binding_to_variant(binding)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == {"trigger": "fire_once", "type": "keypress", "key": "KEY_F1", "modifiers": ["ctrl", "shift"]}


def test_keypress_with_no_modifiers_omits_the_modifiers_field():
    binding = {"trigger": "fire_once", "type": "keypress", "key": "KEY_A", "modifiers": []}

    variant_dict = wire.binding_to_variant(binding)

    assert "modifiers" not in variant_dict


def test_profile_switch_binding_round_trips_through_a_variant():
    binding = {"trigger": "fire_once", "type": "profile_switch", "target": "Gaming"}

    variant_dict = wire.binding_to_variant(binding)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == binding


def test_controller_button_binding_round_trips_through_a_variant():
    binding = {"trigger": "fire_once", "type": "controller_button", "button": "BTN_SOUTH"}

    variant_dict = wire.binding_to_variant(binding)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == binding


def test_macro_binding_round_trips_through_a_variant():
    binding = {"trigger": "toggle", "type": "macro", "macro_id": "screenshot-combo"}

    variant_dict = wire.binding_to_variant(binding)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == binding


def test_macro_step_to_variant_round_trips_every_step_kind():
    steps = [
        {"type": "key_down", "key": "KEY_A"},
        {"type": "delay_ms", "ms": 50},
        {"type": "key_up", "key": "KEY_A"},
    ]

    for step in steps:
        variant_dict = wire.macro_step_to_variant(step)
        unpacked = {k: v.unpack() for k, v in variant_dict.items()}
        assert unpacked == step


def test_step_binding_round_trips_through_a_variant():
    binding = {"trigger": "fire_once", "type": "step", "stepper_id": "weapon-wheel", "direction": "forward"}

    variant_dict = wire.binding_to_variant(binding)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == binding


def test_stepper_item_to_variant_round_trips():
    item = {"type": "key", "key": "KEY_A"}

    variant_dict = wire.stepper_item_to_variant(item)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == item


def test_stepper_item_with_modifiers_round_trips_through_a_variant():
    item = {"type": "key", "key": "KEY_3", "modifiers": ["ctrl", "shift"]}

    variant_dict = wire.stepper_item_to_variant(item)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == item


def test_stepper_item_with_no_modifiers_omits_the_modifiers_field():
    item = {"type": "key", "key": "KEY_A", "modifiers": []}

    variant_dict = wire.stepper_item_to_variant(item)

    assert "modifiers" not in variant_dict


def test_controller_button_stepper_item_round_trips_through_a_variant():
    item = {"type": "controller_button", "button": "BTN_SOUTH"}

    variant_dict = wire.stepper_item_to_variant(item)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == item
    assert "modifiers" not in variant_dict


# --- `tartarus-backlight` ticket 03: `lighting_assignment_to_variant` ------
#
# `Colour` rides as a `(yyy)` byte-triple on this wire encoding (matching the
# daemon's `colour_from_field` decode convention), not the `{"r","g","b"}`
# dict `GetConfig()` hands back — these tests unpack through that boundary
# rather than asserting dict equality against the input.


def test_off_lighting_assignment_round_trips_through_a_variant():
    variant_dict = wire.lighting_assignment_to_variant({"type": "off"})

    assert {k: v.unpack() for k, v in variant_dict.items()} == {"type": "off"}


def test_fixed_effect_reactive_lighting_assignment_encodes_its_colour_as_a_byte_triple():
    assignment = {
        "type": "fixed_effect",
        "effect": {
            "type": "reactive",
            "colour": {"r": 255, "g": 128, "b": 0},
            "speed": 3,
        },
    }

    variant_dict = wire.lighting_assignment_to_variant(assignment)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == {
        "type": "fixed_effect",
        "effect": {
            "type": "reactive",
            "colour": (255, 128, 0),
            "speed": 3,
        },
    }


def test_fixed_effect_breath_dual_lighting_assignment_encodes_both_colours():
    assignment = {
        "type": "fixed_effect",
        "effect": {
            "type": "breath",
            "style": {
                "style": "dual",
                "first": {"r": 1, "g": 2, "b": 3},
                "second": {"r": 4, "g": 5, "b": 6},
            },
        },
    }

    variant_dict = wire.lighting_assignment_to_variant(assignment)
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked == {
        "type": "fixed_effect",
        "effect": {
            "type": "breath",
            "style": {"style": "dual", "first": (1, 2, 3), "second": (4, 5, 6)},
        },
    }


def test_custom_layout_lighting_assignment_encodes_21_colours_as_an_array_of_byte_triples():
    colours = [{"r": i, "g": i, "b": i} for i in range(21)]

    variant_dict = wire.lighting_assignment_to_variant({"type": "custom_layout", "colours": colours})
    unpacked = {k: v.unpack() for k, v in variant_dict.items()}

    assert unpacked["type"] == "custom_layout"
    assert unpacked["colours"] == [(i, i, i) for i in range(21)]


def test_lighting_assignment_to_variant_rejects_an_unknown_type():
    with pytest.raises(ValueError):
        wire.lighting_assignment_to_variant({"type": "bogus"})
