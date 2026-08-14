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


TRIGGER_OPTIONS = [
    ("fire_once", "Fire-once"),
    ("hold_to_repeat", "Hold-to-repeat"),
    ("toggle", "Toggle"),
]
TRIGGER_SHORT = {"fire_once": "1x", "hold_to_repeat": "hold", "toggle": "toggle"}
ACTION_TYPES = [("keypress", "Keypress"), ("macro", "Macro")]
