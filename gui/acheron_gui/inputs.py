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
    ("fire_once", "Fire-once"),
    ("hold_to_repeat", "Hold-to-repeat"),
    ("toggle", "Toggle"),
]
TRIGGER_SHORT = {"fire_once": "1x", "hold_to_repeat": "hold", "toggle": "toggle"}
ACTION_TYPES = [
    ("keypress", "Keypress"),
    ("macro", "Macro"),
    ("step", "Stepper"),
    ("profile_switch", "Profile Switch"),
    ("controller_button", "Controller Button"),
]
