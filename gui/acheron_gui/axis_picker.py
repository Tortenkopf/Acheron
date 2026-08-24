"""The real axis-target picker (ticket 71), for the parallel Axis-assignment
concept (ticket 59/60) — structurally independent of `Action`/`Binding`
(CONTEXT.md: Axis assignment), so this is *not* wired the same way
`controller_picker.py`'s picker is: there's no `Action` dict field for it to
write into. `build_inline_axis_picker` only ever hands its caller a bare
target wire string (one of the 17 in `AXIS_LABEL_BY_TARGET`), matching what
`daemon_client.set_axis_assignment`'s own `target` argument takes.

Ports ticket 60's settled diagram — round 5's refined combination of variant
B's fork mechanism/grid treatment and variant A's diagram picker
(`prototype/60-axis-assignment-ux`) — against the real 17-target catalog
(ticket 59 §3), not the prototype's hand-listed one: Left/Right Stick each a
4-direction cross (Y+ / X− / X+ / Y−) with the stick's name directly above
it, Left Trigger beside the Left stick (to its left), Right Trigger beside
the Right stick (to its right), a horizontal rule below the sticks, then two
named groups by genre — Driving (Wheel +/− with Gas/Brake inline) and Flight
(Rudder +/− with Throttle inline). Every signed half is its own separate,
individually clickable button — never a single control — so "read as two
separate picks" falls out by construction rather than needing a special
case.
"""

from __future__ import annotations

from typing import Callable

from gi.repository import Gtk

from .gtk_utils import clear_children

# ---------------------------------------------------------------------
# Catalog — ticket 59 §3's settled 17-target list: 5 unsigned single-key
# axes (no polar opposite) + 6 signed axes split into independently-
# assignable +/- halves (12 half-axis targets). Wire strings mirror
# `daemon/src/dbus/wire.rs::axis_target_str` exactly.
# ---------------------------------------------------------------------

LEFT_STICK = [
    ("left_stick_y_pos", "↑"),
    ("left_stick_x_neg", "←"),
    ("left_stick_x_pos", "→"),
    ("left_stick_y_neg", "↓"),
]
RIGHT_STICK = [
    ("right_stick_y_pos", "↑"),
    ("right_stick_x_neg", "←"),
    ("right_stick_x_pos", "→"),
    ("right_stick_y_neg", "↓"),
]
TRIGGERS = [("left_trigger", "LT"), ("right_trigger", "RT")]
DRIVING = [("wheel_neg", "Wheel −"), ("wheel_pos", "Wheel +"), ("gas", "Gas"), ("brake", "Brake")]
FLIGHT = [("rudder_neg", "Rudder −"), ("rudder_pos", "Rudder +"), ("throttle", "Throttle")]

AXIS_LABEL_BY_TARGET: dict[str, str] = {
    "left_trigger": "Left Trigger",
    "right_trigger": "Right Trigger",
    "throttle": "Throttle",
    "gas": "Gas",
    "brake": "Brake",
    "left_stick_x_pos": "Left Stick X+",
    "left_stick_x_neg": "Left Stick X−",
    "left_stick_y_pos": "Left Stick Y+",
    "left_stick_y_neg": "Left Stick Y−",
    "right_stick_x_pos": "Right Stick X+",
    "right_stick_x_neg": "Right Stick X−",
    "right_stick_y_pos": "Right Stick Y+",
    "right_stick_y_neg": "Right Stick Y−",
    "rudder_pos": "Rudder +",
    "rudder_neg": "Rudder −",
    "wheel_pos": "Wheel +",
    "wheel_neg": "Wheel −",
}

assert len(AXIS_LABEL_BY_TARGET) == 17, "the axis catalog must match the Daemon's 17-target list"


def _target_button(target: str, cap: str, current: str) -> Gtk.Button:
    btn = Gtk.Button(label=cap, css_classes=["padbtn"])
    if target == current:
        btn.add_css_class("axis-target-current")
    btn.set_tooltip_text(AXIS_LABEL_BY_TARGET[target])
    return btn


def _stick_block(label: str, cross: list[tuple[str, str]], current: str, on_pick: Callable[[str], None]) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2, halign=Gtk.Align.CENTER)
    box.append(Gtk.Label(label=label, css_classes=["section-label"]))
    grid = Gtk.Grid(row_spacing=2, column_spacing=2)
    positions = [(1, 0), (0, 1), (2, 1), (1, 2)]
    for (target, cap), (col, row) in zip(cross, positions):
        btn = _target_button(target, cap, current)
        btn.set_size_request(32, 28)
        btn.connect("clicked", lambda b, target=target: on_pick(target))
        grid.attach(btn, col, row, 1, 1)
    box.append(grid)
    return box


def _diagram(current: str, on_pick: Callable[[str], None]) -> Gtk.Widget:
    row = Gtk.Box(spacing=10, halign=Gtk.Align.CENTER)

    left_trigger_target, left_trigger_cap = TRIGGERS[0]
    left_trigger_btn = _target_button(left_trigger_target, left_trigger_cap, current)
    left_trigger_btn.set_size_request(36, 28)
    left_trigger_btn.connect("clicked", lambda b: on_pick(left_trigger_target))
    row.append(left_trigger_btn)

    row.append(_stick_block("Left Stick", LEFT_STICK, current, on_pick))

    row.append(_stick_block("Right Stick", RIGHT_STICK, current, on_pick))

    right_trigger_target, right_trigger_cap = TRIGGERS[1]
    right_trigger_btn = _target_button(right_trigger_target, right_trigger_cap, current)
    right_trigger_btn.set_size_request(36, 28)
    right_trigger_btn.connect("clicked", lambda b: on_pick(right_trigger_target))
    row.append(right_trigger_btn)

    return row


def build_inline_axis_picker(
    current_target: str | None,
    on_change: Callable[[str], None],
    claimed_by: dict[str, str] | None = None,
) -> Gtk.Widget:
    """A diagram picker plus a "Selected: <label>" summary, always shown
    inline (mirrors `controller_picker.build_inline_controller_picker`'s own
    always-inline shape). `claimed_by` maps a target wire string to another
    key's display label that's already assigned it, if any — used to show
    ticket 60's one-shot cross-key toast ("Also assigned to `<key>` —
    allowed, both keys will drive this axis") the moment a shared target is
    picked; purely informational, never blocking (ticket 59 §5 already
    settled that sharing a target is allowed).
    """
    claimed_by = claimed_by or {}
    state = {"target": current_target, "toast": None}
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

    summary_label = Gtk.Label(xalign=0, css_classes=["controller-picker-summary"])
    root.append(summary_label)

    panel = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["picker-panel"])
    root.append(panel)

    def render_summary():
        label = AXIS_LABEL_BY_TARGET.get(state["target"], "None") if state["target"] else "None"
        summary_label.set_label(f"Selected: {label}")

    def render_panel():
        clear_children(panel)
        current = state["target"] or ""
        panel.append(_diagram(current, on_pick))
        panel.append(Gtk.Separator())

        driving_row = Gtk.Box(spacing=6, halign=Gtk.Align.CENTER)
        driving_row.append(Gtk.Label(label="Driving:", css_classes=["section-label"]))
        for target, cap in DRIVING:
            btn = _target_button(target, cap, current)
            btn.connect("clicked", lambda b, target=target: on_pick(target))
            driving_row.append(btn)
        panel.append(driving_row)

        flight_row = Gtk.Box(spacing=6, halign=Gtk.Align.CENTER)
        flight_row.append(Gtk.Label(label="Flight:", css_classes=["section-label"]))
        for target, cap in FLIGHT:
            btn = _target_button(target, cap, current)
            btn.connect("clicked", lambda b, target=target: on_pick(target))
            flight_row.append(btn)
        panel.append(flight_row)

        if state["toast"] is not None:
            panel.append(Gtk.Label(label=state["toast"], xalign=0, wrap=True, css_classes=["toast"]))

    def on_pick(target: str):
        state["target"] = target
        owner = claimed_by.get(target)
        state["toast"] = (
            f"Also assigned to {owner} — allowed, both keys will drive this axis." if owner else None
        )
        render_summary()
        render_panel()
        on_change(target)

    render_summary()
    render_panel()
    return root
