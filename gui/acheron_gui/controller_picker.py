"""The real controller-button picker (ticket 43), for `Action::ControllerButton`
(ticket 14). Ports ticket 38's winning variant A — Gamepad Diagram — from
`prototype/38-controller-button-picker-ux` (a visual controller face for the
17 named buttons, plus a separate collapsed "Extra buttons" grid for the
`BTN_TRIGGER_HAPPY1`-`40` range), against the real curated gamepad allowlist
(`daemon/src/input.rs::gamepad_button_codes`, mirrored here) rather than the
prototype's hand-listed catalog.

Always shown inline, no outer collapse/expand toggle — ticket 38's own
prototype used a collapsed "`<button>` ▸ Change" summary that expands in
place, but ticket 44 (live-verified on real hardware, for the sibling
key/mouse-button picker mounted in this exact same per-key modal
`Gtk.Window`) found that shape broken on this GTK4/Wayland stack: a Popover's
grow-in-place resize, and a nested Popover off a MenuButton, both silently
fail to find room. This picker skips straight to that fix — always-inline,
like `key_picker.build_inline_key_picker` — rather than reproducing the same
bug in a second picker mounted in the identical container. The "Extra
buttons" section keeps its own *nested* show/hide toggle (mirrors
`key_picker`'s "Show F13-F24 ▸" sub-toggle): that only grows an
already-visible panel, not the outer collapse-to-nothing shape that broke.
"""

from __future__ import annotations

from typing import Callable

from gi.repository import Gtk

from .gtk_utils import clear_children

# ---------------------------------------------------------------------
# Catalog — ticket 14's settled device-advertising scope, mirrored from
# `daemon/src/input.rs::is_gamepad_button`/`gamepad_button_codes` (57
# entries): the named `BTN_GAMEPAD` range, `BTN_DPAD_*`, and the full
# `BTN_TRIGGER_HAPPY1`-`40` extra range.
# ---------------------------------------------------------------------

FACE = [
    ("BTN_SOUTH", "A / South"),
    ("BTN_EAST", "B / East"),
    ("BTN_NORTH", "Y / North"),
    ("BTN_WEST", "X / West"),
]
SHOULDERS = [
    ("BTN_TL", "LB (Left bumper)"),
    ("BTN_TR", "RB (Right bumper)"),
    ("BTN_TL2", "LT (Left trigger)"),
    ("BTN_TR2", "RT (Right trigger)"),
]
STICKS = [
    ("BTN_THUMBL", "L3 (Left stick click)"),
    ("BTN_THUMBR", "R3 (Right stick click)"),
]
DPAD = [
    ("BTN_DPAD_UP", "D-pad Up"),
    ("BTN_DPAD_DOWN", "D-pad Down"),
    ("BTN_DPAD_LEFT", "D-pad Left"),
    ("BTN_DPAD_RIGHT", "D-pad Right"),
]
CENTER = [
    ("BTN_SELECT", "Select / Back"),
    ("BTN_START", "Start"),
    ("BTN_MODE", "Guide / Mode"),
]
EXTRA = [(f"BTN_TRIGGER_HAPPY{i}", f"Extra {i}") for i in range(1, 41)]

GAMEPAD_CATEGORIES: dict[str, list[tuple[str, str]]] = {
    "Face buttons": FACE,
    "Shoulders & triggers": SHOULDERS,
    "Sticks": STICKS,
    "D-pad": DPAD,
    "Select / Start / Mode": CENTER,
    "Extra (Trigger-Happy 1-40)": EXTRA,
}

_ALL_ENTRIES: list[tuple[str, str]] = [
    (code, label) for entries in GAMEPAD_CATEGORIES.values() for code, label in entries
]
LABEL_BY_CODE = {code: label for code, label in _ALL_ENTRIES}

assert len(LABEL_BY_CODE) == 57, "the gamepad catalog must match the Daemon's 57-entry allowlist"


def button_css_class(code: str) -> str | None:
    if code in {c for c, _ in FACE}:
        return "padbtn-face"
    if code in {c for c, _ in SHOULDERS}:
        return "padbtn-shoulder"
    if code in {c for c, _ in STICKS}:
        return "padbtn-stick"
    if code in {c for c, _ in DPAD}:
        return "padbtn-dpad"
    return None


# ---------------------------------------------------------------------
# The gamepad diagram — (code, label-on-pad, x, y, width), hand-placed to
# approximate a real gamepad's face layout. Ported unchanged from ticket 38's
# prototype (`_PAD_LAYOUT`/`_OFFSET_Y` in
# `prototype_38_controller_button_picker_ux.py`), whose round 2 already
# live-reaction-fixed the one real layout bug found (the shoulder/trigger row
# overlapping the D-pad/Y button, R3 off-center) — nothing here changes that
# geometry, only the container it now renders inside (always-inline instead
# of a collapsed popover panel).
# ---------------------------------------------------------------------

_PAD_LAYOUT = [
    # D-pad diamond, left side
    ("BTN_DPAD_UP", "↑", 55, 35, 30),
    ("BTN_DPAD_LEFT", "←", 25, 65, 30),
    ("BTN_DPAD_RIGHT", "→", 85, 65, 30),
    ("BTN_DPAD_DOWN", "↓", 55, 95, 30),
    # Stick clicks — each centered under its diamond's bottom button
    ("BTN_THUMBL", "L3", 55, 135, 34),
    ("BTN_THUMBR", "R3", 338, 135, 34),
    # Center cluster
    ("BTN_SELECT", "Select", 150, 80, 50),
    ("BTN_START", "Start", 210, 80, 46),
    ("BTN_MODE", "Guide", 175, 40, 50),
    # ABXY diamond, right side
    ("BTN_NORTH", "Y", 340, 35, 30),
    ("BTN_WEST", "X", 310, 65, 30),
    ("BTN_EAST", "B", 370, 65, 30),
    ("BTN_SOUTH", "A", 340, 95, 30),
    # Shoulders / triggers, top strip — clear of the diamonds
    ("BTN_TL2", "LT", 5, -35, 40),
    ("BTN_TL", "LB", 5, -9, 40),
    ("BTN_TR2", "RT", 375, -35, 40),
    ("BTN_TR", "RB", 375, -9, 40),
]

# Added to every raw y above before placing on the Fixed, so the trigger
# row's -35 still lands at a non-negative screen coordinate (5px).
_OFFSET_Y = 40
_PAD_WIDTH = 440
_PAD_HEIGHT = 210


def _pad_diagram(on_pick: Callable[[str], None], current: str) -> Gtk.Widget:
    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    outer.set_margin_top(6)

    fixed = Gtk.Fixed()
    body = Gtk.Box(css_classes=["pad-body"])
    body.set_size_request(_PAD_WIDTH, _PAD_HEIGHT)
    fixed.put(body, 0, 0)

    for code, cap, x, y, w in _PAD_LAYOUT:
        btn = Gtk.Button(label=cap, css_classes=["padbtn"])
        cls = button_css_class(code)
        if cls:
            btn.add_css_class(cls)
        if code == current:
            btn.add_css_class("suggested-action")
        btn.set_size_request(w, 26)
        btn.set_tooltip_text(LABEL_BY_CODE[code])
        btn.connect("clicked", lambda b, code=code: on_pick(code))
        fixed.put(btn, x, y + _OFFSET_Y)

    outer.append(fixed)
    return outer


def _extra_grid(on_pick: Callable[[str], None], current: str) -> Gtk.Widget:
    """The Trigger-Happy 1-40 range, kept out of the diagram (per ticket 38's
    own steer) behind a nested show/hide toggle — the same shape as
    `key_picker`'s "Show F13-F24 ▸" sub-toggle, safe because it only grows an
    already-inline, already-visible panel."""
    state = {"shown": False}
    section = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    toggle_btn = Gtk.Button(label="Extra buttons (Trigger-Happy 1-40) ▸", halign=Gtk.Align.START)
    slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    section.append(toggle_btn)
    section.append(slot)

    def render():
        clear_children(slot)
        if not state["shown"]:
            return
        grid = Gtk.Grid(row_spacing=3, column_spacing=3)
        for i, (code, _label) in enumerate(EXTRA):
            btn = Gtk.Button(label=str(i + 1), css_classes=["padbtn"])
            btn.set_size_request(26, 24)
            btn.set_tooltip_text(LABEL_BY_CODE[code])
            if code == current:
                btn.add_css_class("suggested-action")
            btn.connect("clicked", lambda b, code=code: on_pick(code))
            grid.attach(btn, i % 8, i // 8, 1, 1)
        slot.append(grid)

    def on_toggle(b):
        state["shown"] = not state["shown"]
        toggle_btn.set_label("Extra buttons (Trigger-Happy 1-40) ▾" if state["shown"] else "Extra buttons (Trigger-Happy 1-40) ▸")
        render()

    toggle_btn.connect("clicked", on_toggle)
    render()
    return section


def build_inline_controller_picker(
    current_code: str, on_change: Callable[[str], None]
) -> Gtk.Widget:
    """A "Selected: <label>" summary plus the gamepad diagram and the Extra
    grid, always shown inline (see module docstring). One component serves
    both a Binding's ControllerButton `button` field and (per ticket 14's
    Answer) a Macro step's KeyDown/KeyUp value carrying a gamepad code —
    mounted wherever the caller needs it, same as `key_picker`'s picker.
    """
    state = {"code": current_code}
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

    summary_label = Gtk.Label(xalign=0, css_classes=["controller-picker-summary"])
    root.append(summary_label)

    panel = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6, css_classes=["picker-panel"])
    root.append(panel)

    def render_summary():
        summary_label.set_label(f"Selected: {LABEL_BY_CODE.get(state['code'], state['code'])}")

    def render_panel():
        clear_children(panel)
        panel.append(_pad_diagram(on_pick, state["code"]))
        panel.append(_extra_grid(on_pick, state["code"]))

    def on_pick(code: str):
        state["code"] = code
        render_summary()
        render_panel()
        on_change(code)

    render_summary()
    render_panel()
    return root
