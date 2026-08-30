# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""The Tartarus Pro's `Input` set and display labels, mirroring
`daemon/src/input.rs`'s flat snake_case strings exactly (`mode_key`,
`grid_r{row}c{col}`, `thumbstick_up`, `wheel_scroll_up`, `wheel_middle`, …)
— these are also the wire form `SetBinding`/`ClearBinding` take and
`GetConfig()` returns as dict keys (issue 08), so no translation layer
sits between this list and the D-Bus surface.
"""

from __future__ import annotations

GRID_ROWS, GRID_COLS = 4, 5


def grid_input(row: int, col: int) -> str:
    return f"grid_r{row}c{col}"


ALL_INPUTS = (
    ["mode_key"]
    + [grid_input(r, c) for r in range(1, GRID_ROWS + 1) for c in range(1, GRID_COLS + 1)]
    + ["thumbstick_up", "thumbstick_down", "thumbstick_left", "thumbstick_right"]
    + ["wheel_scroll_up", "wheel_scroll_down", "wheel_middle"]
)

LAYOUT_NUMBER = {
    grid_input(r, c): (r - 1) * GRID_COLS + c
    for r in range(1, GRID_ROWS + 1)
    for c in range(1, GRID_COLS + 1)
}

INPUT_LABELS = {
    "mode_key": "Mode",
    # Arrow glyphs match each Input's own default/passthrough evdev keycode
    # (Thumbstick Up passes through KEY_UP, etc.) — NOT the visual position
    # it's drawn at in Device Overview's diamond, which is rotated 90°
    # clockwise from these directions (see layout.md and ticket 09).
    "thumbstick_up": "↑",
    "thumbstick_down": "↓",
    "thumbstick_left": "←",
    "thumbstick_right": "→",
    "wheel_scroll_up": "Wheel ▲",
    "wheel_scroll_down": "Wheel ▼",
    "wheel_middle": "Wheel •",
}


def input_label(inp: str) -> str:
    if inp in LAYOUT_NUMBER:
        return str(LAYOUT_NUMBER[inp])
    return INPUT_LABELS[inp]


# The default output each Input produces with no Binding set, covering every
# entry in ALL_INPUTS — mirrors `daemon/src/input.rs`'s `GRID_KEYS`/
# `key_code_for_input` table for the keyed Inputs (a fixed hardware fact with
# no D-Bus call to fetch it from, ticket 06); `wheel_scroll_up`/
# `wheel_scroll_down` have no discrete keycode there (they inject as `EV_REL`
# scroll, matching `key_code_for_input`'s own `None` for those two) so they
# name the actual behavior instead.
_GRID_DEFAULT_KEYS = [
    ["1", "2", "3", "4", "5"],
    ["Tab", "Q", "W", "E", "R"],
    ["Caps Lock", "A", "S", "D", "F"],
    ["Shift", "Z", "X", "C", "Space"],
]

INPUT_DEFAULT_LABEL = {
    grid_input(r, c): _GRID_DEFAULT_KEYS[r - 1][c - 1]
    for r in range(1, GRID_ROWS + 1)
    for c in range(1, GRID_COLS + 1)
}
INPUT_DEFAULT_LABEL.update(
    {
        "mode_key": "Alt",
        "thumbstick_up": "↑",
        "thumbstick_down": "↓",
        "thumbstick_left": "←",
        "thumbstick_right": "→",
        "wheel_middle": "Middle Click",
        "wheel_scroll_up": "Scroll",
        "wheel_scroll_down": "Scroll",
    }
)


def is_grid_input(inp: str) -> bool:
    """Only Grid keys have depth/Actuation points (ticket 17 §3/ticket 26) —
    the Mode key, thumbstick directions, and wheel events are all-or-nothing
    evdev passthrough with no analog travel to threshold."""
    return inp in LAYOUT_NUMBER


TRIGGER_OPTIONS = [
    # Ticket 89: Hold-to-repeat leads (and is the new-binding default — see
    # `default_trigger_for`), since it's the mode most bindings actually want;
    # Fire-once drops to third.
    ("hold_to_repeat", "Hold-to-repeat"),
    ("toggle", "Toggle"),
    ("fire_once", "Fire-once"),
    # Ticket 20/39: grid-key-only, since only a Grid Input has Depth — callers
    # that build a Trigger-mode dropdown for a non-grid Input, or for a
    # Chord's own Binding, must filter this entry out (mirroring
    # `ACTION_TYPES`'s own `is_grid_input`-gated "Axis" exclusion above,
    # ticket 60's Answer — `Gtk.DropDown` has no per-item sensitivity to
    # merely grey a single option, per ticket 55's precedent for the same
    # limitation).
    ("analog_repeat", "Analog-repeat"),
]
TRIGGER_SHORT = {
    "fire_once": "1x",
    "hold_to_repeat": "hold",
    "toggle": "toggle",
    "analog_repeat": "analog",
}


def default_trigger_for(inp: str | None) -> str:
    """The Trigger mode a freshly-created Binding starts on (ticket 89):
    Hold-to-repeat everywhere except the scroll wheel's two directions, which
    stay Fire-once — the wheel fires once per physical detent, so Hold-to-
    repeat there would machine-gun. `inp is None` (a Chord's own Binding, which
    has no single Input) also gets Hold-to-repeat. This is GUI-authoring-only;
    the Daemon's own `Binding.trigger` serde default is unconditionally
    Hold-to-repeat (it has no "this Input is the wheel" notion at parse time).
    """
    if inp in ("wheel_scroll_up", "wheel_scroll_down"):
        return "fire_once"
    return "hold_to_repeat"


ACTION_TYPES = [
    # Ticket 89: menu order — Keypress / Controller Button / Axis first (the
    # three "emit a device event" kinds), then Macro / Stepper (the library
    # kinds), then Switch Profile last.
    ("keypress", "Keypress"),
    ("controller_button", "Controller Button"),
    # Ticket 71: offered only when `is_grid_input(inp)` — non-grid Inputs
    # (Mode key, thumbstick, wheel) never see this option at all, rather
    # than seeing it disabled (ticket 60's Answer). Callers that build a
    # dropdown for a non-grid Input, or for a Chord's own Binding (which
    # can no more "drive an axis continuously" than it can be a Profile
    # Switch — a Chord fires on a discrete Down), must filter this entry
    # out, mirroring `binding_editor.build_chord_binding_dialog`'s existing
    # `profile_switch` exclusion.
    ("axis", "Axis"),
    ("macro", "Macro"),
    ("step", "Stepper"),
    # Ticket 89: display label "Switch Profile" (imperative, reads like the
    # menu action it is); the internal key stays "profile_switch", as do
    # `Action::ProfileSwitch`, every D-Bus method, and the config.toml tag.
    ("profile_switch", "Switch Profile"),
]
