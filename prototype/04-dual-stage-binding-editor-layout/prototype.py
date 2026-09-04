#!/usr/bin/env python3
"""
PROTOTYPE — throwaway code, not production. Standalone GTK4, nothing persists
across runs, no D-Bus. One deliberate exception to "don't import from real
Acheron code": it imports the real `key_picker`/`controller_picker` modules
(see the sys.path note below) so the layout is judged against the pickers'
actual size, not a stand-in — Charon asked to see them live.

Answers: what should the binding-editor layout look like for a grid key that
carries a deep (second) Actuation stage?
Ticket: .scratch/tartarus-dual-stage-keys/issues/04-dual-stage-binding-editor-layout.md

The one hard constraint every variant must satisfy (Q12): the key/controller-
button picker is large (see the real `gui/acheron_gui/key_picker.py` — a full
inline keyboard grid, always shown, no collapse) — never two of them on
screen at once.

Three *structurally* different answers to "how does the deep stage's Binding
get edited without a second picker visible":

  A — Swap toggle.  One flat panel. A shared 4-marker Actuation bar (primary
      green/amber + deep in a second colour pair, order-constrained) sits on
      top; a "Primary stage / Deep stage" toggle underneath swaps which
      stage's Trigger/Action fields render in a single shared slot below.
      Only one stage's fields (and so only one picker) ever exist in the
      tree.

  B — Stacked bars + accordion.  Two separate 2-marker bars, one per stage,
      stacked with the staging-mode selector between them. Each stage has an
      always-visible one-line summary row; clicking it expands a disclosure
      holding that stage's full fields. The two disclosures are a mutually-
      exclusive group — opening one collapses the other — so the picker
      constraint falls out of the accordion group for free.

  C — Tabs.  A single shared 4-marker bar + staging-mode row up top (same
      widget as A), then a `Gtk.Stack`/`Gtk.StackSwitcher` split into
      "Primary" / "Deep" tabs, each holding that stage's full fields. A
      `Gtk.Stack` only ever draws one child, so the constraint is structural
      — enforced by the widget, not by hand-written mutual exclusion.

Simulation controls (top bar, clearly separate from the design being judged):
"Primary bound" (Q6 — no primary, no deep-stage affordance) and "Digital
capture" (Q7 — deep-stage controls grey with a "requires analog" note,
mirroring the real Actuation section's existing digital-mode behaviour).

Switcher (bottom bar): Left/Right arrows or the ← / → keys cycle A → B → C.

Run:
    python3 prototype/04-dual-stage-binding-editor-layout/prototype.py
"""

from __future__ import annotations

import sys
from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gtk  # noqa: E402

# Deliberate exception to "don't import from real Acheron code" (Charon,
# reviewing this prototype live): the whole point of this round is to see
# the *real* key/controller-button pickers at their actual size inside each
# layout, not a stand-in. `gui/` (not `gui/acheron_gui/`) goes on the path —
# that's what the installed launcher puts on `PYTHONPATH` too
# (`gui/acheron_gui/__main__.py`), so `acheron_gui`'s own internal relative
# imports resolve the same way here as they do for real.
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "gui"))
from acheron_gui.controller_picker import LABEL_BY_CODE as CONTROLLER_LABEL_BY_CODE  # noqa: E402
from acheron_gui.controller_picker import build_inline_controller_picker  # noqa: E402
from acheron_gui.key_picker import LABEL_BY_CODE as KEY_LABEL_BY_CODE  # noqa: E402
from acheron_gui.key_picker import build_inline_key_picker  # noqa: E402

# --------------------------------------------------------------------------
# Shared data/logic (not layout) — fair game to share across variants per
# the prototype skill's own "a shared <Header> is fine" allowance.
# --------------------------------------------------------------------------

STAGING_MODES = [
    (
        "handoff",
        "Handoff",
        "Crossing into the deep band releases the primary stage and presses "
        "the deep stage; crossing back releases deep and re-presses primary. "
        "Exactly one held at a time (camera-shutter model).",
    ),
    (
        "no_return",
        "No-Return",
        "Like Handoff going deeper — but on the way back up, the primary "
        "does not re-fire.",
    ),
    (
        "additive",
        "Additive",
        "Both stages fire and stay held together — the deeper press adds "
        "the deep stage, it does not release the primary.",
    ),
    (
        "quick_skip",
        "Quick-Skip",
        "Deep band reached within ~50ms of the primary Actuation point: the "
        "primary's Down is suppressed entirely. Otherwise the primary fires "
        "(up to 50ms late) and the key behaves as Handoff for the rest of "
        "the press.",
    ),
]
STAGING_MODE_LABEL = {k: lbl for k, lbl, _ in STAGING_MODES}

TRIGGER_OPTIONS = [("hold_to_repeat", "Hold-to-repeat"), ("fire_once", "Fire-once"), ("toggle", "Toggle")]
TRIGGER_LABEL = dict(TRIGGER_OPTIONS)

# Three Action kinds: Keypress and Controller Button are the two kinds the
# real editor mounts a picker for (the thing Q12 is actually about — see
# the real `build_action_and_trigger_fields` in `binding_editor.py` for the
# full matrix, which also has Macro/Step/Axis; irrelevant to this layout
# question, left out here).
ACTION_KINDS = [("keypress", "Keypress"), ("controller_button", "Controller Button"), ("profile_switch", "Profile Switch")]


def fake_action_summary(binding: dict | None) -> str:
    if binding is None:
        return "— unbound —"
    if binding["type"] == "keypress":
        raw = binding["key"]
        key = KEY_LABEL_BY_CODE.get(raw, raw) if raw.startswith("BTN_") else raw.replace("KEY_", "")
        return f"{key}  [{TRIGGER_LABEL[binding['trigger']]}]"
    if binding["type"] == "controller_button":
        button = CONTROLLER_LABEL_BY_CODE.get(binding["button"], binding["button"])
        return f"Btn: {button}  [{TRIGGER_LABEL[binding['trigger']]}]"
    return f"→ {binding['target']}  [{TRIGGER_LABEL[binding['trigger']]}]"


def build_stage_fields(binding: dict, on_change, is_deep: bool = False) -> Gtk.Widget:
    """The Trigger/Action editor for one stage — same shape
    `build_action_and_trigger_fields` has in the real editor (Trigger
    dropdown, Action-kind dropdown, then a kind-specific field), and the
    *real* Key/Controller-button pickers (Charon asked to see them at their
    actual size, not a stand-in — see the sys.path note up top). `is_deep`
    only tints the picker's own "currently selected" highlight — see the
    `.deep-picker` CSS rule — to the same blue as the deep-actuation marker,
    so which stage's picker you're looking at is unambiguous at a glance."""
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)

    trigger_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in TRIGGER_OPTIONS]))
    trigger_dd.set_selected([k for k, _ in TRIGGER_OPTIONS].index(binding["trigger"]))

    def on_trigger(dd, *_):
        binding["trigger"] = TRIGGER_OPTIONS[dd.get_selected()][0]
        on_change()

    trigger_dd.connect("notify::selected", on_trigger)
    box.append(_labeled("Trigger mode", trigger_dd))

    kind_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in ACTION_KINDS]))
    kind_dd.set_selected([k for k, _ in ACTION_KINDS].index(binding["type"]))

    def on_kind(dd, *_):
        kind = ACTION_KINDS[dd.get_selected()][0]
        binding["type"] = kind
        if kind == "keypress" and "key" not in binding:
            binding["key"] = "KEY_A"
        if kind == "controller_button" and "button" not in binding:
            binding["button"] = "BTN_SOUTH"
        if kind == "profile_switch" and "target" not in binding:
            binding["target"] = "Racing"
        on_change()

    kind_dd.connect("notify::selected", on_kind)
    box.append(_labeled("Action", kind_dd))

    if binding["type"] == "keypress":
        def on_key_changed(code: str) -> None:
            binding["key"] = code
            on_change()

        key_picker, _refresh_warning = build_inline_key_picker(binding.get("key", "KEY_A"), on_key_changed)
        if is_deep:
            key_picker.add_css_class("deep-picker")
        box.append(_labeled("Key", key_picker))
    elif binding["type"] == "controller_button":
        def on_button_changed(code: str) -> None:
            binding["button"] = code
            on_change()

        controller_picker = build_inline_controller_picker(binding.get("button", "BTN_SOUTH"), on_button_changed)
        if is_deep:
            controller_picker.add_css_class("deep-picker")
        box.append(_labeled("Button", controller_picker))
    else:
        target_dd = Gtk.DropDown(model=Gtk.StringList.new(["Racing", "FPS", "Sim"]))
        targets = ["Racing", "FPS", "Sim"]
        target_dd.set_selected(targets.index(binding.get("target", "Racing")))

        def on_target(dd, *_):
            binding["target"] = targets[dd.get_selected()]
            on_change()

        target_dd.connect("notify::selected", on_target)
        box.append(_labeled("Target Profile", target_dd))

    return box


def _labeled(label: str, widget: Gtk.Widget) -> Gtk.Box:
    row = Gtk.Box(spacing=8)
    lbl = Gtk.Label(label=label, xalign=0)
    lbl.set_size_request(100, -1)
    row.append(lbl)
    widget.set_hexpand(True)
    row.append(widget)
    return row


# --------------------------------------------------------------------------
# Ordered-marker Actuation bar — trimmed down from the real `DepthTrack`
# (`binding_editor.py:121`, ticket 26): no live Depth fill (no Daemon here),
# no resize-resync timer. Generalises the real 2-marker collision logic
# (`binding_editor.py:298`) to N markers under one strict ordering.
# --------------------------------------------------------------------------

# Fixed, not hexpand — see `OrderedMarkerTrack`'s own docstring for why live
# width caused jumpy dragging. 680px is deliberately not a round guess: it's
# the real `_labeled("Key", key_picker)` row's own natural width — measured
# via `Gtk.Widget.measure()` against the actual `key_picker.
# build_inline_key_picker` output (573px natural, the widest of the two real
# pickers — `build_inline_controller_picker` measures 440px) plus
# `_labeled`'s 100px label column + 8px spacing (573+100+8=681, rounded
# down by 1px). That row is what already forces the window this wide (see
# `_max_scroller_height`'s ScrolledWindow, `hscrollbar_policy=NEVER`) — the
# bar matches it exactly rather than tracking the window live.
_TRACK_WIDTH = 680


def enforce_order(values: dict, order: list[str], moved_key: str, new_value: int) -> None:
    """Keeps `values[k]` strictly increasing along `order` (Q4's disjoint,
    stacked bands: p_rel < p_act < d_rel < d_act) by pushing the chain of
    neighbours the moved marker collides with — same idea as the real
    2-marker `on_moved`, generalised to N."""
    values[moved_key] = max(0, min(255, new_value))
    i = order.index(moved_key)
    for j in range(i + 1, len(order)):
        if values[order[j]] <= values[order[j - 1]]:
            values[order[j]] = min(255, values[order[j - 1]] + 1)
    for j in range(i - 1, -1, -1):
        if values[order[j]] >= values[order[j + 1]]:
            values[order[j]] = max(0, values[order[j + 1]] - 1)


class OrderedMarkerTrack(Gtk.Overlay):
    def __init__(self, marker_specs: list[dict], get_value, on_marker_moved, on_settle, height: int = 16):
        super().__init__()
        self.marker_specs = marker_specs
        self.get_value = get_value
        self.on_marker_moved = on_marker_moved
        self.on_settle = on_settle
        self.height = height
        # Fixed width, not hexpand+live get_width() — tried that (to track
        # the window's own width live) but a container's layout can take a
        # couple of frames to settle, so a `get_width()` read inside a drag
        # handler doesn't reliably match the width the *previous*
        # `sync_markers()` call placed markers with, and even the real
        # `DepthTrack`'s 200ms-resync-while-mapped mitigation (ported here
        # for that attempt) wasn't enough to stop the jump. Fixing the width
        # outright removes the mismatch instead of chasing it — `_TRACK_
        # WIDTH` is tuned to land at the same width anyway (see its own
        # comment).
        self.set_size_request(_TRACK_WIDTH, height)
        self.set_hexpand(False)
        self.set_halign(Gtk.Align.START)

        bg = Gtk.Box(css_classes=["depth-track-bg"], hexpand=False)
        bg.set_size_request(_TRACK_WIDTH, height)
        self.set_child(bg)

        self.marker_widgets: list[Gtk.Box] = []
        for spec in marker_specs:
            mw = Gtk.Box(css_classes=[spec["css"]], halign=Gtk.Align.START, valign=Gtk.Align.FILL)
            mw.set_size_request(3, height)
            self.add_overlay(mw)
            self.marker_widgets.append(mw)

        drag = Gtk.GestureDrag()
        drag.connect("drag-begin", self._on_drag_begin)
        drag.connect("drag-update", self._on_drag_update)
        drag.connect("drag-end", self._on_drag_end)
        self.add_controller(drag)
        self._drag_index: int | None = None
        self._drag_start_value = 0
        self.connect("map", lambda *_: self.sync_markers())

    def _track_width(self) -> int:
        return _TRACK_WIDTH

    def sync_markers(self) -> None:
        for spec, mw in zip(self.marker_specs, self.marker_widgets):
            mw.set_margin_start(round(self._value_to_x(self.get_value(spec["key"])) - 1))

    def _x_to_value(self, x: float) -> int:
        return max(0, min(255, round(x / self._track_width() * 255)))

    def _value_to_x(self, value: int) -> float:
        return value / 255 * self._track_width()

    def _on_drag_begin(self, gesture, start_x, start_y):
        nearest, best = None, 1e9
        for i, spec in enumerate(self.marker_specs):
            if not spec.get("draggable"):
                continue
            d = abs(self._value_to_x(self.get_value(spec["key"])) - start_x)
            if d < best:
                nearest, best = i, d
        self._drag_index = nearest
        if nearest is not None:
            self._drag_start_value = self.get_value(self.marker_specs[nearest]["key"])

    def _on_drag_update(self, gesture, offset_x, offset_y):
        if self._drag_index is None:
            return
        spec = self.marker_specs[self._drag_index]
        start_x = self._value_to_x(self._drag_start_value)
        new_value = self._x_to_value(start_x + offset_x)
        self.on_marker_moved(spec["key"], new_value)
        self.sync_markers()

    def _on_drag_end(self, gesture, offset_x, offset_y):
        if self._drag_index is None:
            return
        self._drag_index = None
        self.on_settle()


def make_grey_overlay(bar_or_section: Gtk.Widget, note: str, digital: bool) -> Gtk.Widget:
    """Mirrors the real Actuation section's digital-mode treatment
    (`apply_mode`, `binding_editor.py:422`): grey the section, centre a
    note over it."""
    overlay = Gtk.Overlay(hexpand=True)
    overlay.set_child(bar_or_section)
    if digital:
        bar_or_section.add_css_class("depth-track-dim")
        bar_or_section.set_sensitive(False)
        note_label = Gtk.Label(
            label=note, wrap=True, halign=Gtk.Align.CENTER, valign=Gtk.Align.CENTER, css_classes=["digital-note"]
        )
        overlay.add_overlay(note_label)
    return overlay


def build_staging_mode_row(current: str, on_pick, sensitive: bool = True) -> Gtk.Widget:
    box = Gtk.Box(spacing=4, css_classes=["staging-mode-row"])
    group_leader = None
    for key, label, tip in STAGING_MODES:
        btn = Gtk.ToggleButton(label=label, tooltip_text=tip)
        if group_leader is None:
            group_leader = btn
        else:
            btn.set_group(group_leader)
        btn.set_active(key == current)
        btn.set_sensitive(sensitive)
        btn.connect("toggled", lambda b, k=key: on_pick(k) if b.get_active() else None)
        box.append(btn)
    return box


def build_marker_legend(has_deep: bool) -> Gtk.Label:
    """Colour-swatch legend for the combined bar — actual colour chips via
    Pango markup (`use_markup=True`), same trick the real `marker-legend`
    label already uses (`binding_editor.py:332`), not the colour name as
    plain text."""
    # Same left-to-right order the bar itself enforces (Q4's disjoint,
    # stacked bands — `marker_order()`/`enforce_order()`): p_rel < p_act <
    # d_rel < d_act.
    legend = Gtk.Label(xalign=0, use_markup=True, css_classes=["marker-legend", "dim"])
    markup = (
        '<span foreground="#e6991a">■</span> primary release   '
        '<span foreground="#2ecc71">■</span> primary actuation'
    )
    if has_deep:
        markup += (
            '     <span foreground="#9b59b6">■</span> deep release   '
            '<span foreground="#3498db">■</span> deep actuation'
        )
    legend.set_markup(markup)
    return legend


# --------------------------------------------------------------------------
# Shared simulation state — a stand-in for one grid key's Profile-level
# config. `deep` is `None` until "+ Add deep stage"; `primary` is `None`
# when the "Primary bound" simulation toggle is off (Q6 gate).
# --------------------------------------------------------------------------

STATE = {
    "capture_mode": "analog",  # "analog" | "digital" — Q7 simulation
    "primary": {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_W"},
    "primary_actuation": {"actuation": 170, "release": 130},
    "deep": None,  # {"binding": {...}, "actuation": .., "release": .., "mode": ..}
}
UI_STATE = {"variant": "A", "a_editing": "primary", "b_open": None, "c_page": "primary"}


def marker_order(state: dict) -> list[str]:
    order = ["p_rel", "p_act"]
    if state["deep"] is not None:
        order += ["d_rel", "d_act"]
    return order


def marker_value(state: dict, key: str) -> int:
    if key == "p_rel":
        return state["primary_actuation"]["release"]
    if key == "p_act":
        return state["primary_actuation"]["actuation"]
    if key == "d_rel":
        return state["deep"]["release"]
    return state["deep"]["actuation"]


def set_marker_value(state: dict, key: str, value: int) -> None:
    if key == "p_rel":
        state["primary_actuation"]["release"] = value
    elif key == "p_act":
        state["primary_actuation"]["actuation"] = value
    elif key == "d_rel":
        state["deep"]["release"] = value
    else:
        state["deep"]["actuation"] = value


def on_marker_moved(state: dict, key: str, new_value: int) -> None:
    values = {k: marker_value(state, k) for k in marker_order(state)}
    enforce_order(values, marker_order(state), key, new_value)
    for k, v in values.items():
        set_marker_value(state, k, v)


def add_deep_stage(state: dict) -> None:
    p_act = state["primary_actuation"]["actuation"]
    d_rel = min(255, p_act + 20)
    d_act = min(255, d_rel + 35)
    state["deep"] = {
        "binding": {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_LEFTSHIFT"},
        "release": d_rel,
        "actuation": d_act,
        "mode": "handoff",
    }


def remove_deep_stage(state: dict) -> None:
    state["deep"] = None


def set_primary_bound(state: dict, bound: bool) -> None:
    if bound and state["primary"] is None:
        state["primary"] = {"trigger": "hold_to_repeat", "type": "keypress", "key": "KEY_W"}
    elif not bound and state["primary"] is not None:
        state["primary"] = None
        if state["deep"] is not None:
            print("  (cascade: clearing primary also clears the deep stage — Q6)")
            state["deep"] = None


# --------------------------------------------------------------------------
# Variant A — swap toggle. One flat panel, one shared 4-marker bar, one
# "Primary stage / Deep stage" toggle swapping a single editor_slot below.
# --------------------------------------------------------------------------

def build_variant_a(state: dict, on_change) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)

    header = Gtk.Box(spacing=6)
    header.append(Gtk.Label(label="Actuation & release", css_classes=["sub-heading"]))
    header.append(Gtk.Label(label=state["capture_mode"], css_classes=["badge"]))
    box.append(header)

    digital = state["capture_mode"] == "digital"
    specs = [{"key": "p_act", "css": "marker-actuation", "draggable": True},
             {"key": "p_rel", "css": "marker-release", "draggable": True}]
    if state["deep"] is not None:
        specs += [{"key": "d_act", "css": "marker-deep-actuation", "draggable": True},
                  {"key": "d_rel", "css": "marker-deep-release", "draggable": True}]
    track = OrderedMarkerTrack(
        specs,
        get_value=lambda k: marker_value(state, k),
        on_marker_moved=lambda k, v: on_marker_moved(state, k, v),
        on_settle=on_change,
    )
    box.append(make_grey_overlay(track, "No depth — analog capture unavailable", digital))
    box.append(build_marker_legend(has_deep=state["deep"] is not None))

    box.append(Gtk.Separator())

    if state["primary"] is None:
        box.append(Gtk.Label(label="Bind a primary Action first to enable a deep stage.", xalign=0, wrap=True))
        return box

    # Primary/Deep row: the Deep toggle and its Remove (✕) button only exist
    # once a deep stage does; until then, "+ Add deep stage" sits in the
    # Deep toggle's own spot.
    edit_row = Gtk.Box(spacing=6)
    primary_toggle = Gtk.ToggleButton(label=f"Primary — {fake_action_summary(state['primary'])}")
    primary_toggle.set_active(UI_STATE["a_editing"] == "primary")
    edit_row.append(primary_toggle)

    if state["deep"] is None:
        add_btn = Gtk.Button(label="+ Add deep stage")

        def on_add(b):
            add_deep_stage(state)
            UI_STATE["a_editing"] = "deep"
            on_change()

        add_btn.connect("clicked", on_add)
        edit_row.append(add_btn)
    else:
        deep_toggle = Gtk.ToggleButton(label=f"Deep — {fake_action_summary(state['deep']['binding'])}")
        deep_toggle.set_group(primary_toggle)
        deep_toggle.set_active(UI_STATE["a_editing"] == "deep")
        edit_row.append(deep_toggle)

        def on_deep_toggled(b):
            if b.get_active():
                UI_STATE["a_editing"] = "deep"
                on_change()

        deep_toggle.connect("toggled", on_deep_toggled)

        remove_btn = Gtk.Button(
            label="✕", tooltip_text="Remove deep stage", css_classes=["destructive-action", "icon-btn"]
        )

        def on_remove(b):
            remove_deep_stage(state)
            UI_STATE["a_editing"] = "primary"
            on_change()

        remove_btn.connect("clicked", on_remove)
        edit_row.append(remove_btn)

    def on_primary_toggled(b):
        if b.get_active():
            UI_STATE["a_editing"] = "primary"
            on_change()

    primary_toggle.connect("toggled", on_primary_toggled)
    box.append(edit_row)

    # Staging mode: only shown once a deep stage exists — it has nothing to
    # hand off between until then.
    if state["deep"] is not None:
        box.append(Gtk.Label(label="Staging mode", xalign=0, css_classes=["sub-heading"]))

        def on_mode(k):
            state["deep"]["mode"] = k
            on_change()

        mode_row = build_staging_mode_row(state["deep"]["mode"], on_mode, sensitive=not digital)
        box.append(make_grey_overlay(mode_row, "Requires analog capture", digital))

    editing = UI_STATE["a_editing"] if state["deep"] is not None else "primary"
    editor_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    if editing == "primary":
        editor_slot.append(build_stage_fields(state["primary"], on_change))
    else:
        if digital:
            editor_slot.append(Gtk.Label(
                label="Deep stage is inert in Digital capture mode — requires analog.",
                xalign=0, wrap=True, css_classes=["dim"],
            ))
        editor_slot.append(build_stage_fields(state["deep"]["binding"], on_change, is_deep=True))
    box.append(editor_slot)

    return box


# --------------------------------------------------------------------------
# Variant B — stacked bars + mutually-exclusive accordion. Two 2-marker
# bars, staging-mode selector between them, a summary row per stage that
# expands into that stage's fields; opening one disclosure closes the other.
# --------------------------------------------------------------------------

def build_variant_b(state: dict, on_change) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    digital = state["capture_mode"] == "digital"

    def stage_section(label: str, is_deep: bool) -> Gtk.Widget:
        section = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        section.append(Gtk.Label(label=label, xalign=0, css_classes=["sub-heading"]))

        specs = (
            [{"key": "d_act", "css": "marker-deep-actuation", "draggable": True},
             {"key": "d_rel", "css": "marker-deep-release", "draggable": True}]
            if is_deep else
            [{"key": "p_act", "css": "marker-actuation", "draggable": True},
             {"key": "p_rel", "css": "marker-release", "draggable": True}]
        )
        track = OrderedMarkerTrack(
            specs,
            get_value=lambda k: marker_value(state, k),
            on_marker_moved=lambda k, v: on_marker_moved(state, k, v),
            on_settle=on_change,
        )
        note = "Requires analog capture" if is_deep else "No depth — analog capture unavailable"
        section.append(make_grey_overlay(track, note, digital))

        binding = state["deep"]["binding"] if is_deep else state["primary"]
        open_key = "deep" if is_deep else "primary"
        summary_row = Gtk.Box(spacing=8)
        summary_row.append(Gtk.Label(label=fake_action_summary(binding), hexpand=True, xalign=0))
        toggle_btn = Gtk.Button(label="▾" if UI_STATE["b_open"] == open_key else "▸")
        summary_row.append(toggle_btn)
        section.append(summary_row)

        if UI_STATE["b_open"] == open_key:
            if is_deep and digital:
                section.append(Gtk.Label(
                    label="Deep stage is inert in Digital capture mode — requires analog.",
                    xalign=0, wrap=True, css_classes=["dim"],
                ))
            section.append(build_stage_fields(binding, on_change, is_deep=is_deep))
            if is_deep:
                remove_btn = Gtk.Button(label="Remove deep stage", css_classes=["destructive-action"])

                def on_remove(b):
                    remove_deep_stage(state)
                    UI_STATE["b_open"] = None
                    on_change()

                remove_btn.connect("clicked", on_remove)
                section.append(remove_btn)

        def on_toggle(b):
            UI_STATE["b_open"] = None if UI_STATE["b_open"] == open_key else open_key
            on_change()

        toggle_btn.connect("clicked", on_toggle)
        return section

    if state["primary"] is None:
        box.append(Gtk.Label(label="Bind a primary Action first to enable a deep stage.", xalign=0, wrap=True))
        return box

    box.append(stage_section("Primary stage", is_deep=False))

    if state["deep"] is None:
        add_btn = Gtk.Button(label="+ Add deep stage")

        def on_add(b):
            add_deep_stage(state)
            UI_STATE["b_open"] = "deep"
            on_change()

        add_btn.connect("clicked", on_add)
        box.append(add_btn)
    else:
        def on_mode(k):
            state["deep"]["mode"] = k
            on_change()

        mode_row = build_staging_mode_row(state["deep"]["mode"], on_mode, sensitive=not digital)
        box.append(make_grey_overlay(mode_row, "Requires analog capture", digital))
        box.append(stage_section("Deep stage", is_deep=True))

    return box


# --------------------------------------------------------------------------
# Variant C — tabs. Shared 4-marker bar + staging-mode row up top, then a
# Primary/Deep `Gtk.Stack` — the constraint is structural (a Stack only ever
# draws one child), not hand-written mutual exclusion.
# --------------------------------------------------------------------------

def build_variant_c(state: dict, on_change) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    digital = state["capture_mode"] == "digital"

    header = Gtk.Box(spacing=6)
    header.append(Gtk.Label(label="Actuation & release", css_classes=["sub-heading"]))
    header.append(Gtk.Label(label=state["capture_mode"], css_classes=["badge"]))
    box.append(header)

    specs = [{"key": "p_act", "css": "marker-actuation", "draggable": True},
             {"key": "p_rel", "css": "marker-release", "draggable": True}]
    if state["deep"] is not None:
        specs += [{"key": "d_act", "css": "marker-deep-actuation", "draggable": True},
                  {"key": "d_rel", "css": "marker-deep-release", "draggable": True}]
    track = OrderedMarkerTrack(
        specs,
        get_value=lambda k: marker_value(state, k),
        on_marker_moved=lambda k, v: on_marker_moved(state, k, v),
        on_settle=on_change,
    )
    box.append(make_grey_overlay(track, "No depth — analog capture unavailable", digital))

    if state["primary"] is None:
        box.append(Gtk.Label(label="Bind a primary Action first to enable a deep stage.", xalign=0, wrap=True))
        return box

    if state["deep"] is not None:
        def on_mode(k):
            state["deep"]["mode"] = k
            on_change()

        mode_row = build_staging_mode_row(state["deep"]["mode"], on_mode, sensitive=not digital)
        box.append(make_grey_overlay(mode_row, "Requires analog capture", digital))

    tab_row = Gtk.Box(spacing=4, css_classes=["linked"])
    primary_tab = Gtk.ToggleButton(label="Primary")
    primary_tab.set_active(UI_STATE["c_page"] == "primary")
    tab_row.append(primary_tab)
    deep_tab = Gtk.ToggleButton(label="Deep")
    deep_tab.set_group(primary_tab)
    deep_tab.set_active(UI_STATE["c_page"] == "deep")
    deep_tab.set_sensitive(state["deep"] is not None)
    if state["deep"] is None:
        deep_tab.set_tooltip_text("Add a deep stage first")
    tab_row.append(deep_tab)
    box.append(tab_row)

    def on_primary_tab(b):
        if b.get_active():
            UI_STATE["c_page"] = "primary"
            on_change()

    def on_deep_tab(b):
        if b.get_active():
            UI_STATE["c_page"] = "deep"
            on_change()

    primary_tab.connect("toggled", on_primary_tab)
    deep_tab.connect("toggled", on_deep_tab)

    page = UI_STATE["c_page"] if state["deep"] is not None else "primary"
    page_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8, css_classes=["boxed-list"])
    if page == "primary":
        page_slot.append(build_stage_fields(state["primary"], on_change))
    elif state["deep"] is None:
        add_btn = Gtk.Button(label="+ Add deep stage")

        def on_add(b):
            add_deep_stage(state)
            UI_STATE["c_page"] = "deep"
            on_change()

        add_btn.connect("clicked", on_add)
        page_slot.append(Gtk.Label(label="No deep stage yet.", xalign=0))
        page_slot.append(add_btn)
    else:
        if digital:
            page_slot.append(Gtk.Label(
                label="Deep stage is inert in Digital capture mode — requires analog.",
                xalign=0, wrap=True, css_classes=["dim"],
            ))
        page_slot.append(build_stage_fields(state["deep"]["binding"], on_change, is_deep=True))
        remove_btn = Gtk.Button(label="Remove deep stage", css_classes=["destructive-action"])

        def on_remove(b):
            remove_deep_stage(state)
            UI_STATE["c_page"] = "primary"
            on_change()

        remove_btn.connect("clicked", on_remove)
        page_slot.append(remove_btn)
    box.append(page_slot)

    return box


VARIANTS = {
    "A": ("Swap toggle", build_variant_a),
    "B": ("Stacked bars + accordion", build_variant_b),
    "C": ("Tabs", build_variant_c),
}
VARIANT_KEYS = list(VARIANTS.keys())


# The picker-specific rules below (.picker-panel through .padbtn-dpad) are
# copied verbatim from the real `gui/acheron_gui/app.py::CSS` — the real
# `key_picker`/`controller_picker` widgets carry these class names, and this
# is a standalone window with no other stylesheet to supply them. Everything
# else is this prototype's own.
CSS = """
.heading { font-weight: bold; font-size: 1.2em; }
.sub-heading { font-weight: bold; }
.dim { opacity: 0.65; font-size: smaller; }
.badge { border-radius: 999px; padding: 1px 8px; background-color: alpha(currentColor, 0.12); font-size: smaller; }
.depth-track-bg { background-color: alpha(currentColor, 0.15); border-radius: 4px; }
.depth-track-dim { opacity: 0.4; }
.marker-actuation { background-color: #2ecc71; }
.marker-release { background-color: #e6991a; }
.marker-deep-actuation { background-color: #3498db; }
.marker-deep-release { background-color: #9b59b6; }
.digital-note { background-color: alpha(currentColor, 0.08); border-radius: 4px; padding: 2px 8px; font-size: smaller; }
.destructive-action { color: #e64c4c; }
.icon-btn { min-width: 22px; min-height: 22px; padding: 0; font-weight: bold; }
.staging-mode-row button { font-size: smaller; }
.switcher-bar { background-color: alpha(currentColor, 0.10); border-radius: 999px; padding: 4px 10px; }
.sim-bar { background-color: alpha(#3498db, 0.10); border-radius: 6px; padding: 6px 10px; }
.picker-panel { padding: 8px; background-color: alpha(currentColor, 0.05); border-radius: 6px; }
.section-label { font-size: smaller; opacity: 0.65; font-weight: bold; }
.keycap { min-height: 25px; padding: 2px 4px; font-size: 12px; }
.keycap-mod { background-color: alpha(#4a90e2, 0.22); }
.keycap-mouse { background-color: alpha(#8e44ad, 0.22); }
.keycap-mm { background-color: alpha(#27ae60, 0.22); }
.pad-body { background-color: alpha(currentColor, 0.06); border-radius: 18px; }
.padbtn { min-width: 0; min-height: 0; font-size: 11px; padding: 2px 4px; }
.padbtn-face { background-color: alpha(#4a90e2, 0.22); }
.padbtn-shoulder { background-color: alpha(#e67e22, 0.22); }
.padbtn-stick { background-color: alpha(#8e44ad, 0.22); }
.padbtn-dpad { background-color: alpha(#27ae60, 0.22); }
/* Ticket 04: the deep stage's own picker (`.deep-picker`, set from
   `build_stage_fields(is_deep=True)`) highlights its current selection in
   the same blue as the deep-actuation marker (`.marker-deep-actuation`,
   #3498db) instead of the theme's generic `.suggested-action` accent — so
   which stage's picker you're looking at is unambiguous at a glance. */
.deep-picker .keycap.suggested-action,
.deep-picker .padbtn.suggested-action {
    /* The theme paints `.suggested-action` with its own accent
       `background-image` (a solid-colour `image()`, not a gradient) that
       sits on top of `background-color` and otherwise masks it regardless
       of provider priority — `background-image: none` clears that layer
       so the colour below actually shows. */
    background-image: none;
    background-color: #3498db;
    border-color: #3498db;
    color: white;
}
"""


def _max_scroller_height() -> int:
    """Screen height minus this window's own non-scrolled chrome (SIMULATE
    bar, heading, switcher, margins) and a compositor-decoration allowance
    — everything past this, the ScrolledWindow scrolls instead of the
    window growing off the bottom of the screen. Reads the Display's own
    monitor list rather than the window's surface, since this runs before
    the window is ever mapped (`rebuild()` fires before `win.present()`)."""
    display = Gdk.Display.get_default()
    if display is not None:
        monitors = display.get_monitors()
        if monitors.get_n_items() > 0:
            geometry = monitors.get_item(0).get_geometry()
            return max(240, geometry.height - 260)
    return 480


def build_window_content(rebuild) -> Gtk.Widget:
    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
    # A minimum, not a fixed, width — keeps the window from narrowing to an
    # awkward shape as the variant/content changes, without preventing it
    # growing wider for something that genuinely needs it (e.g. the real
    # picker's keyboard grid).
    outer.set_size_request(560, -1)
    outer.set_margin_top(10)
    outer.set_margin_bottom(10)
    outer.set_margin_start(10)
    outer.set_margin_end(10)

    # --- simulation controls (prototype-only, not part of any variant) ---
    sim_bar = Gtk.Box(spacing=12, css_classes=["sim-bar"])
    sim_bar.append(Gtk.Label(label="SIMULATE:", css_classes=["dim"]))

    primary_check = Gtk.CheckButton(label="Primary bound")
    primary_check.set_active(STATE["primary"] is not None)

    def on_primary_check(b):
        set_primary_bound(STATE, b.get_active())
        rebuild()

    primary_check.connect("toggled", on_primary_check)
    sim_bar.append(primary_check)

    digital_check = Gtk.CheckButton(label="Digital capture (deep stage inert)")
    digital_check.set_active(STATE["capture_mode"] == "digital")

    def on_digital_check(b):
        STATE["capture_mode"] = "digital" if b.get_active() else "analog"
        rebuild()

    digital_check.connect("toggled", on_digital_check)
    sim_bar.append(digital_check)
    outer.append(sim_bar)

    # --- heading, mirrors build_binding_editor's own heading row ---
    heading = Gtk.Label(label="Racing / Base / Grid r2c3", xalign=0, css_classes=["heading"])
    outer.append(heading)

    # --- the variant under test, scrolled like the real Actuation section ---
    # No `vexpand` here (unlike the real Actuation section's own scroller,
    # which sits inside an already-sized window) — this window has nothing
    # else claiming the remaining space, so an un-expanded, natural-height
    # ScrolledWindow lets the *window itself* grow/shrink with the content
    # (see `set_resizable(False)` below); `set_max_content_height` only
    # caps it once content would run past the screen edge, at which point
    # real scrolling takes over instead of the window growing further.
    scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER)
    scroller.set_max_content_height(_max_scroller_height())
    scroller.set_propagate_natural_height(True)
    _, builder = VARIANTS[UI_STATE["variant"]]
    scroller.set_child(builder(STATE, rebuild))
    outer.append(scroller)

    # --- switcher (prototype chrome, not part of any variant) ---
    switcher = Gtk.Box(spacing=10, halign=Gtk.Align.CENTER, css_classes=["switcher-bar"])
    prev_btn = Gtk.Button(label="◀")
    name, _ = VARIANTS[UI_STATE["variant"]]
    switch_label = Gtk.Label(label=f"{UI_STATE['variant']} — {name}")
    next_btn = Gtk.Button(label="▶")

    def cycle(delta):
        i = VARIANT_KEYS.index(UI_STATE["variant"])
        UI_STATE["variant"] = VARIANT_KEYS[(i + delta) % len(VARIANT_KEYS)]
        UI_STATE["a_editing"] = "primary"
        UI_STATE["b_open"] = None
        UI_STATE["c_page"] = "primary"
        rebuild()

    prev_btn.connect("clicked", lambda b: cycle(-1))
    next_btn.connect("clicked", lambda b: cycle(1))
    switcher.append(prev_btn)
    switcher.append(switch_label)
    switcher.append(next_btn)
    outer.append(switcher)

    return outer


class PrototypeApp(Gtk.Application):
    def __init__(self):
        super().__init__(application_id="com.acheron.prototype.dual_stage_binding_editor")

    def do_activate(self):
        provider = Gtk.CssProvider()
        provider.load_from_string(CSS)
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )

        win = Gtk.ApplicationWindow(application=self, title="Acheron — Dual-stage binding editor (ticket 04)")
        # Not resizable: a fixed-size top-level always sizes itself to its
        # content's natural size (recomputed on every `rebuild()`, since
        # content actually changes shape — e.g. adding a deep stage) rather
        # than keeping whatever height a manual drag left it at. The
        # ScrolledWindow's own `max_content_height` (`_max_scroller_height`)
        # is what actually bounds it at the screen edge — this just lets the
        # window follow that natural size instead of fighting it.
        win.set_resizable(False)

        root = Gtk.Box()

        def rebuild():
            child = root.get_first_child()
            while child is not None:
                nxt = child.get_next_sibling()
                root.remove(child)
                child = nxt
            root.append(build_window_content(rebuild))
            deep_desc = "none" if STATE["deep"] is None else f"mode={STATE['deep']['mode']}"
            print(
                f"=== variant={UI_STATE['variant']} capture_mode={STATE['capture_mode']} "
                f"primary={fake_action_summary(STATE['primary'])} deep=({deep_desc}) ==="
            )

        key_controller = Gtk.EventControllerKey()

        def on_key(controller, keyval, keycode, state_flags):
            name = Gdk.keyval_name(keyval)
            if name == "Left":
                i = VARIANT_KEYS.index(UI_STATE["variant"])
                UI_STATE["variant"] = VARIANT_KEYS[(i - 1) % len(VARIANT_KEYS)]
                rebuild()
                return True
            if name == "Right":
                i = VARIANT_KEYS.index(UI_STATE["variant"])
                UI_STATE["variant"] = VARIANT_KEYS[(i + 1) % len(VARIANT_KEYS)]
                rebuild()
                return True
            return False

        key_controller.connect("key-pressed", on_key)
        win.add_controller(key_controller)

        rebuild()
        win.set_child(root)
        win.present()


def main() -> None:
    app = PrototypeApp()
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
