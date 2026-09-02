# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Behaviour-focused unit tests for `acheron_gui.rules` — the pure mirror of
`daemon/src/config.rs`'s device catalogs and Binding-legality matrix.

`test_rules_contract.py` proves these agree with the Daemon's checked-in
fixture row for row; this file pins the *shape* of each rule so a regression
reads as a named failure, and is where the pure-rule assertions that used to
live in `test_daemon_stub.py` (testing the fake against itself) now belong.
"""

from __future__ import annotations

import pytest

from acheron_gui import rules

# --- catalogs --------------------------------------------------------------


def test_gamepad_button_catalog_is_the_curated_57_entry_allowlist():
    assert len(rules.GAMEPAD_BUTTONS) == 57
    assert {"BTN_SOUTH", "BTN_DPAD_UP", "BTN_TRIGGER_HAPPY1", "BTN_TRIGGER_HAPPY40"} <= rules.GAMEPAD_BUTTONS
    assert "KEY_A" not in rules.GAMEPAD_BUTTONS
    assert "BTN_LEFT" not in rules.GAMEPAD_BUTTONS  # a mouse button, not a gamepad one
    assert "BTN_TRIGGER_HAPPY41" not in rules.GAMEPAD_BUTTONS


def test_axis_target_catalog_is_the_17_wire_strings():
    assert len(rules.AXIS_TARGETS) == 17
    assert {"left_trigger", "left_stick_x_pos", "left_stick_x_neg", "wheel_neg"} <= rules.AXIS_TARGETS
    assert "left_stick_x" not in rules.AXIS_TARGETS  # signed axes are split into halves


# --- valid_action_kinds --------------------------------------------------------


def test_axis_is_offered_only_on_a_grid_input():
    assert "axis" in rules.valid_action_kinds("grid_r1c1")
    assert "axis" not in rules.valid_action_kinds("mode_key")
    assert "axis" not in rules.valid_action_kinds("thumbstick_up")
    assert "axis" not in rules.valid_action_kinds(None)


def test_profile_switch_is_offered_everywhere_except_a_chords_own_binding():
    assert "profile_switch" in rules.valid_action_kinds("grid_r1c1")
    assert "profile_switch" in rules.valid_action_kinds("mode_key")
    assert "profile_switch" not in rules.valid_action_kinds(None)


def test_the_library_and_device_event_kinds_are_always_offered():
    for inp in ("grid_r1c1", "mode_key", "wheel_scroll_up", None):
        assert {"keypress", "controller_button", "macro", "step"} <= rules.valid_action_kinds(inp)


# --- valid_triggers ----------------------------------------------------------


def test_axis_has_no_trigger_mode_at_all():
    assert rules.valid_triggers("axis", "grid_r1c1") == frozenset()


def test_profile_switch_is_locked_to_fire_once():
    assert rules.valid_triggers("profile_switch", "grid_r1c1") == frozenset({"fire_once"})
    # …and has nowhere to run from a Chord, so no trigger is valid there.
    assert rules.valid_triggers("profile_switch", None) == frozenset()


def test_controller_button_excludes_fire_once():
    assert "fire_once" not in rules.valid_triggers("controller_button", "grid_r1c1")
    assert "hold_to_repeat" in rules.valid_triggers("controller_button", "grid_r1c1")
    assert "fire_once" not in rules.valid_triggers("controller_button", None)


def test_step_excludes_toggle():
    assert "toggle" not in rules.valid_triggers("step", "grid_r1c1")
    assert {"fire_once", "hold_to_repeat"} <= rules.valid_triggers("step", "grid_r1c1")


def test_analog_repeat_is_grid_key_only_and_never_on_a_chord():
    assert "analog_repeat" in rules.valid_triggers("keypress", "grid_r1c1")
    assert "analog_repeat" not in rules.valid_triggers("keypress", "mode_key")
    assert "analog_repeat" not in rules.valid_triggers("keypress", "wheel_scroll_up")
    assert "analog_repeat" not in rules.valid_triggers("keypress", None)


def test_keypress_and_macro_allow_every_trigger_on_a_grid_key():
    for kind in ("keypress", "macro"):
        assert rules.valid_triggers(kind, "grid_r1c1") == rules.ALL_TRIGGERS


def test_a_chord_binding_allows_the_three_non_analog_triggers_for_a_keypress():
    assert rules.valid_triggers("keypress", None) == frozenset(
        {"fire_once", "hold_to_repeat", "toggle"}
    )


# --- slug (mirrors config::slug_base) ---------------------------------------


@pytest.mark.parametrize(
    ("name", "fallback", "expected"),
    [
        ("Screenshot Combo", "macro", "screenshot-combo"),
        ("Weapon Wheel", "stepper", "weapon-wheel"),
        ("  padded  ", "macro", "padded"),
        ("runs___of###punctuation", "macro", "runs-of-punctuation"),
        ("--trim--", "macro", "trim"),
        ("!!!", "macro", "macro"),
        ("", "stepper", "stepper"),
        ("Café", "macro", "caf"),
        ("keep123digits", "macro", "keep123digits"),
    ],
)
def test_slug_matches_the_daemon_transform(name, fallback, expected):
    assert rules.slug(name, fallback) == expected


def test_slug_is_a_pure_derivation_the_collision_suffix_lives_in_the_caller():
    # `slug` only ever produces the *base* — `daemon_stub._unique_macro_id`
    # (and the real Daemon's `unique_macro_id`) append `-2`, `-3`, … on top.
    assert rules.slug("Screenshot Combo", "macro") == "screenshot-combo"


# --- input_sort_key / chord_key (mirror Input's derived Ord + ChordKey) ------


def test_chord_key_orders_members_by_inputs_own_ord_not_alphabetically():
    # ModeKey < Grid < Thumbstick < Wheel — "grid_r1c1" < "mode_key"
    # alphabetically, but ModeKey sorts first under the real derived Ord.
    assert rules.chord_key(["grid_r1c1", "mode_key"]) == "mode_key+grid_r1c1"
    assert rules.chord_key(["grid_r1c2", "grid_r1c1"]) == "grid_r1c1+grid_r1c2"
    assert rules.chord_key(["wheel_middle", "thumbstick_up", "mode_key"]) == (
        "mode_key+thumbstick_up+wheel_middle"
    )
    assert rules.chord_key(["grid_r2c1", "grid_r1c5"]) == "grid_r1c5+grid_r2c1"


def test_input_sort_key_ranks_the_four_variant_kinds_in_declaration_order():
    keys = [
        rules.input_sort_key("mode_key"),
        rules.input_sort_key("grid_r1c1"),
        rules.input_sort_key("thumbstick_up"),
        rules.input_sort_key("wheel_scroll_up"),
    ]
    assert keys == sorted(keys)


# --- chord_members_conflict (mirrors the subset/superset rule) ---------------


def test_chord_members_conflict_is_true_only_for_subset_superset():
    a = {"grid_r1c1", "grid_r1c2"}
    superset = {"grid_r1c1", "grid_r1c2", "mode_key"}
    intersecting = {"grid_r1c1", "mode_key"}
    disjoint = {"grid_r2c1", "grid_r2c2"}

    assert rules.chord_members_conflict(a, superset) is True
    assert rules.chord_members_conflict(superset, a) is True
    assert rules.chord_members_conflict(a, a) is True
    assert rules.chord_members_conflict(a, intersecting) is False
    assert rules.chord_members_conflict(a, disjoint) is False
