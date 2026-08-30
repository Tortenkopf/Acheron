# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""The shared Binding editor — one component used from Device Overview's
per-key editor windows (ticket 09's resolved IA; the Action Table sidebar
that once also hosted it was cut outright in ticket 48). Trigger-mode/
Macro-step UI is preserved in full from the
prototype (the wire encoding already round-trips every `TriggerMode`/
`Action::Macro` shape, per ticket 15) even though only Keypress/Fire-once
actually fires in the Daemon yet (ticket 17) — matching ticket 16's
"Macro/other-Trigger-mode UI can exist inert" allowance.

A Binding here is the same *flat* dict `GetConfig()`/`SetBinding` use on the
wire (`{"trigger": ..., "type": ..., "key"/"steps": ...}`), not the ticket
09 prototype's nested `{"trigger": ..., "action": {...}}` shape — this
editor edits exactly what the Daemon will hand back on the next read.
"""

from __future__ import annotations

from typing import Callable

from gi.repository import Gtk, GLib

from .daemon_client import DaemonError
from .gtk_utils import build_name_prompt_popover, clear_children
from .inputs import (
    ACTION_TYPES,
    INPUT_DEFAULT_LABEL,
    TRIGGER_OPTIONS,
    TRIGGER_SHORT,
    default_trigger_for,
    input_label,
    is_grid_input,
)
from .axis_picker import AXIS_LABEL_BY_TARGET, build_inline_axis_picker
from .controller_picker import LABEL_BY_CODE as CONTROLLER_LABEL_BY_CODE
from .controller_picker import build_inline_controller_picker
from .key_picker import LABEL_BY_CODE, build_inline_key_picker


def action_summary(
    binding: dict | None, inp: str, macros: dict, steppers: dict | None = None, axis_target: str | None = None
) -> str:
    if axis_target is not None:
        # Ticket 71: an Axis-assigned Input never has a `binding` at all
        # (ticket 59 §2's mutual exclusion) — checked first, ahead of the
        # `if not binding:` passthrough-label branch below, since `binding`
        # is always `None` here anyway. No Trigger-mode suffix, mirroring
        # Profile Switch's own "always the same" reasoning — Axis output has
        # no Trigger mode at all (ticket 60's Answer).
        return f"Axis: {AXIS_LABEL_BY_TARGET[axis_target]}"
    if not binding:
        # No "passthrough" qualifier: it's jargon the user doesn't need, it's
        # too long to fit the smaller (52px) buttons, and — once a running
        # Daemon is in analog mode — it's no longer even accurate for the
        # grid (those 20 keys are synthesized from depth thresholds then,
        # not literal evdev passthrough; only the Mode key/thumbstick/wheel
        # stay on raw evdev regardless of capture mode). Just name the
        # actual default output instead, which is true either way.
        return INPUT_DEFAULT_LABEL[inp]
    if binding["type"] == "keypress":
        mods = "+".join(m.capitalize() for m in binding.get("modifiers", []))
        raw_key = binding["key"]
        # Ticket 42: the real picker makes a mouse-button Key one click away
        # (previously only reachable by hand-typing "BTN_LEFT" into the free
        # -text field) — a bare `.replace("KEY_", "")` leaves those raw
        # ("BTN_LEFT") instead of a readable label, so BTN_ codes go through
        # key_picker's own catalog instead.
        key = LABEL_BY_CODE.get(raw_key, raw_key) if raw_key.startswith("BTN_") else raw_key.replace("KEY_", "")
        chord = f"{mods}+{key}" if mods else key
        return f"{chord}  [{TRIGGER_SHORT[binding['trigger']]}]"
    if binding["type"] == "profile_switch":
        # No Trigger-mode suffix: ProfileSwitch is validation-locked to
        # Fire-once (ticket 34), so it would always read the same "[1x]"
        # every other binding kind's suffix actually varies by.
        return f"→ {binding['target']}"
    if binding["type"] == "controller_button":
        raw_button = binding["button"]
        button = CONTROLLER_LABEL_BY_CODE.get(raw_button, raw_button)
        return f"Btn: {button}  [{TRIGGER_SHORT[binding['trigger']]}]"
    if binding["type"] == "step":
        # Ticket 55: resolves the library entry's display name rather than
        # the raw stepper_id, mirroring ticket 52's identical closing of
        # ticket 51's own "raw id until the real picker lands" gap for
        # Macro. Falls back to the raw id if the entry is somehow missing
        # (e.g. `steppers` not threaded in by an older caller), same as the
        # Macro branch below.
        arrow = "↑" if binding["direction"] == "forward" else "↓"
        stepper_id = binding["stepper_id"]
        name = (steppers or {}).get(stepper_id, {}).get("name", stepper_id)
        return f"Step {arrow} {name}  [{TRIGGER_SHORT[binding['trigger']]}]"
    # Ticket 52: resolves the library entry's display name rather than the
    # raw macro_id (ticket 51 deferred this — it needed `macros` threaded
    # in, which now happens here). Falls back to the raw id if the entry is
    # somehow missing (e.g. `PLACEHOLDER_CONFIG`'s empty `macros`), rather
    # than crashing on a dict lookup that should be structurally impossible
    # once `SetBinding`'s own unknown-macro_id validation is in play.
    macro_id = binding.get("macro_id", "?")
    name = macros.get(macro_id, {}).get("name", macro_id)
    return f"Macro: {name}  [{TRIGGER_SHORT[binding['trigger']]}]"


def describe_step(step: dict) -> str:
    kind = step["type"]
    if kind in ("key_down", "key_up"):
        raw = step["key"]
        # Ticket 92: a KeyDown/KeyUp step may target a controller button
        # (routed to the gamepad device by the injector). Render it with the
        # gamepad catalog's label and a ↓/↑ prefix, e.g. "↓ Btn: A / South".
        # Mouse buttons (also `BTN_*`) aren't in the gamepad catalog, so
        # they keep the plain "KeyDown BTN_SIDE" form.
        if raw in CONTROLLER_LABEL_BY_CODE:
            return f"{'↓' if kind == 'key_down' else '↑'} Btn: {CONTROLLER_LABEL_BY_CODE[raw]}"
        return f"{'KeyDown' if kind == 'key_down' else 'KeyUp'} {raw}"
    if kind == "delay_ms":
        return f"Delay {step['ms']}ms"
    return str(step)


def labeled_row(label: str, widget: Gtk.Widget) -> Gtk.Box:
    row = Gtk.Box(spacing=8)
    lbl = Gtk.Label(label=label, xalign=0)
    lbl.set_size_request(90, -1)
    row.append(lbl)
    widget.set_hexpand(True)
    row.append(widget)
    return row


# --- Actuation & release (ticket 19's settled "variant B", landed for real
# by ticket 26) ---

_DEPTH_TRACK_WIDTH = 320


class DepthTrack(Gtk.Overlay):
    """A horizontal 0-255 travel bar with two independently draggable
    markers (green Actuation, amber Release) plus a live fill showing the
    key's current depth. Built from plain `Gtk.Box`es rather than a
    `Gtk.DrawingArea` — ticket 19's prototype found this environment's
    pycairo has no `gi._gi_cairo` bridge, so a `draw` func can't take a
    `cairo.Context` here, and plain boxes sidestep that while arguably being
    more portable anyway. Ported from the prototype's winning variant B
    almost unchanged (`prototype/19-trigger-point-depth-ux`); the one
    addition is `on_drag_end`, since the prototype had nothing to persist a
    drag to."""

    def __init__(self, markers: list[dict], on_marker_moved, on_drag_end, height: int = 16):
        super().__init__()
        self.markers = markers
        self.on_marker_moved = on_marker_moved
        self.on_drag_end = on_drag_end
        self.height = height
        self.live_value: int | None = None
        # _DEPTH_TRACK_WIDTH is only a pre-realize fallback — the bar
        # hexpands to fill its container, and all pixel math below reads the
        # real allocated width via `_track_width()`.
        self.set_size_request(_DEPTH_TRACK_WIDTH, height)
        self.set_hexpand(True)

        track_bg = Gtk.Box(css_classes=["depth-track-bg"], hexpand=True)
        track_bg.set_size_request(_DEPTH_TRACK_WIDTH, height)
        self.set_child(track_bg)

        self.fill = Gtk.Box(css_classes=["depth-track-fill"], halign=Gtk.Align.START, valign=Gtk.Align.FILL)
        self.fill.set_size_request(0, height)
        self.fill.set_visible(False)
        self.add_overlay(self.fill)

        self.marker_widgets: list[Gtk.Box] = []
        for m in markers:
            mw = Gtk.Box(css_classes=[m["css"]], halign=Gtk.Align.START, valign=Gtk.Align.FILL)
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

        # No generic "my allocated size changed" signal exists on a plain
        # Gtk.Widget in GTK4 (that's a LayoutManager concern) — a cheap
        # 200ms resync while mapped keeps the bar/markers correct across an
        # interactive window resize, not just at initial map.
        self._resync_timer: int | None = None
        self.connect("map", lambda *_: self._start_resync())
        self.connect("unmap", lambda *_: self._stop_resync())

    def _start_resync(self) -> None:
        self._resync()
        if self._resync_timer is None:
            self._resync_timer = GLib.timeout_add(200, self._resync_tick)

    def _stop_resync(self) -> None:
        if self._resync_timer is not None:
            GLib.source_remove(self._resync_timer)
            self._resync_timer = None

    def _resync_tick(self) -> bool:
        self._resync()
        return True

    def _resync(self) -> None:
        self.set_live_value(self.live_value)
        self.sync_markers()

    def _track_width(self) -> int:
        return self.get_width() or _DEPTH_TRACK_WIDTH

    def set_live_value(self, v: int | None) -> None:
        self.live_value = v
        self.fill.set_size_request(round(self._value_to_x(v)) if v is not None else 0, self.height)
        self.fill.set_visible(v is not None)

    def sync_markers(self) -> None:
        """Repositions every marker widget from `self.markers`' current
        values — needed after an external mutation (e.g. "Reset to Profile
        default") or a resize, since dragging is the only other path that
        moves a marker widget."""
        for m, mw in zip(self.markers, self.marker_widgets):
            mw.set_margin_start(round(self._value_to_x(m["value"]) - 1))

    def _x_to_value(self, x: float) -> int:
        return max(0, min(255, round(x / self._track_width() * 255)))

    def _value_to_x(self, value: int) -> float:
        return value / 255 * self._track_width()

    def _on_drag_begin(self, gesture, start_x, start_y):
        nearest, best_dist = None, 1e9
        for i, m in enumerate(self.markers):
            if not m.get("draggable"):
                continue
            dist = abs(self._value_to_x(m["value"]) - start_x)
            if dist < best_dist:
                nearest, best_dist = i, dist
        self._drag_index = nearest
        if nearest is not None:
            self._drag_start_value = self.markers[nearest]["value"]

    def _on_drag_update(self, gesture, offset_x, offset_y):
        if self._drag_index is None:
            return
        start_x = self._value_to_x(self._drag_start_value)
        new_value = self._x_to_value(start_x + offset_x)
        self.markers[self._drag_index]["value"] = new_value
        self.marker_widgets[self._drag_index].set_margin_start(round(self._value_to_x(new_value) - 1))
        self.on_marker_moved(self._drag_index, new_value)

    def _on_drag_end(self, gesture, offset_x, offset_y):
        if self._drag_index is None:
            return
        self.on_drag_end(self._drag_index, self.markers[self._drag_index]["value"])
        self._drag_index = None


def build_actuation_section(
    client, config: dict, profile: str, inp: str, capture_mode: str, on_saved: Callable[[], None]
) -> Gtk.Widget:
    """The real Actuation & release editor for a Grid key (ticket 19's
    settled variant B, landed for real by ticket 26): two draggable markers,
    a live depth bar fed by `StartDepthStream`/`DepthChanged`, a badge
    doubling as the live capture-mode indicator, and a digital-mode fallback
    that greys the bar with a centered overlay warning rather than a
    separate line. Only meaningful for Grid keys — the Mode key, thumbstick,
    and wheel have no depth to threshold.

    `capture_mode` is `GetState()`'s value as of when the caller's own
    `rebuild()` last fetched it — the badge's *initial* value. It stays
    live for as long as this popover is open via a full app `rebuild()` on
    `CaptureModeChanged` (wired once, at the app level — see
    `daemon_client.DBusDaemonClient.subscribe_capture_mode_changed`), not a
    per-popover subscription: unlike depth, mode transitions are rare enough
    that a rebuild-driven update is simplest and avoids leaking a fresh
    signal connection on every one of this editor's frequent eager rebuilds.

    `on_saved` (ticket 27's live-hardware verification caught this): every
    popover for every Grid key is pre-built once, from one `GetConfig()`
    snapshot, during the app's own `rebuild()` — there is no Daemon signal
    for actuation-point/default changes (unlike `capture_mode`), so without
    forcing a rebuild, a `default_actuation`/override change made here was
    invisible in *any* freshly opened popover (this one or another key's)
    until the GUI was restarted. `set_actuation_point`/`clear_actuation_point`
    only ever affect this one key and already update this popover's own
    markers directly, so they're left alone; `set_default_actuation`/
    `reset_actuation_points` affect other keys' popovers too, so they call
    `on_saved()` on success, same as Save/Clear above — popping this popover
    down and forcing the next one open to read fresh data.
    """
    profile_dict = config["profiles"][profile]
    default_actuation = profile_dict["default_actuation"]
    starting = profile_dict["actuation_overrides"].get(inp, default_actuation)

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.add_css_class("actuation-section")

    header = Gtk.Box(spacing=6)
    header.append(Gtk.Label(label="Actuation & release", css_classes=["sub-heading"]))
    mode_badge = Gtk.Label(css_classes=["badge"])
    header.append(mode_badge)
    box.append(header)

    markers = [
        {"value": starting["actuation"], "css": "marker-actuation", "draggable": True},
        {"value": starting["release"], "css": "marker-release", "draggable": True},
    ]

    def on_moved(i, v):
        # Keep Actuation strictly above Release (hysteresis, ticket 17 §2) —
        # pushing one marker past the other drags it along by 1 rather than
        # letting them cross or collide.
        if i == 0 and v <= markers[1]["value"]:
            v = markers[1]["value"] + 1
            markers[0]["value"] = v
        if i == 1 and v >= markers[0]["value"]:
            v = markers[0]["value"] - 1
            markers[1]["value"] = v
        refresh_label()

    def on_drag_end(i, v):
        try:
            client.set_actuation_point(inp, markers[0]["value"], markers[1]["value"])
        except DaemonError as exc:
            show_error(exc)

    track = DepthTrack(markers, on_marker_moved=on_moved, on_drag_end=on_drag_end)

    # The digital-mode note sits centered *over* the (greyed-out) track
    # rather than as a separate line beneath it, per the live reaction
    # ticket 19's grilling session recorded.
    track_overlay = Gtk.Overlay(hexpand=True)
    track_overlay.set_child(track)
    digital_note = Gtk.Label(
        label="No depth — analog capture unavailable",
        wrap=True,
        halign=Gtk.Align.CENTER,
        valign=Gtk.Align.CENTER,
        css_classes=["digital-note-overlay"],
    )
    track_overlay.add_overlay(digital_note)
    box.append(track_overlay)

    legend = Gtk.Label(xalign=0, use_markup=True, css_classes=["marker-legend"])
    legend.set_markup(
        '<span foreground="#2ecc71">green</span> = actuation (fires Down)   ·   '
        '<span foreground="#e6991a">amber</span> = release (fires Up)'
    )
    box.append(legend)

    value_label = Gtk.Label(xalign=0)
    value_label.add_css_class("dim")
    box.append(value_label)

    error_label = Gtk.Label(xalign=0, wrap=True)
    error_label.add_css_class("error")
    error_label.set_visible(False)
    box.append(error_label)

    def show_error(exc: Exception):
        error_label.set_label(str(exc))
        error_label.set_visible(True)

    def pct(v: int) -> str:
        return f"{round(v / 255 * 100)}%"

    def refresh_label():
        value_label.set_label(f"Actuation {pct(markers[0]['value'])}   Release {pct(markers[1]['value'])}")

    refresh_label()

    actions_row = Gtk.Box(spacing=8)
    reset_btn = Gtk.Button(label="Reset to Profile default")

    def on_reset(b):
        try:
            client.clear_actuation_point(inp)
        except DaemonError as exc:
            show_error(exc)
            return
        markers[0]["value"], markers[1]["value"] = (
            default_actuation["actuation"],
            default_actuation["release"],
        )
        track.sync_markers()
        refresh_label()

    reset_btn.connect("clicked", on_reset)
    actions_row.append(reset_btn)
    box.append(actions_row)

    profile_row = Gtk.Box(spacing=8)
    set_default_btn = Gtk.Button(label="Set as Profile default", css_classes=["dim"])

    def on_set_default(b):
        try:
            client.set_default_actuation(markers[0]["value"], markers[1]["value"])
        except DaemonError as exc:
            show_error(exc)
            return
        on_saved()

    set_default_btn.connect("clicked", on_set_default)
    profile_row.append(set_default_btn)

    reset_all_btn = Gtk.Button(label="Reset all keys to Profile default", css_classes=["dim"])

    def on_reset_all(b):
        try:
            client.reset_actuation_points()
        except DaemonError as exc:
            show_error(exc)
            return
        on_saved()

    reset_all_btn.connect("clicked", on_reset_all)
    profile_row.append(reset_all_btn)
    box.append(profile_row)

    force_digital_check = Gtk.CheckButton(label="Force digital capture (disable analog)")
    force_digital_check.add_css_class("dim")
    # Ticket 27: seeded from the real persisted preference (`GetConfig()`
    # now serializes it) rather than always constructing unchecked — set
    # before connecting "toggled" so seeding this doesn't itself fire
    # `on_force_digital` and re-send an unchanged value to the Daemon.
    force_digital_check.set_active(config.get("force_digital", False))

    def on_force_digital(b):
        client.set_force_digital(b.get_active())

    force_digital_check.connect("toggled", on_force_digital)
    box.append(force_digital_check)

    def apply_mode(mode: str) -> None:
        digital = mode == "digital"
        mode_badge.set_label("digital" if digital else "analog")
        mode_badge.remove_css_class("badge-analog")
        mode_badge.remove_css_class("badge-digital")
        mode_badge.add_css_class("badge-digital" if digital else "badge-analog")
        digital_note.set_visible(digital)
        if digital:
            track.add_css_class("depth-track-dim")
        else:
            track.remove_css_class("depth-track-dim")
        track.set_sensitive(not digital)

    apply_mode(capture_mode)

    def on_depth(depth: int) -> None:
        track.set_live_value(depth)

    # Live only while this popover is open (ticket 19's Answer): starts the
    # real `StartDepthStream`/`DepthChanged` subscription on map, stops it
    # on unmap — `build_binding_editor` is rebuilt eagerly for every Grid
    # key on every app `rebuild()`, so this must not run at construction
    # time, or every rebuild would call `StartDepthStream` for all 20 grid
    # keys instead of just the one popover a user might have open.
    box.connect("map", lambda *_: client.start_depth_stream(inp, on_depth))
    box.connect("unmap", lambda *_: client.stop_depth_stream(inp))

    return box


def build_action_and_trigger_fields(
    client,
    config: dict,
    profile: str,
    starting: dict,
    save_btn: Gtk.Button,
    available_action_types: list[tuple[str, str]] = ACTION_TYPES,
    inp: str | None = None,
    layer: str | None = None,
) -> tuple[Gtk.Widget, Gtk.DropDown, Callable[[], dict]]:
    """The Trigger-mode/Action editor core — everything below a Binding's
    own heading, shared verbatim by `build_binding_editor`'s per-Input
    popover and `build_chord_binding_dialog`'s small modal (ticket 01/40:
    a Chord's Binding is edited with "the existing Trigger/Action editor
    UI", not a second copy of it — `Binding` itself is unchanged, just keyed
    by a Set<Input> instead of one `Input`).

    `available_action_types` defaults to every kind `build_binding_editor`
    offers; `build_chord_binding_dialog` passes a narrower list excluding
    Profile Switch, which `SetChordBinding` always rejects (a Chord's own
    Action can never be one — see `ConfigError::InvalidChordProfileSwitch`)
    — offering it here would be a guaranteed-failing round-trip rather than
    a structurally-prevented one, per the codebase's "impossible, not just
    tolerated" standard (e.g. `SetChordBinding`'s subset/superset rule
    itself).

    Returns `(fields, trigger_dd, get_binding)`: `fields` is the widget to
    append into the caller's own box; `trigger_dd` is exposed so a caller
    that also validates Trigger-mode-specific rules (none currently do, but
    `build_binding_editor` did historically) can still reach it directly;
    `get_binding()` reads the current widget state into the same flat
    Binding dict every caller sends to the Daemon. `save_btn` is the
    caller's own Save button — this function only ever toggles its
    `set_sensitive`, never builds or places it, so each caller keeps full
    control of its own button row.
    """
    fields = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)

    # Analog-repeat only has meaning for a grid key (ticket 20/39 — only a
    # Grid Input has Depth) — excluded outright for a non-grid Input or a
    # Chord's own Binding (`inp is None`), mirroring `available_action_
    # types`'s own `is_grid_input`-gated "Axis" exclusion above it. The
    # Daemon's own `parse`/`SetBinding`/`SetChordBinding` validation
    # (ticket 39) makes it structurally impossible for `starting["trigger"]`
    # to be "analog_repeat" here when this filters it out. Fixed for the
    # popover's whole lifetime (an Input's grid-ness never changes), unlike
    # `trigger_options` below, which `render_action_editor` also narrows for
    # Controller Button and rebuilds live as the Action-kind changes (ticket
    # 78).
    base_trigger_options = (
        TRIGGER_OPTIONS
        if inp is not None and is_grid_input(inp)
        else [(k, lbl) for k, lbl in TRIGGER_OPTIONS if k != "analog_repeat"]
    )
    trigger_options = base_trigger_options
    trigger_keys = [k for k, _ in trigger_options]
    trigger_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in trigger_options]))
    trigger_dd.set_selected(trigger_keys.index(starting["trigger"]))
    fields.append(labeled_row("Trigger mode", trigger_dd))

    known_action_kinds = [k for k, _ in available_action_types]
    # `Action::Step` (ticket 03/54) has no editor built here yet (ticket
    # 55's job) and isn't offered in `ACTION_TYPES` — but the Daemon can
    # already hand back a Step Binding (a hand-edited config.toml, or any
    # other `com.acheron.Daemon` caller), and this popover must not crash
    # trying to look an unknown `starting["type"]` up in a list that
    # doesn't contain it. Falls back to index 0 for the dropdown itself;
    # `unsupported_kind` (below) keeps Save disabled until the user
    # explicitly picks a real Action kind, so simply opening this popover
    # can never silently clobber the existing Binding with an unrelated
    # default.
    unsupported_kind = starting["type"] not in known_action_kinds

    action_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in available_action_types]))
    action_dd.set_selected(0 if unsupported_kind else known_action_kinds.index(starting["type"]))
    fields.append(labeled_row("Action", action_dd))

    editor_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    fields.append(editor_slot)

    # `save_btn` is the caller's own button (see this function's docstring)
    # — the Macro/Step branches below gate its sensitivity directly,
    # mirroring how trigger_dd's sensitivity is already gated for
    # profile_switch.
    draft = {
        "keypress": {"key": starting.get("key", "KEY_A"), "modifiers": list(starting.get("modifiers", []))}
        if starting["type"] == "keypress"
        else {"key": "KEY_A", "modifiers": []},
        "macro": {"macro_id": starting.get("macro_id")} if starting["type"] == "macro" else {"macro_id": None},
        "step": {
            "stepper_id": starting.get("stepper_id"),
            "direction": starting.get("direction", "forward"),
        }
        if starting["type"] == "step"
        else {"stepper_id": None, "direction": "forward"},
        "profile_switch": {"target": starting.get("target", profile)}
        if starting["type"] == "profile_switch"
        else {"target": profile},
        "controller_button": {"button": starting.get("button", "BTN_SOUTH")}
        if starting["type"] == "controller_button"
        else {"button": "BTN_SOUTH"},
        "axis": {"target": starting.get("target")} if starting["type"] == "axis" else {"target": None},
    }

    # Ticket 42: the keypress Key field's modifier warning also depends on
    # Trigger mode, which the picker component can't see on its own — a
    # single current handler on trigger_dd (which outlives every
    # render_action_editor() rebuild, unlike editor_slot's own children)
    # avoids piling up one stale listener per rebuild.
    _trigger_handler: dict = {"id": None}

    def render_action_editor():
        nonlocal trigger_options, trigger_keys
        clear_children(editor_slot)
        if _trigger_handler["id"] is not None:
            trigger_dd.disconnect(_trigger_handler["id"])
            _trigger_handler["id"] = None

        kind = available_action_types[action_dd.get_selected()][0]
        # Reset here, unconditionally — only the Macro branch below ever
        # disables it (no picker yet to assign a fresh macro_id), and every
        # other kind must not stay disabled from a previous render.
        save_btn.set_sensitive(True)

        # Ticket 78: Fire-once is locked out for Controller Button (Hold-to-
        # repeat's sustained-hold behavior already covers a quick tap; no
        # real gamepad button press works like Fire-once's decoupled pulse)
        # — unlike Analog-repeat's exclusion above (fixed for the popover's
        # whole lifetime, since an Input's grid-ness never changes), this
        # depends on `kind`, which the user can flip live via `action_dd`, so
        # the model is rebuilt here rather than computed once. Mirrors the
        # non-grid-Input/Chord Analog-repeat exclusion's own reasoning
        # (`Gtk.DropDown` has no per-item sensitivity — ticket 39's
        # precedent), applied a second time for a second kind-gated entry.
        new_trigger_options = (
            [(k, lbl) for k, lbl in base_trigger_options if k != "fire_once"]
            if kind == "controller_button"
            else base_trigger_options
        )
        new_trigger_keys = [k for k, _ in new_trigger_options]
        if new_trigger_keys != trigger_keys:
            previous_key = (
                trigger_keys[trigger_dd.get_selected()]
                if trigger_dd.get_selected() < len(trigger_keys)
                else None
            )
            trigger_dd.set_model(Gtk.StringList.new([lbl for _, lbl in new_trigger_options]))
            trigger_options, trigger_keys = new_trigger_options, new_trigger_keys
            trigger_dd.set_selected(
                trigger_keys.index(previous_key)
                if previous_key in trigger_keys
                # Fire-once just got excluded (kind became controller_button)
                # — Hold-to-repeat is the closest real-gamepad equivalent.
                else trigger_keys.index("hold_to_repeat")
            )

        # Profile Switch has no coherent held/toggled meaning (ticket 05) —
        # locked to Fire-once here and again, defensively, in on_save. Axis
        # output has no Trigger mode at all (ticket 60's Answer) — same
        # lock mechanism, reused for a second kind, with its own tooltip
        # explaining why (reset to none for every other kind, so a stale
        # tooltip doesn't survive switching away from Axis).
        trigger_dd.set_sensitive(kind not in ("profile_switch", "axis"))
        trigger_dd.set_tooltip_text(None)
        if kind == "profile_switch":
            trigger_dd.set_selected(trigger_keys.index("fire_once"))
        elif kind == "axis":
            trigger_dd.set_tooltip_text("Axis output has no Trigger mode")

        if kind == "keypress":
            def on_key_changed(code: str) -> None:
                draft["keypress"]["key"] = code

            def key_warn_predicate() -> bool:
                return trigger_options[trigger_dd.get_selected()][0] != "toggle"

            key_picker, refresh_key_warning = build_inline_key_picker(
                draft["keypress"].get("key", "KEY_A"), on_key_changed, key_warn_predicate
            )
            editor_slot.append(labeled_row("Key", key_picker))
            _trigger_handler["id"] = trigger_dd.connect("notify::selected", lambda *_: refresh_key_warning())

            mod_box = Gtk.Box(spacing=8)
            mods = set(draft["keypress"].get("modifiers", []))
            for m in ("ctrl", "shift", "alt", "super"):
                cb = Gtk.CheckButton(label=m)
                cb.set_active(m in mods)

                def on_mod(c, m=m):
                    cur = set(draft["keypress"].get("modifiers", []))
                    if c.get_active():
                        cur.add(m)
                    else:
                        cur.discard(m)
                    draft["keypress"]["modifiers"] = sorted(cur)

                cb.connect("toggled", on_mod)
                mod_box.append(cb)
            editor_slot.append(mod_box)
        elif kind == "profile_switch":
            profile_names = sorted(config["profiles"].keys())
            target_dd = Gtk.DropDown(model=Gtk.StringList.new(profile_names))
            current_target = draft["profile_switch"].get("target", profile)
            if current_target not in profile_names:
                current_target = profile_names[0]
            draft["profile_switch"]["target"] = current_target
            target_dd.set_selected(profile_names.index(current_target))

            def on_target_changed(dd, *_):
                draft["profile_switch"]["target"] = profile_names[dd.get_selected()]

            target_dd.connect("notify::selected", on_target_changed)
            editor_slot.append(labeled_row("Target Profile", target_dd))
        elif kind == "controller_button":
            def on_button_changed(code: str) -> None:
                draft["controller_button"]["button"] = code

            controller_picker = build_inline_controller_picker(
                draft["controller_button"].get("button", "BTN_SOUTH"), on_button_changed
            )
            editor_slot.append(labeled_row("Button", controller_picker))
        elif kind == "axis":
            def on_axis_changed(target: str) -> None:
                draft["axis"]["target"] = target
                save_btn.set_sensitive(target is not None)

            # Ticket 60's cross-key toast: which other key (if any) already
            # claims each target on this same Layer — `inp`/`layer` are
            # `None` for `build_chord_binding_dialog`'s call (which excludes
            # "axis" from its own `available_action_types`, so this branch
            # never actually runs there; the `or {}` just keeps this
            # defensive rather than reaching for a missing config key).
            axis_map = config["profiles"][profile][f"axis_{layer}"] if layer else {}
            claimed_by = {target: input_label(other) for other, target in axis_map.items() if other != inp}
            axis_picker = build_inline_axis_picker(
                draft["axis"].get("target"), on_axis_changed, claimed_by
            )
            editor_slot.append(labeled_row("Target", axis_picker))
            save_btn.set_sensitive(draft["axis"].get("target") is not None)
        elif kind == "step":
            # Ticket 55: the real assignment flow, mirroring the Macro
            # branch below almost exactly — a dropdown of existing library
            # entries (by display name) plus "+ New Stepper" to create one
            # inline and assign it right away. Unlike Macro, Action::Step
            # carries a second field (`direction`), so a Forward/Backward
            # dropdown sits alongside the Stepper dropdown; full item
            # authoring and the Forward/Backward *Input*-pair assignment
            # both live in the Library screen
            # (`library_view.build_stepper_editor_columns`), not here — this
            # popover only ever assigns `stepper_id`/`direction` to the
            # Binding on this one Input, exactly like every other branch
            # here only assigns its own field(s).
            steppers = config.get("steppers", {})
            stepper_ids = sorted(steppers, key=lambda sid: steppers[sid]["name"].lower())
            current_stepper_id = draft["step"].get("stepper_id")

            if stepper_ids:
                stepper_dd = Gtk.DropDown(model=Gtk.StringList.new([steppers[sid]["name"] for sid in stepper_ids]))
                if current_stepper_id in stepper_ids:
                    stepper_dd.set_selected(stepper_ids.index(current_stepper_id))
                else:
                    stepper_dd.set_selected(0)
                    draft["step"]["stepper_id"] = stepper_ids[0]

                def on_stepper_changed(dd, *_):
                    draft["step"]["stepper_id"] = stepper_ids[dd.get_selected()]

                stepper_dd.connect("notify::selected", on_stepper_changed)
                editor_slot.append(labeled_row("Stepper", stepper_dd))
            else:
                editor_slot.append(
                    Gtk.Label(label="No Steppers in the library yet — create one below.", xalign=0, wrap=True)
                )

            direction_options = [("forward", "Forward"), ("backward", "Backward")]
            direction_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in direction_options]))
            current_direction = draft["step"].get("direction", "forward")
            direction_dd.set_selected([k for k, _ in direction_options].index(current_direction))

            def on_direction_changed(dd, *_):
                draft["step"]["direction"] = direction_options[dd.get_selected()][0]

            direction_dd.connect("notify::selected", on_direction_changed)
            editor_slot.append(labeled_row("Direction", direction_dd))

            new_stepper_btn = Gtk.MenuButton(label="+ New Stepper")

            def on_new_stepper_submitted(name: str):
                stepper_id = client.create_stepper(name, [])
                # Same reasoning as "+ New Macro" below: mutate the snapshot
                # in place so the rebuild sees the entry it just created.
                config.setdefault("steppers", {})[stepper_id] = {"name": name, "items": []}
                draft["step"]["stepper_id"] = stepper_id
                render_action_editor()

            new_stepper_btn.set_popover(
                build_name_prompt_popover("Creating a Stepper", "", "Create", on_new_stepper_submitted)
            )
            editor_slot.append(new_stepper_btn)

            save_btn.set_sensitive(draft["step"].get("stepper_id") is not None)
        else:
            # Ticket 52: the real assignment flow — a dropdown of existing
            # library entries (by display name, ticket 51's macro_id stays
            # internal) plus "+ New Macro" to create one inline and assign it
            # right away, replacing ticket 51's temporary read-only stub.
            # Full step authoring lives in the Library screen
            # (`library_view.build_macro_editor_columns`), not here — this popover
            # only ever assigns a `macro_id` to the Binding, exactly like the
            # Controller-button/Profile-switch branches only ever assign
            # their own single field.
            macros = config.get("macros", {})
            macro_ids = sorted(macros, key=lambda mid: macros[mid]["name"].lower())
            current_macro_id = draft["macro"].get("macro_id")

            if macro_ids:
                macro_dd = Gtk.DropDown(model=Gtk.StringList.new([macros[mid]["name"] for mid in macro_ids]))
                if current_macro_id in macro_ids:
                    macro_dd.set_selected(macro_ids.index(current_macro_id))
                else:
                    macro_dd.set_selected(0)
                    draft["macro"]["macro_id"] = macro_ids[0]

                def on_macro_changed(dd, *_):
                    draft["macro"]["macro_id"] = macro_ids[dd.get_selected()]

                macro_dd.connect("notify::selected", on_macro_changed)
                editor_slot.append(labeled_row("Macro", macro_dd))
            else:
                editor_slot.append(
                    Gtk.Label(label="No Macros in the library yet — create one below.", xalign=0, wrap=True)
                )

            new_macro_btn = Gtk.MenuButton(label="+ New Macro")

            def on_new_macro_submitted(name: str):
                macro_id = client.create_macro(name, [])
                # `config` is a snapshot fetched before this popover opened
                # (per the module docstring) — mutated in place here so the
                # rebuild below sees the entry it just created, rather than
                # `render_action_editor` immediately overwriting `macro_id`
                # with `macro_ids[0]` because the fresh id isn't in its
                # (stale) `macro_ids` list yet.
                config.setdefault("macros", {})[macro_id] = {"name": name, "steps": []}
                draft["macro"]["macro_id"] = macro_id
                render_action_editor()

            new_macro_btn.set_popover(
                build_name_prompt_popover("Creating a Macro", "", "Create", on_new_macro_submitted)
            )
            editor_slot.append(new_macro_btn)

            save_btn.set_sensitive(draft["macro"].get("macro_id") is not None)

    action_dd.connect("notify::selected", lambda *_: render_action_editor())
    render_action_editor()
    if unsupported_kind:
        # See `unsupported_kind`'s definition above — render_action_editor()
        # just built an ordinary (misleadingly unrelated) editor for
        # whatever kind index 0 is, so Save is force-disabled again here and
        # a banner explains the mismatch. Clear still works normally (it
        # doesn't depend on `kind` at all), and picking a different Action
        # above re-enables Save via render_action_editor()'s own reset.
        save_btn.set_sensitive(False)
        editor_slot.append(
            Gtk.Label(
                label=f"This Input's current Binding type ({starting['type']!r}) has no editor here "
                "yet — pick a different Action above to replace it, or use Clear below to remove it.",
                xalign=0,
                wrap=True,
                css_classes=["dim"],
            )
        )

    def get_binding() -> dict:
        kind = available_action_types[action_dd.get_selected()][0]
        if kind == "axis":
            # Not a Binding at all (ticket 59 §2 — Axis assignment is a
            # parallel, structurally independent concept, no Trigger mode/
            # Action). The caller (`build_binding_editor`'s `on_save`) must
            # branch on this `"type"` and call `client.set_axis_assignment`
            # instead of `client.set_binding`.
            return {"type": "axis", "target": draft["axis"].get("target")}
        if kind == "keypress":
            return {
                "trigger": trigger_options[trigger_dd.get_selected()][0],
                "type": "keypress",
                "key": draft["keypress"].get("key", "KEY_A"),
                "modifiers": draft["keypress"].get("modifiers", []),
            }
        if kind == "profile_switch":
            return {
                # Always Fire-once regardless of the (disabled) dropdown's
                # own selection — the Daemon rejects anything else anyway.
                "trigger": "fire_once",
                "type": "profile_switch",
                "target": draft["profile_switch"].get("target", profile),
            }
        if kind == "controller_button":
            return {
                "trigger": trigger_options[trigger_dd.get_selected()][0],
                "type": "controller_button",
                "button": draft["controller_button"].get("button", "BTN_SOUTH"),
            }
        if kind == "step":
            return {
                "trigger": trigger_options[trigger_dd.get_selected()][0],
                "type": "step",
                "stepper_id": draft["step"]["stepper_id"],
                "direction": draft["step"].get("direction", "forward"),
            }
        return {
            "trigger": trigger_options[trigger_dd.get_selected()][0],
            "type": "macro",
            "macro_id": draft["macro"]["macro_id"],
        }

    return fields, trigger_dd, get_binding


def build_binding_editor(
    client,
    config: dict,
    profile: str,
    layer: str,
    inp: str,
    on_saved: Callable[[], None],
    capture_mode: str = "digital",
) -> Gtk.Widget:
    bindings = config["profiles"][profile][layer]
    existing = bindings.get(inp)
    # Ticket 71: an Axis-assigned Input never has a `binding` at all (ticket
    # 59 §2's mutual exclusion) — checked ahead of `existing`, so the editor
    # defaults to the "Axis" dropdown entry with the current target seeded,
    # rather than falling through to `existing`'s always-`None` value here.
    current_axis_target = config["profiles"][profile][f"axis_{layer}"].get(inp)
    if current_axis_target is not None:
        # Axis output has no Trigger mode at all (ticket 60) — this "trigger"
        # is inert, never read by the "axis" branch, left as-is.
        starting = {"trigger": "fire_once", "type": "axis", "target": current_axis_target}
    else:
        # Ticket 89: a freshly-created Binding defaults to Hold-to-repeat
        # (Fire-once for the scroll wheel — see `default_trigger_for`), not
        # Fire-once everywhere.
        starting = existing or {
            "trigger": default_trigger_for(inp),
            "type": "keypress",
            "key": "KEY_A",
            "modifiers": [],
        }
    # "Axis" is offered only for grid keys (ticket 60's Answer) — non-grid
    # Inputs (Mode key, thumbstick, wheel) never see the option at all,
    # rather than seeing it disabled.
    available_action_types = ACTION_TYPES if is_grid_input(inp) else [e for e in ACTION_TYPES if e[0] != "axis"]

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    box.set_margin_top(10)
    box.set_margin_bottom(10)
    box.set_margin_start(10)
    box.set_margin_end(10)
    heading = Gtk.Label(label=f"{profile} / {layer} / {input_label(inp)}", xalign=0)
    heading.add_css_class("heading")
    box.append(heading)

    error_label = Gtk.Label(xalign=0, wrap=True)
    error_label.add_css_class("error")
    error_label.set_visible(False)
    box.append(error_label)

    save_btn = Gtk.Button(label="Save")
    save_btn.add_css_class("suggested-action")

    fields, _trigger_dd, get_binding = build_action_and_trigger_fields(
        client, config, profile, starting, save_btn, available_action_types, inp, layer
    )
    box.append(fields)

    def show_error(exc: Exception):
        error_label.set_label(str(exc))
        error_label.set_visible(True)

    btn_row = Gtk.Box(spacing=8)

    def on_save(b):
        binding = get_binding()
        try:
            if binding["type"] == "axis":
                client.set_axis_assignment(inp, layer, binding["target"])
            else:
                client.set_binding(inp, layer, binding)
        except DaemonError as exc:
            show_error(exc)
            return
        on_saved()

    save_btn.connect("clicked", on_save)
    btn_row.append(save_btn)

    clear_btn = Gtk.Button(label="Clear Binding")

    def on_clear(b):
        if current_axis_target is not None:
            try:
                client.clear_axis_assignment(inp, layer)
            except DaemonError as exc:
                show_error(exc)
                return
            on_saved()
            return
        if existing is None:
            # Already unbound — nothing to clear, no D-Bus call needed.
            on_saved()
            return
        try:
            client.clear_binding(inp, layer)
        except DaemonError as exc:
            show_error(exc)
            return
        on_saved()

    clear_btn.connect("clicked", on_clear)
    btn_row.append(clear_btn)
    box.append(btn_row)

    if is_grid_input(inp):
        # Ticket 70 follow-up, live-verified: on first open, the window
        # this sits in (device_overview.make_input_button) should always
        # be tall enough to show everything above this point — heading,
        # error, the Trigger/Action fields including the inline key/
        # mouse-button picker's full expanded shape, and the Save/Clear
        # row — without scrolling, since the user always wants those
        # reachable. Only the Actuation & release section (this one, grid-
        # Inputs only) is deferred behind its own scroll: it's needed less
        # often, and it's the section whose live depth bar/marker controls
        # can push the window past the screen if left unbounded. Wrapping
        # just this section, rather than the whole editor (as an earlier
        # pass here did), keeps the rest of the window sized to its own
        # natural height instead of being capped along with it.
        actuation_scroller = Gtk.ScrolledWindow(hscrollbar_policy=Gtk.PolicyType.NEVER)
        actuation_scroller.set_propagate_natural_width(True)
        actuation_scroller.set_propagate_natural_height(True)
        actuation_scroller.set_max_content_height(320)
        actuation_scroller.set_child(build_actuation_section(client, config, profile, inp, capture_mode, on_saved))
        box.append(actuation_scroller)

    return box


def build_chord_binding_dialog(
    client,
    config: dict,
    profile: str,
    layer: str,
    members: list[str],
    existing: dict | None,
    on_saved: Callable[[], None],
    win: Gtk.Window,
    editing_key: str | None = None,
) -> Gtk.Window:
    """The small modal ticket 40 asks for: just the Trigger/Action editor
    (ticket 01/02's already-settled UI, reused via
    `build_action_and_trigger_fields`) for a Chord's own Binding — its
    membership (`members`) is fully decided before this dialog ever opens
    (the caller's own grid selection in `device_overview.py`), so there is
    no Input picker here, unlike `build_binding_editor`'s per-Input popover.
    A fresh dialog is built and presented per call rather than built once
    and reused (unlike ticket 44's per-key editor windows) — `members` and
    `existing` differ on every "Binding →"/"Edit" click, so there is nothing
    to usefully cache here.

    `editing_key` is the Chord's *original* `+`-joined member-key string
    when editing an existing Chord whose membership may have just changed
    (`chord_ui["edit_key"]`, unrelated to `members`, the *new* selection) —
    `None` for a brand new Chord. When it differs from `members`' own key,
    `on_save` clears the old key *before* setting the new one (code-review
    finding: setting first would make the still-present old key spuriously
    conflict with itself whenever the edit grows/shrinks membership by
    containment — e.g. `{A,B}` edited into `{A,B,C}` — since
    `SetChordBinding`'s subset/superset check runs before any old entry is
    removed).
    """
    # Ticket 89: a Chord's own Binding has no single Input, so it takes the
    # plain Hold-to-repeat default (`default_trigger_for(None)`).
    starting = existing or {
        "trigger": default_trigger_for(None),
        "type": "keypress",
        "key": "KEY_A",
        "modifiers": [],
    }
    # Neither Profile Switch nor Axis has anywhere coherent to run from a
    # Chord's own Binding — Profile Switch because `fire_chord` has no
    # `&mut Config` to run a switch through, Axis because it isn't a Binding
    # at all (ticket 59 §2) and a Chord fires on a discrete Down, not a
    # continuous value.
    chord_action_types = [(k, lbl) for k, lbl in ACTION_TYPES if k not in ("profile_switch", "axis")]

    dialog = Gtk.Window(transient_for=win, modal=True, title="Chord binding")
    dialog.set_default_size(320, 280)
    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    for setter in (outer.set_margin_top, outer.set_margin_bottom, outer.set_margin_start, outer.set_margin_end):
        setter(10)
    # No scrolling wrapper here (unlike build_binding_editor's own Actuation
    # & release section) — this dialog has nothing past `fields` that's
    # reasonable to defer behind a scroll the way a grid Input's Actuation
    # section is. `fields` can include the inline key picker (a Keypress
    # Action), whose expandable modifier-group toggles are exactly what the
    # user always wants reachable without scrolling — `set_default_size`
    # above is only the *initial* suggested size; the dialog still grows to
    # fit `outer`'s full natural height when the picker needs more room.
    dialog.set_child(outer)

    heading = Gtk.Label(label=" + ".join(input_label(m) for m in members), xalign=0)
    heading.add_css_class("heading")
    outer.append(heading)

    error_label = Gtk.Label(xalign=0, wrap=True)
    error_label.add_css_class("error")
    error_label.set_visible(False)
    outer.append(error_label)

    save_btn = Gtk.Button(label="Save Chord")
    save_btn.add_css_class("suggested-action")

    fields, _trigger_dd, get_binding = build_action_and_trigger_fields(
        client, config, profile, starting, save_btn, chord_action_types
    )
    outer.append(fields)

    def on_save(b):
        binding = get_binding()
        try:
            # Compared as sets, not by re-deriving what the wire key
            # "should" look like — this stub/GUI must never assume its own
            # guess at the Daemon's `ChordKey` string-ordering convention
            # (code-review finding: an earlier version built a
            # `"+".join(sorted(members))` string here to compare against
            # `editing_key`, which is wrong whenever a Chord mixes Input
            # variant kinds, since the real Daemon's `ChordKey` orders by
            # `Input`'s own `Ord`, not alphabetically).
            if editing_key is not None and set(editing_key.split("+")) != set(members):
                client.clear_chord_binding(editing_key.split("+"), layer)
            client.set_chord_binding(members, layer, binding)
        except DaemonError as exc:
            error_label.set_label(str(exc))
            error_label.set_visible(True)
            return
        dialog.close()
        on_saved()

    save_btn.connect("clicked", on_save)
    btn_row = Gtk.Box(spacing=8)
    btn_row.append(save_btn)
    cancel_btn = Gtk.Button(label="Cancel")
    cancel_btn.connect("clicked", lambda b: dialog.close())
    btn_row.append(cancel_btn)
    outer.append(btn_row)

    return dialog
