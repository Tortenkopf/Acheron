# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""`inputs.py` owns all *presentation* (labels, menu ordering, `TRIGGER_SHORT`,
`default_trigger_for`); `rules.py` owns the label-free predicate sets. These
tests pin the one place the two must agree — the option-*key* sets — so a new
Trigger mode or Action kind can't be added to one without the other.
"""

from __future__ import annotations

from acheron_gui import rules
from acheron_gui.inputs import (
    ACTION_TYPES,
    ALL_INPUTS,
    INPUT_DEFAULT_KEY_CODE,
    TRIGGER_OPTIONS,
    default_key_code_for,
)
from acheron_gui.key_picker import LABEL_BY_CODE


def test_trigger_option_keys_match_rules_all_triggers():
    assert {k for k, _ in TRIGGER_OPTIONS} == rules.ALL_TRIGGERS


def test_action_type_keys_match_rules_all_action_kinds():
    assert {k for k, _ in ACTION_TYPES} == rules.ALL_ACTION_KINDS


def test_default_key_code_for_covers_every_input_with_a_pickable_code():
    # The editor seeds an unbound Keypress field with this — every value must
    # be a code the key picker actually renders a keycap for, or the "current
    # pick" highlight lands nowhere.
    for inp in ALL_INPUTS:
        assert default_key_code_for(inp) in LABEL_BY_CODE
    # The two scroll-wheel directions inject as EV_REL scroll with no
    # discrete keycode (daemon `key_code_for_input` → None) — fall back to
    # KEY_A, as does a Chord's own Binding (`inp is None`).
    assert default_key_code_for("wheel_scroll_up") == "KEY_A"
    assert default_key_code_for("wheel_scroll_down") == "KEY_A"
    assert default_key_code_for(None) == "KEY_A"


def test_input_default_key_codes_mirror_the_daemon_layout():
    # Spot-check against daemon/src/input.rs `GRID_KEYS` / `key_code_for_input`.
    assert INPUT_DEFAULT_KEY_CODE["grid_r1c1"] == "KEY_1"
    assert INPUT_DEFAULT_KEY_CODE["grid_r3c2"] == "KEY_A"
    assert INPUT_DEFAULT_KEY_CODE["grid_r4c1"] == "KEY_LEFTSHIFT"
    assert INPUT_DEFAULT_KEY_CODE["grid_r4c5"] == "KEY_SPACE"
    assert INPUT_DEFAULT_KEY_CODE["mode_key"] == "KEY_LEFTALT"
    assert INPUT_DEFAULT_KEY_CODE["thumbstick_up"] == "KEY_UP"
    assert INPUT_DEFAULT_KEY_CODE["wheel_middle"] == "BTN_MIDDLE"
    # scroll directions deliberately absent
    assert "wheel_scroll_up" not in INPUT_DEFAULT_KEY_CODE
