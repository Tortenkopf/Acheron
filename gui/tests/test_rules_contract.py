# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Contract test: `acheron_gui.rules` must agree exactly with the Daemon's
checked-in `daemon/contract/daemon-schema.json`, which `daemon/src/schema.rs`
derives by driving the real `config::validate`.

If this fails, the two sides of ADR 0003's split-language seam have drifted:
regenerate the fixture with `ACHERON_BLESS=1 cargo test --manifest-path
daemon/Cargo.toml schema` and mirror the change into `acheron_gui/rules.py`
(see `CONTRIBUTING.md`), or fix whichever side is wrong.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from acheron_gui import rules

_SCHEMA_PATH = Path(__file__).resolve().parents[2] / "daemon" / "contract" / "daemon-schema.json"


@pytest.fixture(scope="module")
def schema() -> dict:
    return json.loads(_SCHEMA_PATH.read_text())


def _input(cell: str) -> str | None:
    """The fixture's `"__chord__"` sentinel is `rules`' `input_str=None`."""
    return None if cell == "__chord__" else cell


def test_gamepad_button_catalog_matches(schema: dict) -> None:
    assert rules.GAMEPAD_BUTTONS == set(schema["gamepad_buttons"])


def test_axis_target_catalog_matches(schema: dict) -> None:
    assert rules.AXIS_TARGETS == set(schema["axis_targets"])


def test_trigger_matrix_matches_row_for_row(schema: dict) -> None:
    mismatches = [
        row
        for row in schema["trigger_matrix"]
        if (row["trigger"] in rules.valid_triggers(row["action_kind"], _input(row["input"])))
        != row["allowed"]
    ]
    assert mismatches == []


def test_action_kind_matrix_matches_row_for_row(schema: dict) -> None:
    mismatches = [
        row
        for row in schema["action_kind_matrix"]
        if (row["action_kind"] in rules.valid_action_kinds(_input(row["input"]))) != row["allowed"]
    ]
    assert mismatches == []


def test_slug_examples_match(schema: dict) -> None:
    for row in schema["slug_examples"]:
        assert rules.slug(row["name"], row["fallback"]) == row["slug"], row


def test_chord_key_examples_match(schema: dict) -> None:
    for row in schema["chord_key_examples"]:
        assert rules.chord_key(row["members"]) == row["key"], row
