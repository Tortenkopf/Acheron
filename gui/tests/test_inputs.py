# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""`inputs.py` owns all *presentation* (labels, menu ordering, `TRIGGER_SHORT`,
`default_trigger_for`); `rules.py` owns the label-free predicate sets. These
tests pin the one place the two must agree — the option-*key* sets — so a new
Trigger mode or Action kind can't be added to one without the other.
"""

from __future__ import annotations

from acheron_gui import rules
from acheron_gui.inputs import ACTION_TYPES, TRIGGER_OPTIONS


def test_trigger_option_keys_match_rules_all_triggers():
    assert {k for k, _ in TRIGGER_OPTIONS} == rules.ALL_TRIGGERS


def test_action_type_keys_match_rules_all_action_kinds():
    assert {k for k, _ in ACTION_TYPES} == rules.ALL_ACTION_KINDS
