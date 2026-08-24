"""PROTOTYPE — throwaway, answers ticket 60 (Prototype the axis-assignment
GUI): .scratch/tartarus-input-expansion/issues/60-prototype-axis-assignment-ux.md

Builds on ticket 59's settled model: axis assignment is a new, parallel
per-(Layer, Input) concept, grid keys only — mutually exclusive with that
Layer's ordinary Binding/Chord-membership for the same key — offering one of
17 targets (5 unsigned single-key axes + 6 signed axes split into two
independently-assignable +/- halves). It reuses the key's existing
Actuation/Release-point UX (tickets 19/26) as the axis's start/end
thresholds, so no deadzone control is modeled here — ticket 59 already
settled that part.

Four open UX questions, one variant apiece, each answering *all four*
differently so the three read as genuinely different systems rather than a
palette swap on one shared layout:

    A - Toggle + Diagram + Toast + Grey-out
        A plain toggle button sits above the ordinary Action-kind dropdown;
        turning it on swaps the whole Action/Trigger area for a hand-drawn
        gamepad-style diagram (ROUND 3: LT/RT sit directly above their
        stick, matching a real gamepad's physical layout, over the Left/
        Right stick crosses; a horizontal rule below the sticks separates
        them from two named groups underneath — Driving: Wheel/Gas/Brake,
        Flight: Rudder/Throttle — Gtk.Fixed-free, all plain Buttons/Grids,
        same cairo-avoidance as ticket 19/38's prototypes). Picking a
        target already claimed by another key shows a one-shot banner
        reusing the real app's existing `.toast` convention (ticket 55's
        steal-toast).
        Axis-assigned grid keys go fully insensitive (grey, unclickable,
        tooltipped) the moment Chord-member selection is toggled on.

    B - 6th dropdown entry + Diagram (from A) + Toast (from A) + Stripe
        ROUND 2, after live reaction: the user's pick, combining B's fork
        mechanism and grid treatment with A's picker. "Axis" is simply a
        6th entry in the existing Action-kind dropdown (Keypress /
        Controller Button / Macro / Stepper / Profile Switch / Axis) for
        grid keys — non-grid Inputs never see the option at all, rather
        than seeing it and having it disabled. Trigger-mode locks
        insensitive with a tooltip, mirroring Profile Switch's existing
        lock in the real `binding_editor.py`. Picking "Axis" swaps in
        variant A's own diagram picker (ROUND 3 layout: LT/RT above their
        stick, a rule under the sticks, Driving/Flight groups below it,
        `.toast` steal-banner included) — `build_axis_picker_diagram` is
        shared code, not a re-implementation.
        Axis-assigned grid keys stay fully clickable at all times and carry
        an always-visible purple diagonal-stripe look (not just during
        Chord selection, and not merely a tooltip) tying the same accent
        color used for every "this is an axis" affordance across the whole
        pane; clicking a striped key while selecting Chord members surfaces
        an inline error line instead of toggling it into the selection.

    C - Segmented control + Group-then-value + Proactive dot + Padlock
        A two-button Digital/Axis segmented control *replaces* the Action
        dropdown outright for grid keys (and is never shown at all for
        non-grid Inputs — the fork is structurally absent, not just
        disabled). The catalog is two-step: pick a physical group first
        (Triggers/Pedals, Left Stick, Right Stick, Rudder, Wheel), then a
        value within it — every value shows a green/amber dot *before* it's
        picked, so a claim is visible up front rather than surfaced only
        after committing (unlike A/B's reactive toast/note). Axis-assigned
        grid keys carry an always-visible padlock badge; clicking one while
        selecting Chord members flags it with an inline note standing in
        for a shake animation (this environment's GTK4 has no real
        keyframe wiring set up here — noted, not implemented).

Shared mock: `CLAIMS` doubles as both "which grid keys are already
Axis-assigned this Layer" (feeds every grid-strip's exclusion visual) and
"what target each has claimed" (feeds every picker's cross-key-claim
affordance) — Grid 5 → Right Trigger, Grid 12 → Left Stick X+. Switch
Inputs in any variant's dropdown to Grid 5 or Grid 12 to see an
already-Axis-assigned key open with its pick pre-filled.

Wipe me: nothing here persists past process exit; none of it should be
promoted as-is (see each variant's own fold-in note once one wins).

Run:
    python3 gui/prototype_60_axis_assignment_ux.py
"""

from __future__ import annotations

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gtk

from .gtk_utils import clear_children

CSS = """
.heading { font-weight: bold; }
.dim { opacity: 0.6; font-size: smaller; }
.editor-pane { padding: 10px; background-color: alpha(currentColor, 0.04); border-radius: 6px; margin-bottom: 6px; }
.picker-panel { padding: 8px; background-color: alpha(currentColor, 0.05); border-radius: 6px; }
.section-label { font-size: smaller; opacity: 0.65; font-weight: bold; }
.switcher-pill { background-color: alpha(currentColor, 0.08); border: 1px solid alpha(currentColor, 0.25); border-radius: 999px; padding: 6px 14px; }
.variant-label { font-weight: bold; }

/* Ticket 55's real steal-toast convention, reused verbatim for variant A */
.toast { background-color: alpha(#4a90e2, 0.18); border-radius: 6px; padding: 6px 8px; font-size: smaller; }
/* The real app's .error convention, reused for variant B/C's inline flags */
.error { color: #e53935; font-size: smaller; }
.inline-note { opacity: 0.7; font-size: smaller; font-style: italic; }

/* One shared "axis" accent (violet) across all three variants, so the
   concept reads consistently even though the interaction shape differs. */
.axis-toggle:checked { background-color: alpha(#8e44ad, 0.35); }
.axis-btn { min-width: 0; padding: 4px 8px; font-size: 11px; }
.axis-btn-current { background-color: alpha(#8e44ad, 0.35); border: 1px solid #8e44ad; }
.axis-btn-stick { min-width: 34px; }
.axis-list-btn { padding: 3px 8px; font-size: 12px; }
.category-row { padding: 2px 6px 2px 12px; }

.grid-btn { min-width: 30px; min-height: 30px; font-size: 11px; padding: 2px; }
.chord-selected { border: 3px solid #2ecc71; }
/* Variant A: fully greyed via set_sensitive(False), no extra class needed
   beyond GTK's own :disabled look — kept minimal on purpose. */
/* Variant B: always-visible diagonal stripe on an Axis-assigned key. */
.axis-stripe {
    background-image: repeating-linear-gradient(45deg, alpha(#8e44ad, 0.30) 0px, alpha(#8e44ad, 0.30) 4px, transparent 4px, transparent 9px);
}
/* Variant C: padlock badge + a static "shake" stand-in class (no keyframes
   wired in this throwaway app — see module docstring). */
.padlock-badge { font-size: 10px; margin: 1px; }
.shake { border: 2px solid #e53935; }

.segmented { background-color: alpha(currentColor, 0.06); border-radius: 6px; padding: 2px; }
.segmented-btn { padding: 4px 14px; }
.segmented-active { background-color: alpha(#8e44ad, 0.35); font-weight: bold; }
.group-btn { padding: 4px 10px; font-size: 12px; }
.group-btn-active { background-color: alpha(currentColor, 0.15); font-weight: bold; }
.value-btn { padding: 3px 8px; font-size: 12px; }
.value-btn-current { background-color: alpha(#8e44ad, 0.35); border: 1px solid #8e44ad; }
"""

# ---------------------------------------------------------------------
# Shared catalog — ticket 59's settled 17-target scope: 5 unsigned single-
# key axes, 6 signed axes each split into an independently-assignable +/-
# half. ABS_HAT0X/Y and a distinct HOTAS device identity are both out of
# scope per ticket 59 §3 — not modeled here.
# ---------------------------------------------------------------------

UNSIGNED = [
    ("ABS_Z", "Left Trigger"),
    ("ABS_RZ", "Right Trigger"),
    ("ABS_THROTTLE", "Throttle"),
    ("ABS_GAS", "Gas"),
    ("ABS_BRAKE", "Brake"),
]
SIGNED = [
    ("ABS_X", "Left Stick X"),
    ("ABS_Y", "Left Stick Y"),
    ("ABS_RX", "Right Stick X"),
    ("ABS_RY", "Right Stick Y"),
    ("ABS_RUDDER", "Rudder"),
    ("ABS_WHEEL", "Wheel"),
]


def _signed_halves() -> list[tuple[str, str]]:
    out = []
    for code, label in SIGNED:
        out.append((f"{code}_POS", f"{label} +"))
        out.append((f"{code}_NEG", f"{label} −"))
    return out


ALL_TARGETS = UNSIGNED + _signed_halves()
assert len(ALL_TARGETS) == 17, "ticket 59 settled exactly 17 axis targets"
LABEL_BY_TARGET = {code: label for code, label in ALL_TARGETS}

# Variant B's always-expanded category grouping.
AXIS_CATEGORIES: dict[str, list[tuple[str, str]]] = {
    "Triggers / Pedals": UNSIGNED,
    "Left Stick": [("ABS_X_POS", "X +"), ("ABS_X_NEG", "X −"), ("ABS_Y_POS", "Y +"), ("ABS_Y_NEG", "Y −")],
    "Right Stick": [("ABS_RX_POS", "X +"), ("ABS_RX_NEG", "X −"), ("ABS_RY_POS", "Y +"), ("ABS_RY_NEG", "Y −")],
    "Rudder": [("ABS_RUDDER_POS", "+"), ("ABS_RUDDER_NEG", "−")],
    "Wheel": [("ABS_WHEEL_POS", "+"), ("ABS_WHEEL_NEG", "−")],
}

# Variant C's group-then-value picker — same partition, keyed for the
# two-step flow.
GROUPS: list[tuple[str, str, list[tuple[str, str]]]] = [
    ("triggers", "Triggers / Pedals", UNSIGNED),
    ("lstick", "Left Stick", AXIS_CATEGORIES["Left Stick"]),
    ("rstick", "Right Stick", AXIS_CATEGORIES["Right Stick"]),
    ("rudder", "Rudder", AXIS_CATEGORIES["Rudder"]),
    ("wheel", "Wheel", AXIS_CATEGORIES["Wheel"]),
]


def group_of(code: str) -> str:
    for key, _label, entries in GROUPS:
        if code in {c for c, _ in entries}:
            return key
    return "triggers"


# ---------------------------------------------------------------------
# Mock Inputs + claims. A real grid has 20 keys (ticket 59 §1: only grid
# keys are eligible, the Mode key/thumbstick/wheel stay fully digital) —
# two non-grid Inputs are in the dropdown specifically to demonstrate the
# fork's *absence* for them, not just a disabled state.
#
# CLAIMS does double duty: it is both "which grid keys are already
# Axis-assigned on this Layer" (feeds every grid-strip's exclusion visual)
# and "what target each already claimed" (feeds every picker's cross-key-
# claim affordance) — a real Daemon/config snapshot would separate these,
# but for this mock one dict of `grid key -> target code` covers both.
# ---------------------------------------------------------------------

DROPDOWN_INPUTS = ["Grid 1", "Grid 2", "Grid 5", "Grid 12", "Mode key", "Thumbstick Up"]
CLAIMS: dict[str, str] = {"Grid 5": "ABS_RZ", "Grid 12": "ABS_X_POS"}
CLAIMANT_BY_TARGET = {target: key for key, target in CLAIMS.items()}


def is_grid(inp: str) -> bool:
    return inp.startswith("Grid ")


def seed_state(state: dict, inp: str) -> None:
    """Re-seeds the fork/target fields for a freshly selected Input —
    shared by all three variants so switching to Grid 5/Grid 12 always
    opens pre-filled with its existing Axis pick, and switching to a
    non-grid Input always forces the fork off."""
    state["input"] = inp
    existing = CLAIMS.get(inp) if is_grid(inp) else None
    state["is_axis"] = existing is not None
    state["axis_target"] = existing
    state["axis_group"] = group_of(existing) if existing else "triggers"


def new_state(initial_input: str) -> dict:
    state: dict = {
        "chord_selecting": False,
        "chord_selected": set(),
        "chord_error": None,
        "chord_shake": None,
    }
    seed_state(state, initial_input)
    return state


def labeled_row(label: str, widget: Gtk.Widget) -> Gtk.Box:
    row = Gtk.Box(spacing=8)
    lbl = Gtk.Label(label=label, xalign=0)
    lbl.set_size_request(90, -1)
    row.append(lbl)
    widget.set_hexpand(True)
    row.append(widget)
    return row


def input_dropdown_row(state: dict, render) -> Gtk.Box:
    dd = Gtk.DropDown(model=Gtk.StringList.new(DROPDOWN_INPUTS))
    dd.set_selected(DROPDOWN_INPUTS.index(state["input"]))

    def on_changed(d, *_):
        seed_state(state, DROPDOWN_INPUTS[d.get_selected()])
        render()

    dd.connect("notify::selected", on_changed)
    return labeled_row("Input", dd)


def mock_digital_body() -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    box.append(labeled_row("Trigger mode", Gtk.DropDown(model=Gtk.StringList.new(["Fire-once", "Hold-to-repeat", "Toggle"]))))
    box.append(labeled_row("Action", Gtk.DropDown(model=Gtk.StringList.new(["Keypress", "Controller Button", "Macro"]))))
    box.append(Gtk.Label(label="(ordinary Action body — not mocked here, see tickets 32/38/55)", xalign=0, css_classes=["dim"]))
    return box


# =======================================================================
# Variant A — Toggle + Diagram + Toast + Grey-out
# =======================================================================


def build_stick_cross(
    title: str,
    x_base: str,
    y_base: str,
    current: str | None,
    on_pick,
    trigger: tuple[str, str, int] | None = None,
) -> Gtk.Widget:
    """A stick's X/Y cross, optionally with a trigger button placed *in the
    same Gtk.Grid* as an extra row above it — `trigger` is
    `(code, caption, column)`, where `column` is 0 (X−'s column) or 2 (X+'s
    column), never 1 (Y's own column). Sharing one grid, rather than
    stacking a separately-centered trigger button over a second widget, is
    what makes the trigger land pixel-exact above X−/X+ instead of merely
    close to it."""
    col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    col.append(Gtk.Label(label=title, css_classes=["section-label"]))
    grid = Gtk.Grid(row_spacing=2, column_spacing=2)

    def mk(code: str, cap: str) -> Gtk.Button:
        classes = ["axis-btn", "axis-btn-stick"] + (["axis-btn-current"] if code == current else [])
        b = Gtk.Button(label=cap, css_classes=classes)
        b.set_tooltip_text(LABEL_BY_TARGET[code])
        b.connect("clicked", lambda _b, c=code: on_pick(c))
        return b

    row = 0
    if trigger is not None:
        t_code, t_cap, t_col = trigger
        tbtn = mk(t_code, t_cap)
        tbtn.set_margin_bottom(6)
        grid.attach(tbtn, t_col, row, 1, 1)
        row += 1

    grid.attach(mk(f"{y_base}_POS", "Y+"), 1, row, 1, 1)
    grid.attach(mk(f"{x_base}_NEG", "X−"), 0, row + 1, 1, 1)
    grid.attach(Gtk.Label(label="⊕", css_classes=["dim"]), 1, row + 1, 1, 1)
    grid.attach(mk(f"{x_base}_POS", "X+"), 2, row + 1, 1, 1)
    grid.attach(mk(f"{y_base}_NEG", "Y−"), 1, row + 2, 1, 1)
    col.append(grid)
    return col


def build_axis_picker_diagram(state: dict, render) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["picker-panel"])

    claimant = CLAIMS.get(state["axis_target"]) if state["axis_target"] else None
    # Ticket 59 §5: sharing a target is allowed, not rejected — the toast
    # only ever informs, it never blocks the pick that already happened.
    if state["axis_target"] and claimant and claimant != state["input"]:
        box.append(
            Gtk.Label(
                label=f"Also assigned to {claimant} — allowed, both keys will drive this axis.",
                xalign=0,
                wrap=True,
                css_classes=["toast"],
            )
        )

    current_label = LABEL_BY_TARGET.get(state["axis_target"], "— none picked —")
    box.append(Gtk.Label(label=f"Axis target: {current_label}", xalign=0, css_classes=["heading"]))

    def on_pick(code: str) -> None:
        state["axis_target"] = code
        render()

    # Top: LT/RT sit directly above their stick's *outer* column — X− for
    # the Left stick, X+ for the Right stick — pushing them further apart
    # than a plain center-above-the-stick placement would, matching where
    # shoulder triggers actually sit relative to the sticks on a real
    # gamepad (outboard of them, not directly overhead).
    top_row = Gtk.Box(spacing=24, halign=Gtk.Align.CENTER)
    top_row.append(
        build_stick_cross(
            "Left Stick", "ABS_X", "ABS_Y", state["axis_target"], on_pick, trigger=("ABS_Z", "LT", 0)
        )
    )
    top_row.append(
        build_stick_cross(
            "Right Stick", "ABS_RX", "ABS_RY", state["axis_target"], on_pick, trigger=("ABS_RZ", "RT", 2)
        )
    )
    box.append(top_row)

    box.append(Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL))

    # Below the line: the remaining 7 targets split into two named groups
    # rather than one flat list — Driving (Wheel, Gas, Brake) and Flight
    # (Rudder, Throttle), grouped by which genre of game actually uses them
    # together rather than by unsigned/signed shape. Each group is a single
    # inline row — the unsigned targets sit alongside their signed pair
    # rather than stacked in their own rows below it, to save vertical
    # space.
    def unsigned_btn(code: str, label: str) -> Gtk.Widget:
        classes = ["axis-btn"] + (["axis-btn-current"] if code == state["axis_target"] else [])
        b = Gtk.Button(label=label, css_classes=classes)
        b.connect("clicked", lambda _b, c=code: on_pick(c))
        return b

    def signed_pair_row(base: str, name: str, extra: list[tuple[str, str]]) -> Gtk.Widget:
        row = Gtk.Box(spacing=4)
        row.append(Gtk.Label(label=name, halign=Gtk.Align.START))
        for suffix, sign in (("_NEG", "−"), ("_POS", "+")):
            code = base + suffix
            classes = ["axis-btn"] + (["axis-btn-current"] if code == state["axis_target"] else [])
            b = Gtk.Button(label=sign, css_classes=classes)
            b.connect("clicked", lambda _b, c=code: on_pick(c))
            row.append(b)
        extra_box = Gtk.Box(spacing=4)
        extra_box.set_margin_start(10)
        for code, label in extra:
            extra_box.append(unsigned_btn(code, label))
        row.append(extra_box)
        return row

    def group_box(title: str, row: Gtk.Widget) -> Gtk.Widget:
        col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        col.append(Gtk.Label(label=title, xalign=0, css_classes=["section-label"]))
        col.append(row)
        return col

    bottom_row = Gtk.Box(spacing=28)
    bottom_row.append(
        group_box("Driving", signed_pair_row("ABS_WHEEL", "Wheel", [("ABS_GAS", "Gas"), ("ABS_BRAKE", "Brake")]))
    )
    bottom_row.append(
        group_box("Flight", signed_pair_row("ABS_RUDDER", "Rudder", [("ABS_THROTTLE", "Throttle")]))
    )
    box.append(bottom_row)

    return box


def build_editor_pane_a(state: dict, render) -> Gtk.Widget:
    pane = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["editor-pane"])
    pane.append(Gtk.Label(label="Binding editor (mock)", xalign=0, css_classes=["heading"]))
    pane.append(input_dropdown_row(state, render))

    if is_grid(state["input"]):
        toggle = Gtk.ToggleButton(label="⚡ This key outputs an axis", css_classes=["axis-toggle"], halign=Gtk.Align.START)
        toggle.set_active(state["is_axis"])

        def on_toggle(b):
            state["is_axis"] = b.get_active()
            render()

        toggle.connect("toggled", on_toggle)
        pane.append(toggle)
    else:
        pane.append(
            Gtk.Label(
                label="(Only grid keys can be Axis-assigned — the Mode key, thumbstick, and wheel always use the ordinary Action editor.)",
                xalign=0,
                wrap=True,
                css_classes=["dim"],
            )
        )

    if state["is_axis"] and is_grid(state["input"]):
        pane.append(build_axis_picker_diagram(state, render))
    else:
        pane.append(mock_digital_body())

    return pane


def build_grid_strip_a(state: dict, render) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    header = Gtk.Box(spacing=8)
    header.append(Gtk.Label(label="Device grid (mock)", xalign=0, css_classes=["section-label"]))
    chord_btn = Gtk.ToggleButton(label="Select Chord members")
    chord_btn.set_active(state["chord_selecting"])

    def on_chord_toggle(b):
        state["chord_selecting"] = b.get_active()
        if not state["chord_selecting"]:
            state["chord_selected"] = set()
        render()

    chord_btn.connect("toggled", on_chord_toggle)
    header.append(chord_btn)
    box.append(header)

    grid = Gtk.Grid(row_spacing=3, column_spacing=3)
    for i in range(1, 21):
        key = f"Grid {i}"
        axis_assigned = key in CLAIMS or (key == state["input"] and state["is_axis"])
        classes = ["grid-btn"] + (["chord-selected"] if key in state["chord_selected"] else [])
        b = Gtk.Button(label=str(i), css_classes=classes)
        if state["chord_selecting"] and axis_assigned:
            b.set_sensitive(False)
            b.set_tooltip_text(f"{key}: Axis-assigned — can't join a Chord")
        elif state["chord_selecting"]:
            def on_click(_b, k=key):
                if k in state["chord_selected"]:
                    state["chord_selected"].discard(k)
                else:
                    state["chord_selected"].add(k)
                render()

            b.connect("clicked", on_click)
        elif axis_assigned:
            b.set_tooltip_text(f"{key}: Axis-assigned")
        grid.attach(b, (i - 1) % 5, (i - 1) // 5, 1, 1)
    box.append(grid)

    if state["chord_selecting"]:
        chosen = ", ".join(sorted(state["chord_selected"])) or "(none)"
        box.append(Gtk.Label(label=f"Selected: {chosen}", xalign=0, css_classes=["dim"]))

    return box


def build_variant_a() -> Gtk.Widget:
    state = new_state(DROPDOWN_INPUTS[0])
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

    def render():
        clear_children(root)
        root.append(build_editor_pane_a(state, render))
        root.append(Gtk.Separator())
        root.append(build_grid_strip_a(state, render))

    render()
    return root


# =======================================================================
# Variant B — 6th dropdown entry + Diagram (from A) + Toast (from A) + Stripe
#
# Round 2, folded in after live reaction: the user picked B's fork
# mechanism (Axis as a 6th Action-kind entry, absent — not disabled — for
# non-grid Inputs) and B's always-visible purple diagonal stripe on the
# grid strip, but wanted variant A's diagram-style Axis Target picker in
# place of B's flat category list — `build_axis_picker_diagram` is reused
# verbatim below, toast and all, rather than re-implemented, since it was
# already factored out as its own function. `build_axis_picker_list`'s
# flat-list rendering is no longer used by any variant after this swap;
# `AXIS_CATEGORIES` survives only as variant C's own catalog partition.
#
# Round 3, also folded into the shared `build_axis_picker_diagram`: LT/RT
# moved to sit directly above their corresponding stick (gamepad-style),
# a horizontal rule now separates the sticks from the remaining 7 targets,
# and those split into two named groups — Driving (Wheel/Gas/Brake) and
# Flight (Rudder/Throttle) — instead of one flat trigger column.
# =======================================================================

ACTION_KINDS_B = ["Keypress", "Controller Button", "Macro", "Stepper", "Profile Switch", "Axis"]


def build_editor_pane_b(state: dict, render) -> Gtk.Widget:
    pane = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["editor-pane"])
    pane.append(Gtk.Label(label="Binding editor (mock)", xalign=0, css_classes=["heading"]))
    pane.append(input_dropdown_row(state, render))

    # Non-grid Inputs simply never see "Axis" in the list — the option is
    # absent, not present-and-disabled.
    kinds = ACTION_KINDS_B if is_grid(state["input"]) else ACTION_KINDS_B[:-1]
    action_dd = Gtk.DropDown(model=Gtk.StringList.new(kinds))
    action_dd.set_selected(kinds.index("Axis") if state["is_axis"] and "Axis" in kinds else 0)

    def on_action_changed(dd, *_):
        state["is_axis"] = kinds[dd.get_selected()] == "Axis"
        render()

    action_dd.connect("notify::selected", on_action_changed)
    pane.append(labeled_row("Action", action_dd))

    # Mirrors the real binding_editor.py's existing Profile-Switch lock
    # (`trigger_dd.set_sensitive(kind != "profile_switch")`) — Axis gets the
    # same treatment: Trigger-mode has no coherent meaning for a continuous
    # value (ticket 59 §2).
    trigger_dd = Gtk.DropDown(model=Gtk.StringList.new(["Fire-once", "Hold-to-repeat", "Toggle"]))
    trigger_dd.set_sensitive(not state["is_axis"])
    if state["is_axis"]:
        trigger_dd.set_tooltip_text("Axis output has no Trigger mode")
    pane.append(labeled_row("Trigger mode", trigger_dd))

    if state["is_axis"]:
        pane.append(build_axis_picker_diagram(state, render))
    else:
        pane.append(Gtk.Label(label="(ordinary Action body — not mocked here, see tickets 32/38/55)", xalign=0, css_classes=["dim"]))

    return pane


def build_grid_strip_b(state: dict, render) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    header = Gtk.Box(spacing=8)
    header.append(Gtk.Label(label="Device grid (mock)", xalign=0, css_classes=["section-label"]))
    chord_btn = Gtk.ToggleButton(label="Select Chord members")
    chord_btn.set_active(state["chord_selecting"])

    def on_chord_toggle(b):
        state["chord_selecting"] = b.get_active()
        if not state["chord_selecting"]:
            state["chord_selected"] = set()
            state["chord_error"] = None
        render()

    chord_btn.connect("toggled", on_chord_toggle)
    header.append(chord_btn)
    box.append(header)

    grid = Gtk.Grid(row_spacing=3, column_spacing=3)
    for i in range(1, 21):
        key = f"Grid {i}"
        axis_assigned = key in CLAIMS or (key == state["input"] and state["is_axis"])
        classes = ["grid-btn"]
        if axis_assigned:
            classes.append("axis-stripe")
        if key in state["chord_selected"]:
            classes.append("chord-selected")
        b = Gtk.Button(label=str(i), css_classes=classes)

        if state["chord_selecting"]:
            def on_click(_b, k=key, axis=axis_assigned):
                if axis:
                    state["chord_error"] = f"{k} is Axis-assigned — can't join a Chord"
                else:
                    state["chord_error"] = None
                    if k in state["chord_selected"]:
                        state["chord_selected"].discard(k)
                    else:
                        state["chord_selected"].add(k)
                render()

            b.connect("clicked", on_click)
        elif axis_assigned:
            b.set_tooltip_text(f"{key}: Axis-assigned")
        grid.attach(b, (i - 1) % 5, (i - 1) // 5, 1, 1)
    box.append(grid)

    if state["chord_selecting"]:
        if state["chord_error"]:
            box.append(Gtk.Label(label=state["chord_error"], xalign=0, css_classes=["error"]))
        else:
            chosen = ", ".join(sorted(state["chord_selected"])) or "(none)"
            box.append(Gtk.Label(label=f"Selected: {chosen}", xalign=0, css_classes=["dim"]))

    return box


def build_variant_b() -> Gtk.Widget:
    state = new_state(DROPDOWN_INPUTS[0])
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

    def render():
        clear_children(root)
        root.append(build_editor_pane_b(state, render))
        root.append(Gtk.Separator())
        root.append(build_grid_strip_b(state, render))

    render()
    return root


# =======================================================================
# Variant C — Segmented control + Group-then-value + Proactive dot + Padlock
# =======================================================================


def build_segmented(state: dict, render) -> Gtk.Widget:
    row = Gtk.Box(spacing=0, css_classes=["segmented"], halign=Gtk.Align.START)
    for key, label in (("digital", "Digital"), ("axis", "Axis")):
        active = (key == "axis") == state["is_axis"]
        classes = ["segmented-btn"] + (["segmented-active"] if active else [])
        b = Gtk.Button(label=label, css_classes=classes)

        def on_click(_b, k=key):
            state["is_axis"] = k == "axis"
            render()

        b.connect("clicked", on_click)
        row.append(b)
    return row


def build_axis_picker_group_then_value(state: dict, render) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["picker-panel"])
    box.append(Gtk.Label(label="1. Pick a group", xalign=0, css_classes=["section-label"]))

    group_row = Gtk.Box(spacing=4)
    for key, label, _entries in GROUPS:
        classes = ["group-btn"] + (["group-btn-active"] if key == state["axis_group"] else [])
        b = Gtk.Button(label=label, css_classes=classes)

        def on_group(_b, k=key):
            state["axis_group"] = k
            render()

        b.connect("clicked", on_group)
        group_row.append(b)
    box.append(group_row)

    box.append(Gtk.Label(label="2. Pick a value", xalign=0, css_classes=["section-label"]))
    entries = next(e for k, _l, e in GROUPS if k == state["axis_group"])
    value_row = Gtk.Box(spacing=4)
    for code, label in entries:
        claimant = CLAIMS.get(code)
        shared = claimant is not None and claimant != state["input"]
        # Proactive: the dot is shown *before* picking, unlike A's toast or
        # B's note which both only appear once the value is already chosen.
        dot = "\U0001f7e0" if shared else "\U0001f7e2"
        classes = ["value-btn"] + (["value-btn-current"] if code == state["axis_target"] else [])
        btn = Gtk.Button(label=f"{dot} {label}", css_classes=classes)
        if shared:
            btn.set_tooltip_text(f"Already assigned to {claimant} — picking this will share the axis.")

        def on_value(_b, c=code):
            state["axis_target"] = c
            render()

        btn.connect("clicked", on_value)
        value_row.append(btn)
    box.append(value_row)

    if state["axis_target"]:
        claimant = CLAIMS.get(state["axis_target"])
        line = f"Selected: {LABEL_BY_TARGET[state['axis_target']]}"
        if claimant and claimant != state["input"]:
            line += f"  ·  sharing with {claimant}"
        box.append(Gtk.Label(label=line, xalign=0, css_classes=["dim"]))

    return box


def build_editor_pane_c(state: dict, render) -> Gtk.Widget:
    pane = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["editor-pane"])
    pane.append(Gtk.Label(label="Binding editor (mock)", xalign=0, css_classes=["heading"]))
    pane.append(input_dropdown_row(state, render))

    if is_grid(state["input"]):
        pane.append(build_segmented(state, render))
    else:
        # The fork is structurally absent for non-grid Inputs, not merely
        # disabled — there is nothing here to grey out.
        state["is_axis"] = False
        pane.append(
            Gtk.Label(
                label="(Only grid keys get a Digital/Axis choice — the Mode key, thumbstick, and wheel are always digital.)",
                xalign=0,
                wrap=True,
                css_classes=["dim"],
            )
        )

    if state["is_axis"] and is_grid(state["input"]):
        pane.append(build_axis_picker_group_then_value(state, render))
    else:
        pane.append(mock_digital_body())

    return pane


def build_grid_strip_c(state: dict, render) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    header = Gtk.Box(spacing=8)
    header.append(Gtk.Label(label="Device grid (mock)", xalign=0, css_classes=["section-label"]))
    chord_btn = Gtk.ToggleButton(label="Select Chord members")
    chord_btn.set_active(state["chord_selecting"])

    def on_chord_toggle(b):
        state["chord_selecting"] = b.get_active()
        if not state["chord_selecting"]:
            state["chord_selected"] = set()
            state["chord_shake"] = None
        render()

    chord_btn.connect("toggled", on_chord_toggle)
    header.append(chord_btn)
    box.append(header)

    grid = Gtk.Grid(row_spacing=3, column_spacing=3)
    for i in range(1, 21):
        key = f"Grid {i}"
        axis_assigned = key in CLAIMS or (key == state["input"] and state["is_axis"])
        classes = ["grid-btn"]
        if key == state.get("chord_shake"):
            classes.append("shake")
        if key in state["chord_selected"]:
            classes.append("chord-selected")
        b = Gtk.Button(label=str(i), css_classes=classes)

        overlay = Gtk.Overlay()
        overlay.set_child(b)
        if axis_assigned:
            lock = Gtk.Label(label="\U0001f512", css_classes=["padlock-badge"], halign=Gtk.Align.END, valign=Gtk.Align.START)
            overlay.add_overlay(lock)
            if not state["chord_selecting"]:
                b.set_tooltip_text(f"{key}: Axis-assigned")

        if state["chord_selecting"]:
            def on_click(_b, k=key, axis=axis_assigned):
                if axis:
                    state["chord_shake"] = k
                else:
                    state["chord_shake"] = None
                    if k in state["chord_selected"]:
                        state["chord_selected"].discard(k)
                    else:
                        state["chord_selected"].add(k)
                render()

            b.connect("clicked", on_click)

        grid.attach(overlay, (i - 1) % 5, (i - 1) // 5, 1, 1)
    box.append(grid)

    if state["chord_selecting"]:
        if state.get("chord_shake"):
            box.append(
                Gtk.Label(
                    label=f"{state['chord_shake']} is Axis-assigned — can't join a Chord "
                    "(stand-in for a shake animation — not actually wired here)",
                    xalign=0,
                    wrap=True,
                    css_classes=["error"],
                )
            )
        else:
            chosen = ", ".join(sorted(state["chord_selected"])) or "(none)"
            box.append(Gtk.Label(label=f"Selected: {chosen}", xalign=0, css_classes=["dim"]))

    return box


def build_variant_c() -> Gtk.Widget:
    state = new_state(DROPDOWN_INPUTS[0])
    root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

    def render():
        clear_children(root)
        root.append(build_editor_pane_c(state, render))
        root.append(Gtk.Separator())
        root.append(build_grid_strip_c(state, render))

    render()
    return root


# --- Switcher chrome (same shape as tickets 19/30/31/32/38) ---

VARIANTS = [
    ("A", "Toggle + Diagram + Toast + Grey-out", build_variant_a),
    ("B", "6th dropdown entry + Diagram (from A) + Toast (from A) + Stripe — round 2 pick", build_variant_b),
    ("C", "Segmented control + Group-then-value + Proactive dot + Padlock", build_variant_c),
]


def build_window(app: Gtk.Application) -> Gtk.ApplicationWindow:
    provider = Gtk.CssProvider()
    provider.load_from_data(CSS.encode())
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
    )

    win = Gtk.ApplicationWindow(application=app, title="Ticket 60 prototype — axis-assignment UX")
    win.set_default_size(560, 700)

    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
    win.set_child(outer)

    scroller = Gtk.ScrolledWindow(vexpand=True, hexpand=True)
    scroller.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
    outer.append(scroller)

    content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    content.set_margin_top(10)
    content.set_margin_bottom(10)
    content.set_margin_start(10)
    content.set_margin_end(10)
    scroller.set_child(content)

    index = {"i": 0}

    def render():
        clear_children(content)
        key, label, build = VARIANTS[index["i"]]
        content.append(build())
        variant_label.set_label(f"{key} — {label}")

    switcher = Gtk.Box(spacing=8, halign=Gtk.Align.CENTER, css_classes=["switcher-pill"])
    switcher.set_margin_bottom(10)
    prev_btn = Gtk.Button(label="←")
    next_btn = Gtk.Button(label="→")
    variant_label = Gtk.Label(css_classes=["variant-label"])

    def cycle(delta):
        index["i"] = (index["i"] + delta) % len(VARIANTS)
        render()

    prev_btn.connect("clicked", lambda b: cycle(-1))
    next_btn.connect("clicked", lambda b: cycle(1))
    switcher.append(prev_btn)
    switcher.append(variant_label)
    switcher.append(next_btn)
    outer.append(switcher)

    key_controller = Gtk.EventControllerKey()

    def on_key(controller, keyval, keycode, state_flags):
        focus = win.get_focus()
        if isinstance(focus, (Gtk.Editable, Gtk.Scale, Gtk.SpinButton)):
            return False
        if keyval == Gdk.KEY_Left:
            cycle(-1)
            return True
        if keyval == Gdk.KEY_Right:
            cycle(1)
            return True
        return False

    key_controller.connect("key-pressed", on_key)
    win.add_controller(key_controller)

    render()
    return win


def main() -> None:
    app = Gtk.Application(application_id="com.acheron.prototype.ticket60")
    app.connect("activate", lambda a: build_window(a).present())
    app.run(None)


if __name__ == "__main__":
    main()
