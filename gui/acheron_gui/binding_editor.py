"""The shared Binding editor — one component used identically from Device
Overview's popovers and the Action Table sidebar's expandable rows (ticket
09's resolved IA). Trigger-mode/Macro-step UI is preserved in full from the
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
from .gtk_utils import clear_children
from .inputs import ACTION_TYPES, TRIGGER_OPTIONS, TRIGGER_SHORT, input_label, is_grid_input


def action_summary(binding: dict | None) -> str:
    if not binding:
        return "passthrough"
    if binding["type"] == "keypress":
        mods = "+".join(m.capitalize() for m in binding.get("modifiers", []))
        key = binding["key"].replace("KEY_", "")
        chord = f"{mods}+{key}" if mods else key
        return f"{chord}  [{TRIGGER_SHORT[binding['trigger']]}]"
    steps = binding.get("steps", [])
    return f"Macro ({len(steps)} steps)  [{TRIGGER_SHORT[binding['trigger']]}]"


def describe_step(step: dict) -> str:
    kind = step["type"]
    if kind == "key_down":
        return f"KeyDown {step['key']}"
    if kind == "key_up":
        return f"KeyUp {step['key']}"
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


def build_actuation_section(client, config: dict, profile: str, inp: str, capture_mode: str) -> Gtk.Widget:
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

    set_default_btn.connect("clicked", on_set_default)
    profile_row.append(set_default_btn)

    reset_all_btn = Gtk.Button(label="Reset all keys to Profile default", css_classes=["dim"])

    def on_reset_all(b):
        try:
            client.reset_actuation_points()
        except DaemonError as exc:
            show_error(exc)
            return
        markers[0]["value"], markers[1]["value"] = (
            default_actuation["actuation"],
            default_actuation["release"],
        )
        track.sync_markers()
        refresh_label()

    reset_all_btn.connect("clicked", on_reset_all)
    profile_row.append(reset_all_btn)
    box.append(profile_row)

    force_digital_check = Gtk.CheckButton(label="Force digital capture (disable analog)")
    force_digital_check.add_css_class("dim")

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
    starting = existing or {"trigger": "fire_once", "type": "keypress", "key": "KEY_A", "modifiers": []}

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

    trigger_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in TRIGGER_OPTIONS]))
    trigger_dd.set_selected([k for k, _ in TRIGGER_OPTIONS].index(starting["trigger"]))
    box.append(labeled_row("Trigger mode", trigger_dd))

    action_dd = Gtk.DropDown(model=Gtk.StringList.new([lbl for _, lbl in ACTION_TYPES]))
    action_dd.set_selected([k for k, _ in ACTION_TYPES].index(starting["type"]))
    box.append(labeled_row("Action", action_dd))

    editor_slot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.append(editor_slot)

    draft = {
        "keypress": {"key": starting.get("key", "KEY_A"), "modifiers": list(starting.get("modifiers", []))}
        if starting["type"] == "keypress"
        else {"key": "KEY_A", "modifiers": []},
        "steps": list(starting.get("steps", [])) if starting["type"] == "macro" else [],
    }

    def render_action_editor():
        clear_children(editor_slot)

        kind = ACTION_TYPES[action_dd.get_selected()][0]
        if kind == "keypress":
            key_entry = Gtk.Entry(text=draft["keypress"].get("key", "KEY_A"))
            key_entry.connect("changed", lambda e: draft["keypress"].__setitem__("key", e.get_text()))
            editor_slot.append(labeled_row("Key", key_entry))

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
        else:
            steps_list = Gtk.ListBox()
            steps_list.add_css_class("boxed-list")

            def render_steps():
                clear_children(steps_list)
                for i, step in enumerate(draft["steps"]):
                    row_box = Gtk.Box(spacing=6)
                    row_box.set_margin_top(2)
                    row_box.set_margin_bottom(2)
                    row_box.set_margin_start(4)
                    row_box.set_margin_end(4)
                    row_box.append(Gtk.Label(label=describe_step(step), hexpand=True, xalign=0))
                    rm = Gtk.Button(label="×")

                    def on_remove(b, i=i):
                        draft["steps"].pop(i)
                        render_steps()

                    rm.connect("clicked", on_remove)
                    row_box.append(rm)
                    steps_list.append(row_box)

            render_steps()
            editor_slot.append(steps_list)

            add_box = Gtk.Box(spacing=6)
            step_kind_dd = Gtk.DropDown(model=Gtk.StringList.new(["KeyDown", "KeyUp", "Delay (ms)"]))
            value_entry = Gtk.Entry(text="KEY_A", width_chars=10)
            add_box.append(step_kind_dd)
            add_box.append(value_entry)
            add_btn = Gtk.Button(label="+ Add step")

            def on_add(b):
                kind_i = step_kind_dd.get_selected()
                val = value_entry.get_text()
                if kind_i == 0:
                    step = {"type": "key_down", "key": val}
                elif kind_i == 1:
                    step = {"type": "key_up", "key": val}
                else:
                    step = {"type": "delay_ms", "ms": int(val) if val.isdigit() else 0}
                draft["steps"].append(step)
                render_steps()

            add_btn.connect("clicked", on_add)
            add_box.append(add_btn)
            editor_slot.append(add_box)

    action_dd.connect("notify::selected", lambda *_: render_action_editor())
    render_action_editor()

    def show_error(exc: Exception):
        error_label.set_label(str(exc))
        error_label.set_visible(True)

    btn_row = Gtk.Box(spacing=8)
    save_btn = Gtk.Button(label="Save")
    save_btn.add_css_class("suggested-action")

    def on_save(b):
        kind = ACTION_TYPES[action_dd.get_selected()][0]
        if kind == "keypress":
            binding = {
                "trigger": TRIGGER_OPTIONS[trigger_dd.get_selected()][0],
                "type": "keypress",
                "key": draft["keypress"].get("key", "KEY_A"),
                "modifiers": draft["keypress"].get("modifiers", []),
            }
        else:
            binding = {
                "trigger": TRIGGER_OPTIONS[trigger_dd.get_selected()][0],
                "type": "macro",
                "steps": draft["steps"],
            }
        try:
            client.set_binding(inp, layer, binding)
        except DaemonError as exc:
            show_error(exc)
            return
        on_saved()

    save_btn.connect("clicked", on_save)
    btn_row.append(save_btn)

    clear_btn = Gtk.Button(label="Clear (passthrough)")

    def on_clear(b):
        if existing is None:
            # Already passthrough — nothing to clear, no D-Bus call needed.
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
        box.append(build_actuation_section(client, config, profile, inp, capture_mode))

    return box
