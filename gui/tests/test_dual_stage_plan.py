# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

from acheron_gui.binding_draft import BindingDraft
from acheron_gui.dual_stage_plan import DualStagePlan


def _keypress(key="KEY_F1", trigger="hold_to_repeat", modifiers=None):
    return {"trigger": trigger, "type": "keypress", "key": key, "modifiers": modifiers or []}


def _draft(wire, *, inp="grid_r1c1", profile="Default"):
    return BindingDraft.from_wire(wire, inp=inp, profile=profile)


# --- diff(): what needs pushing, and in what order ---


def test_diff_pushes_both_stages_primary_before_deep_when_both_drafts_differ():
    plan = DualStagePlan()
    plan.capture("primary", _draft(_keypress("KEY_F2")))
    plan.capture("deep", _draft(_keypress("KEY_F1")))

    steps = plan.diff(_keypress("KEY_A"), _keypress("KEY_A"))

    assert [stage for stage, _ in steps] == ["primary", "deep"]
    assert steps[0][1]["key"] == "KEY_F2"
    assert steps[1][1]["key"] == "KEY_F1"


def test_diff_only_includes_the_stage_whose_draft_actually_differs():
    plan = DualStagePlan()
    plan.capture("primary", _draft(_keypress("KEY_F2")))
    plan.capture("deep", _draft(_keypress("KEY_A")))  # unchanged from the snapshot

    steps = plan.diff(_keypress("KEY_A"), _keypress("KEY_A"))

    assert [stage for stage, _ in steps] == ["primary"]


def test_diff_returns_nothing_when_no_drafts_were_captured():
    plan = DualStagePlan()

    assert plan.diff(_keypress("KEY_A"), _keypress("KEY_A")) == []


def test_diff_pushes_an_unbound_primary_unconditionally():
    plan = DualStagePlan()
    # The field editor always mounts *some* draft, even an untouched one
    # matching the caller-resolved synthetic default — an unbound primary is
    # still pushed the first time regardless.
    plan.capture("primary", _draft(_keypress("KEY_A")))

    steps = plan.diff(None, None)

    assert [stage for stage, _ in steps] == ["primary"]


def test_diff_never_pushes_a_deep_stage_that_does_not_exist_yet():
    plan = DualStagePlan()
    plan.capture("deep", _draft(_keypress("KEY_F1")))

    steps = plan.diff(_keypress("KEY_A"), None)

    assert steps == []


# --- stage_starting(): what a stage's field editor should seed from ---


def test_stage_starting_returns_the_snapshot_when_no_draft_is_captured():
    plan = DualStagePlan()

    assert plan.stage_starting("primary", _keypress("KEY_A")) == _keypress("KEY_A")
    assert plan.stage_starting("deep", None) is None


def test_stage_starting_round_trips_a_captured_draft_including_axis_with_no_trigger_key():
    # Ticket 28's concern, at the plan level: an unsaved Axis edit on the
    # primary must round-trip through a stage swap without losing its shape
    # (no "trigger" key) or crashing the caller that feeds it back into
    # `BindingDraft.from_wire`.
    plan = DualStagePlan()
    axis_draft = _draft(None)
    axis_draft.set_kind("axis")
    axis_draft.set_axis_target("left_trigger")
    plan.capture("primary", axis_draft)

    seeded = plan.stage_starting("primary", _keypress("KEY_A"))

    assert seeded == {"type": "axis", "target": "left_trigger"}
    assert "trigger" not in seeded


# --- mark_committed(): clearing a draft after it lands (or goes stale) ---


def test_mark_committed_clears_the_draft_so_stage_starting_falls_back_to_the_snapshot_again():
    plan = DualStagePlan()
    plan.capture("primary", _draft(_keypress("KEY_F2")))

    plan.mark_committed("primary")

    assert plan.stage_starting("primary", _keypress("KEY_A")) == _keypress("KEY_A")
    assert plan.diff(_keypress("KEY_A"), None) == []
