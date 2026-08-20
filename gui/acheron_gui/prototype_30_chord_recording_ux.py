"""PROTOTYPE — throwaway, answers ticket 30 (Design the look and feel of the
Chord-recording flow in the GUI):
.scratch/tartarus-input-expansion/issues/30-prototype-chord-recording-ux.md

Plan: three structurally different variants of the Chord-recording flow, a
new top-level Device Overview surface (not scoped to one Input's popover the
way ticket 19's prototype was), switchable via a floating bottom pill (same
adaptation of the skill's `?variant=` convention as ticket 19 — Prev/Next
buttons plus Left/Right arrow keys, this being a GTK4 desktop app rather
than a routed web page). No real hardware in this environment: a compact
replica of Device Overview's own grid/thumbstick/mode-key/wheel layout
stands in for "the physical device" — clicking a button toggles that Input's
membership in the Chord currently being recorded (a click-to-toggle
simplification of a real press/release capture, same spirit as ticket 19's
Gtk.Scale standing in for real depth). Wipe me: nothing here persists past
process exit, and none of it should be promoted as-is (see each variant's
own fold-in note for what to carry into the real GUI).

Run:
    python3 gui/prototype_30_chord_recording_ux.py

Variants:
    A — The winner (round 2, per live reaction): no separate recording
        surface at all. Free Inputs on Device Overview's own grid toggle
        straight into the in-progress selection; a "Binding →" button below
        the grid enables once ≥2 are selected and opens a small dialog for
        just the Trigger/Action step. Already-claimed Inputs are disabled
        (round 1 instead opened a second grid in a modal wizard and only
        rejected overlap at the very end — kept below for comparison).
    B — Entry point lives inside each Input's own popover ("Add to
        Chord…" / "Part of Chord…"); recording plays out as a non-modal
        banner above the live grid and auto-finishes ~1.2s after the last
        change; an already-claimed Input is skipped with an inline note.
    C — A persistent "Chords" sidebar (mirrors the Action Table's own
        pattern) lists every Chord; recording happens directly on the main
        grid, with already-claimed Inputs structurally disabled rather
        than merely warned about.

Every variant shares two low-level primitives that are settled, not part of
this design question: `build_device_surface`/`sync_surface` (the simulated
hardware) and `build_chord_binding_editor` (the Chord's own Trigger+Action
editing, already the same UI as an ordinary Binding per ticket 01 — this
ticket is only about where/how membership gets recorded relative to it) and
`build_debug_slider` (ticket 01's own explicit ask: a debug-only slider for
the ~50ms window, identical across variants since only its *placement* is a
design question, not its behavior — the slider itself never ships).
"""

from __future__ import annotations

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk

from .gtk_utils import clear_children
from .inputs import GRID_COLS, GRID_ROWS, TRIGGER_OPTIONS, grid_input, input_label

CSS = """
.heading { font-weight: bold; }
.dim { opacity: 0.6; font-size: smaller; }
.error { color: #e53935; font-size: smaller; }
.sidebar { padding: 8px; background-color: alpha(currentColor, 0.06); border-radius: 6px; }
.actuation-section { padding: 8px; margin-top: 4px; background-color: alpha(currentColor, 0.05); border-radius: 6px; }
.badge { border-radius: 999px; padding: 0px 6px; font-size: smaller; font-weight: bold; }
.badge-analog { background-color: #2ecc71; color: black; }
.digital-note { color: #e5883b; font-size: smaller; }
.switcher-pill { background-color: alpha(currentColor, 0.08); border: 1px solid alpha(currentColor, 0.25); border-radius: 999px; padding: 6px 14px; }
.variant-label { font-weight: bold; }
.device-btn { padding: 2px; font-size: smaller; }
.device-btn-down { background-color: #4a90e2; color: white; }
.device-btn-recorded { border: 2px solid #2ecc71; }
.device-btn-claimed { opacity: 0.35; }
.device-btn-highlight { border: 2px solid #e6991a; }
"""


# --- Shared low-level primitive 1: the simulated device surface ---


def build_device_surface(on_click) -> tuple[Gtk.Widget, dict[str, Gtk.Widget]]:
    """A compact replica of Device Overview's own grid/thumbstick/mode-key/
    wheel layout (`device_overview.py::build_main_view`), simplified to
    plain clickable buttons rather than MenuButtons opening a real popover
    — that popover behavior is exactly what each variant below is
    designing around. `on_click(inp)` fires on every button click, single
    down+up collapsed into one event (a click-to-toggle simplification —
    see the module docstring). Returns the root widget plus every Input's
    button, so a variant can restyle them live via `sync_surface`."""
    buttons: dict[str, Gtk.Widget] = {}

    def make_btn(inp: str, w: int = 44, h: int = 40) -> Gtk.Widget:
        btn = Gtk.Button(label=input_label(inp))
        btn.set_size_request(w, h)
        btn.add_css_class("device-btn")
        btn.connect("clicked", lambda b, inp=inp: on_click(inp))
        buttons[inp] = btn
        return btn

    root = Gtk.Box(spacing=20)

    grid = Gtk.Grid(row_spacing=3, column_spacing=3)
    for r in range(1, GRID_ROWS + 1):
        cols = GRID_COLS if r < GRID_ROWS else GRID_COLS - 1
        for c in range(1, cols + 1):
            grid.attach(make_btn(grid_input(r, c)), c - 1, r - 1, 1, 1)
    wheel_col = GRID_COLS - 1
    grid.attach(make_btn("wheel_scroll_up"), wheel_col, GRID_ROWS - 1, 1, 1)
    grid.attach(make_btn("wheel_middle"), wheel_col, GRID_ROWS, 1, 1)
    grid.attach(make_btn("wheel_scroll_down"), wheel_col, GRID_ROWS + 1, 1, 1)
    root.append(grid)

    # Diamond rotation mirrors build_main_view exactly (adjacent lobes are
    # the thumbstick-diagonal worked example ticket 01 asks this prototype
    # to keep discoverable).
    stick_col = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10, halign=Gtk.Align.CENTER)
    stick_col.append(make_btn("mode_key", 40, 28))
    diamond = Gtk.Grid(row_spacing=2, column_spacing=2)
    diamond.attach(make_btn("thumbstick_left", 40, 28), 1, 0, 1, 1)
    diamond.attach(make_btn("thumbstick_down", 40, 28), 0, 1, 1, 1)
    diamond.attach(make_btn("thumbstick_up", 40, 28), 2, 1, 1, 1)
    diamond.attach(make_btn("thumbstick_right", 40, 28), 1, 2, 1, 1)
    stick_col.append(diamond)
    stick_col.append(make_btn(grid_input(4, 5), 40, 28))
    root.append(stick_col)

    return root, buttons


def sync_surface(buttons: dict[str, Gtk.Widget], styler) -> None:
    """`styler(inp) -> (css_classes, sensitive, tooltip)` — called for every
    button on every state change, mirroring ticket 19's DepthTrack.sync_
    markers "resync from source of truth" pattern rather than mutating
    incrementally."""
    for inp, btn in buttons.items():
        for cls in ("device-btn-down", "device-btn-recorded", "device-btn-claimed", "device-btn-highlight"):
            btn.remove_css_class(cls)
        classes, sensitive, tooltip = styler(inp)
        for cls in classes:
            btn.add_css_class(cls)
        btn.set_sensitive(sensitive)
        btn.set_tooltip_text(tooltip or "")


# --- Shared chord store + lookup helpers (identical across variants: which
# Input belongs to which Chord is a fact, not a design question) ---


def make_seed_chords() -> list[dict]:
    return [
        {
            "members": ["grid_r1c1", "grid_r1c2"],
            "trigger": "fire_once",
            "type": "keypress",
            "key": "KEY_C",
            "modifiers": ["ctrl"],
        }
    ]


def chord_members_text(members: list[str]) -> str:
    return " + ".join(input_label(m) for m in members)


def chord_action_text(chord: dict) -> str:
    mods = "+".join(m.capitalize() for m in chord.get("modifiers", []))
    key = chord["key"].replace("KEY_", "")
    combo = f"{mods}+{key}" if mods else key
    return combo


def chord_index_for(state: dict, inp: str, exclude: int | None = None) -> int | None:
    for i, ch in enumerate(state["chords"]):
        if i == exclude:
            continue
        if inp in ch["members"]:
            return i
    return None


def is_claimed(state: dict, inp: str, exclude: int | None = None) -> bool:
    return chord_index_for(state, inp, exclude) is not None


def chords_containing(state: dict, inp: str, exclude: int | None = None) -> list[int]:
    """An Input may belong to any number of Chords (round 3's correction —
    see ticket 01's amended Answer): the four thumbstick directions are the
    motivating case, each sitting in two of the four diagonals."""
    return [i for i, ch in enumerate(state["chords"]) if i != exclude and inp in ch["members"]]


def chord_owner_tooltip(state: dict, inp: str, exclude: int | None = None) -> str | None:
    idxs = chords_containing(state, inp, exclude)
    if not idxs:
        return None
    lines = [
        f"{chord_members_text(state['chords'][i]['members'])} → {chord_action_text(state['chords'][i])}"
        for i in idxs
    ]
    heading = "Part of Chord:" if len(lines) == 1 else "Part of Chords:"
    return heading + "\n" + "\n".join(lines)


def conflicts_with_existing(state: dict, members: list[str], exclude: int | None = None):
    """The only conflict that survives round 3's correction: a subset/
    superset relationship between two Chords' member sets — completing the
    smaller one is indistinguishable from being partway into the larger
    one. A plain intersection (the diagonal shape: two members share one
    Input but each has a member the other lacks) is not a conflict."""
    s = set(members)
    for i, ch in enumerate(state["chords"]):
        if i == exclude:
            continue
        other = set(ch["members"])
        if s <= other or other <= s:
            return i, ch
    return None, None


def new_working_binding() -> dict:
    return {"trigger": "fire_once", "type": "keypress", "key": "KEY_A", "modifiers": []}


# --- Shared low-level primitive 2: the Chord's own Binding editor. Ticket
# 01 already settled that a Chord reuses Binding{trigger, action} unchanged
# (not a new Action variant), and ticket 02 already owns the key/mouse-
# button picker's own look — this is a deliberately minimal stand-in, not a
# redesign, kept identical across variants so only membership-recording
# placement varies. ---


def build_chord_binding_editor(chord: dict) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

    trigger_row = Gtk.Box(spacing=6)
    trigger_row.append(Gtk.Label(label="Trigger"))
    trigger_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in TRIGGER_OPTIONS]))
    trigger_dd.set_selected([k for k, _ in TRIGGER_OPTIONS].index(chord.get("trigger", "fire_once")))
    trigger_dd.connect(
        "notify::selected",
        lambda dd, *_: chord.__setitem__("trigger", TRIGGER_OPTIONS[dd.get_selected()][0]),
    )
    trigger_row.append(trigger_dd)
    box.append(trigger_row)

    key_row = Gtk.Box(spacing=6)
    key_row.append(Gtk.Label(label="Key"))
    key_entry = Gtk.Entry(text=chord.get("key", "KEY_A"))
    key_entry.connect("changed", lambda e: chord.__setitem__("key", e.get_text()))
    key_row.append(key_entry)
    box.append(key_row)

    mod_row = Gtk.Box(spacing=8)
    mods = set(chord.get("modifiers", []))
    for m in ("ctrl", "shift", "alt", "super"):
        cb = Gtk.CheckButton(label=m)
        cb.set_active(m in mods)

        def on_mod(c, m=m):
            cur = set(chord.get("modifiers", []))
            cur.add(m) if c.get_active() else cur.discard(m)
            chord["modifiers"] = sorted(cur)

        cb.connect("toggled", on_mod)
        mod_row.append(cb)
    box.append(mod_row)
    return box


def build_debug_slider(state: dict) -> Gtk.Widget:
    row = Gtk.Box(spacing=6)
    row.append(Gtk.Label(label="Debug: chord window (ms)", css_classes=["dim"]))
    scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 10, 300, 5)
    scale.set_value(state["window_ms"])
    scale.set_hexpand(True)
    scale.set_size_request(140, -1)
    val_lbl = Gtk.Label(label=str(state["window_ms"]), css_classes=["dim"])

    def on_change(s):
        state["window_ms"] = int(s.get_value())
        val_lbl.set_label(str(state["window_ms"]))

    scale.connect("value-changed", on_change)
    row.append(scale)
    row.append(val_lbl)
    return row


# --- Variant A: the winner (round 3, per live reaction). Round 2 dropped the
# separate recording grid in favor of clicking Device Overview's own grid
# directly. Round 3 corrects a premise round 1/2 both inherited from ticket
# 01's *original* Answer — "an Input belongs to at most one Chord" — which
# breaks the thumbstick-diagonal worked example ticket 01 itself calls out
# (Up needs to sit in both Up-Left and Up-Right). Per the corrected model
# (ticket 01 amended, see the map's Decisions-so-far), an Input may belong
# to any number of Chords; the only conflict left is a subset/superset
# relationship between two Chords' member sets, which stays genuinely
# ambiguous ({Up,Left} completing is indistinguishable from being partway
# into {Up,Left,ModeKey}) — everything else, including every pairwise
# thumbstick-diagonal intersection, is fine. That conflict can only be
# known once the *whole* candidate set is known, not per-click, so buttons
# are no longer disabled just for being claimed — the check runs live
# against the current selection instead. ---


def build_variant_a(state: dict, win: Gtk.Window) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    box.append(Gtk.Label(label="Device Overview", xalign=0, css_classes=["heading"]))
    box.append(
        Gtk.Label(
            label="Click two or more Inputs below to start a Chord — an Input may belong to "
            "more than one.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )

    rec = {"recorded": [], "edit_index": None}
    conflict_target = {"i": None}
    preview = {"i": None}

    def styler(inp):
        classes = []
        if inp in rec["recorded"]:
            classes.append("device-btn-recorded")
        elif preview["i"] is not None and inp in state["chords"][preview["i"]]["members"]:
            classes.append("device-btn-highlight")
        return classes, True, chord_owner_tooltip(state, inp, exclude=rec["edit_index"])

    def on_click(inp):
        if inp in rec["recorded"]:
            rec["recorded"].remove(inp)
        else:
            rec["recorded"].append(inp)
        refresh()

    surface, buttons = build_device_surface(on_click)
    box.append(surface)

    status = Gtk.Label(xalign=0, wrap=True, css_classes=["dim"])
    box.append(status)

    action_row = Gtk.Box(spacing=8)
    binding_btn = Gtk.Button(label="Binding →", css_classes=["suggested-action"])
    action_row.append(binding_btn)
    clear_btn = Gtk.Button(label="Clear selection")
    action_row.append(clear_btn)
    conflict_btn = Gtk.Button(label="Edit conflicting Chord")
    conflict_btn.set_visible(False)
    action_row.append(conflict_btn)
    box.append(action_row)

    debug_expander = Gtk.Expander(label="Debug")
    debug_expander.set_child(build_debug_slider(state))
    box.append(debug_expander)

    box.append(Gtk.Separator())
    box.append(Gtk.Label(label="Chords", xalign=0, css_classes=["heading"]))
    box.append(
        Gtk.Label(
            label="Click a Chord to preview its Inputs on the grid above; click Edit to change it.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )
    list_box = Gtk.ListBox(css_classes=["boxed-list"])

    def on_row_selected(lb, row):
        preview["i"] = row.get_index() if row is not None else None
        sync_surface(buttons, styler)

    list_box.connect("row-selected", on_row_selected)
    box.append(list_box)

    def refresh():
        sync_surface(buttons, styler)
        conflict_i, conflict = (
            conflicts_with_existing(state, rec["recorded"], exclude=rec["edit_index"])
            if len(rec["recorded"]) >= 2
            else (None, None)
        )
        conflict_target["i"] = conflict_i
        conflict_btn.set_visible(conflict is not None)
        binding_btn.set_sensitive(len(rec["recorded"]) >= 2 and conflict is None)
        clear_btn.set_sensitive(bool(rec["recorded"]))
        if conflict is not None:
            status.set_label(
                f"{chord_members_text(rec['recorded'])} conflicts with an existing Chord "
                f"({chord_members_text(conflict['members'])} → {chord_action_text(conflict)}): "
                "one member set fully contains the other, so completion would be ambiguous. "
                "Adjust the selection, or edit that Chord instead."
            )
        elif rec["recorded"]:
            status.set_label(
                f"Editing: {chord_members_text(rec['recorded'])}"
                if rec["edit_index"] is not None
                else f"Selected: {chord_members_text(rec['recorded'])}"
            )
        else:
            status.set_label("Click two or more Inputs below to start a Chord — an Input may belong to more than one.")
        desired_preview = preview["i"]
        clear_children(list_box)
        for i, ch in enumerate(state["chords"]):
            row = Gtk.Box(spacing=6)
            row.append(
                Gtk.Label(
                    label=f"{chord_members_text(ch['members'])} → {chord_action_text(ch)}",
                    hexpand=True,
                    xalign=0,
                )
            )
            edit_btn = Gtk.Button(label="Edit")
            edit_btn.connect("clicked", lambda b, i=i: start_edit(i))
            row.append(edit_btn)
            list_box.append(row)
        if desired_preview is not None and desired_preview < len(state["chords"]):
            list_box.select_row(list_box.get_row_at_index(desired_preview))
        else:
            preview["i"] = None

    def start_edit(i: int) -> None:
        rec["edit_index"] = i
        rec["recorded"] = list(state["chords"][i]["members"])
        refresh()

    def on_clear(b):
        rec["recorded"] = []
        rec["edit_index"] = None
        refresh()

    clear_btn.connect("clicked", on_clear)
    conflict_btn.connect("clicked", lambda b: start_edit(conflict_target["i"]))

    def on_binding(b):
        editing = rec["edit_index"] is not None
        working = dict(state["chords"][rec["edit_index"]]) if editing else new_working_binding()

        dlg = Gtk.Window(transient_for=win, modal=True, title="Chord binding")
        dlg.set_default_size(320, 280)
        outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        for setter in (outer.set_margin_top, outer.set_margin_bottom, outer.set_margin_start, outer.set_margin_end):
            setter(10)
        dlg.set_child(outer)

        outer.append(Gtk.Label(label=chord_members_text(rec["recorded"]), xalign=0, css_classes=["heading"]))
        outer.append(build_chord_binding_editor(working))

        btn_row = Gtk.Box(spacing=8)
        outer.append(btn_row)

        def on_save(b):
            chord = dict(working)
            chord["members"] = list(rec["recorded"])
            if editing:
                state["chords"][rec["edit_index"]] = chord
            else:
                state["chords"].append(chord)
            rec["recorded"] = []
            rec["edit_index"] = None
            dlg.close()
            refresh()

        save_btn = Gtk.Button(label="Save Chord", css_classes=["suggested-action"])
        save_btn.connect("clicked", on_save)
        btn_row.append(save_btn)
        cancel_btn = Gtk.Button(label="Cancel")
        cancel_btn.connect("clicked", lambda b: dlg.close())
        btn_row.append(cancel_btn)

        dlg.present()

    binding_btn.connect("clicked", on_binding)

    box.append(
        Gtk.Label(
            label="Tip: click two adjacent thumbstick directions together to define a diagonal — "
            "e.g. Up + Right and Up + Left can coexist; Up belongs to both.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )
    box.append(
        Gtk.Label(
            label="Fold-in note: the grid above is Device Overview's own grid, not a copy — "
            "clicking any Input toggles it into the in-progress Chord directly, no separate "
            "recording dialog. 'Binding →' is the only modal left, and it's just the Trigger/"
            "Action step. No button is ever disabled for being claimed — only the finished "
            "selection is checked, and only for a subset/superset conflict.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )

    refresh()
    return box


# --- Variant B: per-Input popover entry point, non-modal banner, auto-
# finish on settle, overlap skipped live with a note ---


def build_variant_b(state: dict) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.append(Gtk.Label(label="Device Overview", xalign=0, css_classes=["heading"]))
    box.append(
        Gtk.Label(
            label="Click any Input to open its normal Binding popover — "
            "'Add to Chord…' lives inside it.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )

    banner = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4, css_classes=["actuation-section"])
    banner.set_visible(False)
    box.append(banner)

    rec = {"active": False, "recorded": [], "edit_index": None, "working": None, "timer": None}

    def styler(inp):
        classes = ["device-btn-recorded"] if inp in rec["recorded"] else []
        return classes, True, chord_owner_tooltip(state, inp, exclude=rec["edit_index"])

    def on_click(inp):
        if not rec["active"]:
            popovers[inp].popup()
            return
        if inp in rec["recorded"]:
            rec["recorded"].remove(inp)
        elif is_claimed(state, inp, exclude=rec["edit_index"]):
            refresh_banner(note=f"{input_label(inp)} is already in another Chord — skipped.")
            schedule_auto_finish()
            return
        else:
            rec["recorded"].append(inp)
        refresh_banner()
        schedule_auto_finish()

    surface, buttons = build_device_surface(on_click)
    box.append(surface)

    popovers: dict[str, Gtk.Popover] = {}
    for inp, btn in buttons.items():
        pop = Gtk.Popover()
        pop.set_parent(btn)
        popovers[inp] = pop

    def rebuild_input_popovers() -> None:
        for inp, pop in popovers.items():
            content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
            for setter in (content.set_margin_top, content.set_margin_bottom, content.set_margin_start, content.set_margin_end):
                setter(6)
            idx = chord_index_for(state, inp)
            if idx is not None:
                ch = state["chords"][idx]
                content.append(
                    Gtk.Label(
                        label=f"Part of Chord:\n{chord_members_text(ch['members'])} → {chord_action_text(ch)}",
                        xalign=0,
                        wrap=True,
                    )
                )
                edit_btn = Gtk.Button(label="Edit membership")
                edit_btn.connect("clicked", lambda b, i=idx, p=pop: (p.popdown(), start_recording(i)))
                content.append(edit_btn)
                rm_btn = Gtk.Button(label="Remove Chord")
                rm_btn.connect("clicked", lambda b, i=idx, p=pop: (p.popdown(), remove_chord(i)))
                content.append(rm_btn)
            else:
                content.append(Gtk.Label(label=f"{input_label(inp)} — ordinary Binding here (unchanged)", xalign=0, wrap=True))
                add_btn = Gtk.Button(label="Add to Chord…")
                add_btn.connect("clicked", lambda b, p=pop, inp=inp: (p.popdown(), start_recording(None, seed=inp)))
                content.append(add_btn)
            pop.set_child(content)

    def remove_chord(i: int) -> None:
        state["chords"].pop(i)
        rebuild_input_popovers()
        sync_surface(buttons, styler)

    def start_recording(edit_index: int | None, seed: str | None = None) -> None:
        rec["active"] = True
        rec["edit_index"] = edit_index
        rec["recorded"] = list(state["chords"][edit_index]["members"]) if edit_index is not None else ([seed] if seed else [])
        rec["working"] = dict(state["chords"][edit_index]) if edit_index is not None else new_working_binding()
        refresh_banner()

    def schedule_auto_finish() -> None:
        if rec["timer"] is not None:
            GLib.source_remove(rec["timer"])
            rec["timer"] = None
        if len(rec["recorded"]) >= 2:
            rec["timer"] = GLib.timeout_add(1200, do_auto_finish)

    def do_auto_finish():
        rec["timer"] = None
        if rec["active"] and len(rec["recorded"]) >= 2:
            finish_recording()
        return False

    def cancel_recording() -> None:
        if rec["timer"] is not None:
            GLib.source_remove(rec["timer"])
            rec["timer"] = None
        rec["active"] = False
        rec["recorded"] = []
        rec["edit_index"] = None
        banner.set_visible(False)
        sync_surface(buttons, styler)

    def refresh_banner(note: str | None = None) -> None:
        sync_surface(buttons, styler)
        clear_children(banner)
        banner.set_visible(True)
        row = Gtk.Box(spacing=6)
        row.append(
            Gtk.Label(
                label=f"Recording Chord… {chord_members_text(rec['recorded']) or '(click Inputs on the device)'}",
                hexpand=True,
                xalign=0,
            )
        )
        gear = Gtk.MenuButton(label="⚙")
        gp = Gtk.Popover()
        gp.set_child(build_debug_slider(state))
        gear.set_popover(gp)
        row.append(gear)
        cancel_btn = Gtk.Button(label="Cancel")
        cancel_btn.connect("clicked", lambda b: cancel_recording())
        row.append(cancel_btn)
        banner.append(row)
        if note:
            banner.append(Gtk.Label(label=note, xalign=0, wrap=True, css_classes=["digital-note"]))
        banner.append(
            Gtk.Label(
                label="Auto-finishes ~1.2s after your last change — no explicit Done needed.",
                xalign=0,
                css_classes=["dim"],
            )
        )

    def finish_recording() -> None:
        clear_children(banner)
        banner.append(
            Gtk.Label(
                label=("Editing Chord: " if rec["edit_index"] is not None else "New Chord: ")
                + chord_members_text(rec["recorded"]),
                xalign=0,
                css_classes=["heading"],
            )
        )
        banner.append(build_chord_binding_editor(rec["working"]))
        btn_row = Gtk.Box(spacing=8)

        def on_save(b):
            chord = dict(rec["working"])
            chord["members"] = list(rec["recorded"])
            if rec["edit_index"] is not None:
                state["chords"][rec["edit_index"]] = chord
            else:
                state["chords"].append(chord)
            rec["active"] = False
            rec["recorded"] = []
            rec["edit_index"] = None
            banner.set_visible(False)
            rebuild_input_popovers()
            sync_surface(buttons, styler)

        save_btn = Gtk.Button(label="Save", css_classes=["suggested-action"])
        save_btn.connect("clicked", on_save)
        btn_row.append(save_btn)
        discard_btn = Gtk.Button(label="Discard")
        discard_btn.connect("clicked", lambda b: cancel_recording())
        btn_row.append(discard_btn)
        banner.append(btn_row)

    rebuild_input_popovers()
    sync_surface(buttons, styler)

    box.append(
        Gtk.Label(
            label="Fold-in note: 'Add to Chord…'/'Part of Chord' rows land in the real "
            "per-Input popover (binding_editor.py's Save/Clear row), not a second popover "
            "bolted on beside it like this prototype's stand-in.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )
    return box


# --- Variant C: persistent Chords sidebar, recording plays out on the main
# grid itself, overlap prevented structurally (claimed Inputs disabled) ---


def build_variant_c(state: dict) -> Gtk.Widget:
    root = Gtk.Box(spacing=16)

    left = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    left.append(Gtk.Label(label="Device Overview", xalign=0, css_classes=["heading"]))
    status_lbl = Gtk.Label(xalign=0, wrap=True)
    left.append(status_lbl)

    rec = {"active": False, "recorded": [], "edit_index": None}
    working: dict = {}

    def styler(inp):
        claimed = is_claimed(state, inp, exclude=rec["edit_index"])
        classes = []
        sensitive = True
        tooltip = None
        if inp in rec["recorded"]:
            classes.append("device-btn-recorded")
        if rec["active"] and claimed:
            classes.append("device-btn-claimed")
            sensitive = False
            tooltip = chord_owner_tooltip(state, inp, exclude=rec["edit_index"])
        elif not rec["active"] and claimed:
            tooltip = chord_owner_tooltip(state, inp)
        return classes, sensitive, tooltip

    def on_click(inp):
        if not rec["active"]:
            return  # this variant's entry point is the sidebar, not the grid
        if inp in rec["recorded"]:
            rec["recorded"].remove(inp)
        else:
            rec["recorded"].append(inp)
        render_detail()
        refresh()

    surface, buttons = build_device_surface(on_click)
    left.append(surface)
    root.append(left)

    sidebar = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6, css_classes=["sidebar"])
    sidebar.set_size_request(260, -1)
    sidebar.append(Gtk.Label(label="Chords", xalign=0, css_classes=["heading"]))
    list_box = Gtk.ListBox(css_classes=["boxed-list"])
    sidebar.append(list_box)
    detail_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    sidebar.append(detail_box)
    new_btn = Gtk.Button(label="+ Record new Chord")
    sidebar.append(new_btn)
    sidebar.append(Gtk.Separator())
    sidebar.append(build_debug_slider(state))
    root.append(sidebar)

    def refresh() -> None:
        sync_surface(buttons, styler)
        status_lbl.set_label(
            f"Listening — {len(rec['recorded'])} pressed. Click more, or confirm below."
            if rec["active"]
            else "Click '+ Record new Chord' below, or select an existing Chord to edit it."
        )
        clear_children(list_box)
        if not state["chords"]:
            hint_row = Gtk.Box(spacing=6)
            hint_row.append(Gtk.Label(label="No Chords yet.", hexpand=True, xalign=0))
            try_btn = Gtk.Button(label="Try: ← + ↑ diagonal")
            try_btn.connect("clicked", lambda b: seed_diagonal())
            hint_row.append(try_btn)
            list_box.append(hint_row)
        for i, ch in enumerate(state["chords"]):
            row = Gtk.Box(spacing=6)
            row.append(
                Gtk.Label(
                    label=f"{chord_members_text(ch['members'])} → {chord_action_text(ch)}",
                    hexpand=True,
                    xalign=0,
                )
            )
            edit_btn = Gtk.Button(label="Edit")
            edit_btn.connect("clicked", lambda b, i=i: open_detail(i))
            row.append(edit_btn)
            del_btn = Gtk.Button(label="×")
            del_btn.connect("clicked", lambda b, i=i: delete_chord(i))
            row.append(del_btn)
            list_box.append(row)

    def delete_chord(i: int) -> None:
        state["chords"].pop(i)
        refresh()

    def open_detail(index: int | None, keep_recorded: bool = False) -> None:
        nonlocal working
        rec["edit_index"] = index
        if not keep_recorded:
            rec["recorded"] = list(state["chords"][index]["members"]) if index is not None else []
        rec["active"] = True
        working = dict(state["chords"][index]) if index is not None else new_working_binding()
        render_detail()
        refresh()

    def seed_diagonal() -> None:
        rec["recorded"] = ["thumbstick_left", "thumbstick_up"]
        open_detail(None, keep_recorded=True)

    def render_detail() -> None:
        clear_children(detail_box)
        if not rec["active"]:
            return
        detail_box.append(
            Gtk.Label(
                label="Editing:" if rec["edit_index"] is not None else "New Chord:",
                xalign=0,
                css_classes=["heading"],
            )
        )
        chip_row = Gtk.Box(spacing=4)
        for m in rec["recorded"]:
            item = Gtk.Box(spacing=2)
            item.append(Gtk.Label(label=input_label(m)))
            rm = Gtk.Button(label="×")

            def on_rm(b, m=m):
                rec["recorded"].remove(m)
                render_detail()
                refresh()

            rm.connect("clicked", on_rm)
            item.append(rm)
            chip_row.append(item)
        detail_box.append(chip_row)
        detail_box.append(
            Gtk.Label(
                label="Click more Inputs on the device to add them — already-claimed keys "
                "are disabled, not just warned about.",
                xalign=0,
                wrap=True,
                css_classes=["dim"],
            )
        )
        detail_box.append(build_chord_binding_editor(working))
        btn_row = Gtk.Box(spacing=8)

        def on_confirm(b):
            if len(rec["recorded"]) < 2:
                return
            chord = dict(working)
            chord["members"] = list(rec["recorded"])
            if rec["edit_index"] is not None:
                state["chords"][rec["edit_index"]] = chord
            else:
                state["chords"].append(chord)
            rec["active"] = False
            render_detail()
            refresh()

        confirm_btn = Gtk.Button(label="Confirm Chord", css_classes=["suggested-action"])
        confirm_btn.connect("clicked", on_confirm)
        btn_row.append(confirm_btn)

        def on_cancel(b):
            rec["active"] = False
            render_detail()
            refresh()

        cancel_btn = Gtk.Button(label="Cancel")
        cancel_btn.connect("clicked", on_cancel)
        btn_row.append(cancel_btn)
        detail_box.append(btn_row)

    new_btn.connect("clicked", lambda b: open_detail(None))

    refresh()

    right_note = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    right_note.append(
        Gtk.Label(
            label="Fold-in note: greying out claimed Inputs needs the Chord list available "
            "wherever Device Overview renders (already true — GetConfig() would carry it), "
            "so this is free once the sidebar's own data is wired up.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )
    sidebar.append(right_note)

    return root


# --- Switcher chrome ---

VARIANTS = [
    ("A", "Click grid directly · Binding → dialog only · structural overlap (round 2)"),
    ("B", "Per-Input popover · banner · auto-finish"),
    ("C", "Chords sidebar · structural overlap prevention"),
]


def build_window(app: Gtk.Application) -> Gtk.ApplicationWindow:
    provider = Gtk.CssProvider()
    provider.load_from_data(CSS.encode())
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
    )

    win = Gtk.ApplicationWindow(application=app, title="Ticket 30 prototype — Chord recording UX")
    win.set_default_size(560, 760)

    state = {"chords": make_seed_chords(), "window_ms": 50}

    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
    win.set_child(outer)

    content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    for setter in (content.set_margin_top, content.set_margin_bottom, content.set_margin_start, content.set_margin_end):
        setter(8)
    outer.append(content)

    index = {"i": 0}

    def render():
        clear_children(content)
        key, label = VARIANTS[index["i"]]
        if key == "A":
            content.append(build_variant_a(state, win))
        elif key == "B":
            content.append(build_variant_b(state))
        else:
            content.append(build_variant_c(state))
        variant_label.set_label(f"{key} — {label}")

    switcher = Gtk.Box(spacing=8, halign=Gtk.Align.CENTER)
    switcher.add_css_class("switcher-pill")
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
    app = Gtk.Application(application_id="com.acheron.prototype.ticket30")
    app.connect("activate", lambda a: build_window(a).present())
    app.run(None)


if __name__ == "__main__":
    main()
