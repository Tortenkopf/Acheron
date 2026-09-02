# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""The acknowledged Python mirror of the Daemon's *pure* domain core — the
device vocabularies (`input::gamepad_button_codes`, `AxisTarget::ALL`) and
the parts of `daemon/src/config.rs::validate` that are a function of
`(vocabulary + one Binding/Chord + its Input)` and nothing else.

**Why a mirror and not a shared schema.** ADR 0003 settled the split-language
stack: a Rust Daemon and this Python + GTK GUI, talking over a D-Bus process
seam. Nothing crosses that seam as types — the model is necessarily
re-expressed on each side. This module is where the GUI's copy of the pure
half lives, in one place instead of scattered across `daemon_stub.py`,
`binding_editor.py`, and the pickers.

**Why it can be trusted.** `daemon/src/schema.rs` emits the Daemon's real
catalogs and `config::validate` verdicts as a checked-in golden fixture,
`daemon/contract/daemon-schema.json`, and `gui/tests/test_rules_contract.py`
asserts every symbol here agrees with it exactly. When a device-catalog
entry or a Binding-legality rule changes on the Daemon, regenerate the
fixture (`ACHERON_BLESS=1 cargo test`) and update this file to match — see
`CONTRIBUTING.md`.

Every symbol here is **pure**: it returns data, never raises, and never
touches a `Config` or any stub state. Whole-`Config` checks (dangling
`macro_id`/`stepper_id`/`profile_switch` targets, reference counts, the
stepper-steal, `release < actuation`, the axis↔binding↔chord
mutual-exclusion clear) stay in `daemon_stub.py` as operation logic over its
own in-memory state — that split is deliberate and is not a deepening target
(ADR 0005). Keycodes are out — `Action::Keypress.key` accepts any `KeyCode`
by design, so there is no Daemon SSOT to contract-test.

The one dependency is one-way, `rules` → `inputs` (`is_grid_input`, the grid
enumeration).
"""

from __future__ import annotations

from typing import Iterable

from .inputs import is_grid_input

# ---------------------------------------------------------------------
# Device catalogs
# ---------------------------------------------------------------------

# Mirrors `daemon/src/input.rs::gamepad_button_codes()` (ticket 43's curated
# 57-entry gamepad allowlist): the named `BTN_GAMEPAD` range, the four
# `BTN_DPAD_*`, and the full `BTN_TRIGGER_HAPPY1`-`40` extra range. Each name
# is the `KeyCode`'s `{:?}` form, which is also its D-Bus wire string
# (`dbus/wire.rs::key_to_string`).
_NAMED_GAMEPAD_BUTTONS = (
    "BTN_SOUTH",
    "BTN_EAST",
    "BTN_NORTH",
    "BTN_WEST",
    "BTN_TL",
    "BTN_TR",
    "BTN_TL2",
    "BTN_TR2",
    "BTN_SELECT",
    "BTN_START",
    "BTN_MODE",
    "BTN_THUMBL",
    "BTN_THUMBR",
    "BTN_DPAD_UP",
    "BTN_DPAD_DOWN",
    "BTN_DPAD_LEFT",
    "BTN_DPAD_RIGHT",
)

GAMEPAD_BUTTONS: frozenset[str] = frozenset(_NAMED_GAMEPAD_BUTTONS) | frozenset(
    f"BTN_TRIGGER_HAPPY{i}" for i in range(1, 41)
)

# Mirrors `AxisTarget::ALL` via `dbus/wire.rs::axis_target_str` (ticket 59
# §3): 5 unsigned single-key axes + 6 signed axes split into independently-
# assignable +/- halves (12 half-axis targets).
AXIS_TARGETS: frozenset[str] = frozenset(
    {
        "left_trigger",
        "right_trigger",
        "throttle",
        "gas",
        "brake",
        "left_stick_x_pos",
        "left_stick_x_neg",
        "left_stick_y_pos",
        "left_stick_y_neg",
        "right_stick_x_pos",
        "right_stick_x_neg",
        "right_stick_y_pos",
        "right_stick_y_neg",
        "rudder_pos",
        "rudder_neg",
        "wheel_pos",
        "wheel_neg",
    }
)

# ---------------------------------------------------------------------
# Trigger-mode and Action-kind vocabularies
# ---------------------------------------------------------------------

# Mirrors `daemon/src/config.rs::TriggerMode` (`#[serde(rename_all =
# "snake_case")]`).
ALL_TRIGGERS: frozenset[str] = frozenset(
    {"fire_once", "hold_to_repeat", "toggle", "analog_repeat"}
)

# Every Action-*placement* kind: the five `Action` variants plus `axis`
# (Axis assignment — a parallel concept, not an `Action` variant, but the
# GUI's Action dropdown offers it in the same list — `inputs.ACTION_TYPES`).
ALL_ACTION_KINDS: frozenset[str] = frozenset(
    {"keypress", "controller_button", "axis", "macro", "step", "profile_switch"}
)


def valid_action_kinds(input_str: str | None) -> frozenset[str]:
    """Which Action kinds are structurally legal on `input_str` in isolation,
    mirroring the Action/Axis-placement parts of `config::validate`:

    - `axis` only on a Grid Input (only Grid keys have Depth — ticket 59 §1);
    - `profile_switch` never on a Chord's own Binding
      (`ConfigError::InvalidChordProfileSwitch`).

    `input_str is None` means a Chord's own Binding.
    """
    kinds = set(ALL_ACTION_KINDS)
    if input_str is None:
        kinds -= {"axis", "profile_switch"}
    elif not is_grid_input(input_str):
        kinds -= {"axis"}
    return frozenset(kinds)


def valid_triggers(action_kind: str, input_str: str | None) -> frozenset[str]:
    """Which `TriggerMode`s are legal for `action_kind` on `input_str`,
    mirroring `config::validate`'s `TriggerMode` matrix:

    - an Axis assignment has no `TriggerMode` at all → always `frozenset()`
      (`binding_editor` reads that as "disable the Trigger-mode dropdown");
    - an Action kind that isn't itself legal here → `frozenset()`;
    - `profile_switch` → `{fire_once}` (`InvalidProfileSwitchTrigger`);
    - `controller_button` excludes `fire_once` (`InvalidControllerButtonTrigger`);
    - `step` excludes `toggle` (`InvalidStepTrigger`);
    - `analog_repeat` only on a Grid Input (`InvalidAnalogRepeatInput` /
      `InvalidChordAnalogRepeat`).

    `input_str is None` means a Chord's own Binding.
    """
    if action_kind == "axis":
        return frozenset()
    if action_kind not in valid_action_kinds(input_str):
        return frozenset()

    triggers = set(ALL_TRIGGERS)
    if action_kind == "profile_switch":
        triggers &= {"fire_once"}
    if action_kind == "controller_button":
        triggers -= {"fire_once"}
    if action_kind == "step":
        triggers -= {"toggle"}
    if input_str is None or not is_grid_input(input_str):
        triggers -= {"analog_repeat"}
    return frozenset(triggers)


# ---------------------------------------------------------------------
# Slug / Input-ordering / Chord-key transforms
# ---------------------------------------------------------------------


def slug(name: str, fallback: str) -> str:
    """Mirrors `daemon/src/config.rs::slug_base` char for char: every ASCII
    alphanumeric is kept (lowercased), every run of anything else collapses
    to one `-`, leading/trailing `-` are trimmed, and an empty result falls
    back to `fallback`.

    Ported as a character loop rather than a regex on purpose — a regex over
    `name.lower()` would diverge from the Rust `is_ascii_alphanumeric` gate
    on non-ASCII characters that Python's full-Unicode `str.lower()` maps
    into the ASCII range.
    """
    out: list[str] = []
    last_was_hyphen = True  # suppresses a leading hyphen
    for ch in name:
        if ch.isascii() and ch.isalnum():
            out.append(ch.lower())
            last_was_hyphen = False
        elif not last_was_hyphen:
            out.append("-")
            last_was_hyphen = True
    result = "".join(out).rstrip("-")
    return result or fallback


def input_sort_key(input_str: str) -> tuple:
    """Mirrors `daemon/src/input.rs::Input`'s *derived* `Ord`:
    `ModeKey < Grid(row, col) < Thumbstick(Direction) < Wheel(WheelEvent)`,
    each variant's fields compared in declaration order. A plain alphabetical
    sort disagrees for any set mixing Input variant kinds (e.g.
    `{mode_key, grid_r1c1}` → `mode_key` first, not `grid_r1c1`).
    """
    if input_str == "mode_key":
        return (0,)
    if input_str.startswith("grid_r"):
        rest = input_str[len("grid_r") :]
        row_str, _, col_str = rest.partition("c")
        return (1, int(row_str), int(col_str))
    direction_order = {
        "thumbstick_up": 0,
        "thumbstick_down": 1,
        "thumbstick_left": 2,
        "thumbstick_right": 3,
    }
    if input_str in direction_order:
        return (2, direction_order[input_str])
    wheel_order = {"wheel_scroll_up": 0, "wheel_scroll_down": 1, "wheel_middle": 2}
    return (3, wheel_order[input_str])


def chord_key(inputs: Iterable[str]) -> str:
    """Mirrors `daemon/src/config.rs::ChordKey`'s `Display`: a `+`-joined
    string of member Input strings ordered by `Input`'s own `Ord`
    (`input_sort_key`), not alphabetically."""
    return "+".join(sorted(inputs, key=input_sort_key))


def chord_members_conflict(a: set[str], b: set[str]) -> bool:
    """`True` iff one member set fully contains the other — ticket 01's
    amended subset/superset rule (`ConfigError::ChordMemberSetConflict`). A
    plain intersection (the thumbstick-diagonal shape) is not a conflict.
    Equal sets count as containing each other; a caller comparing a Chord
    against the live map skips the identical key itself.
    """
    return a <= b or b <= a
