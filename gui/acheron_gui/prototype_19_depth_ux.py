"""PROTOTYPE — throwaway, answers ticket 19 (Design how a user sets a grid
key's trigger point, and the live-depth channel that makes it possible):
.scratch/tartarus-input-expansion/issues/19-prototype-trigger-point-ux-and-live-depth.md

Plan: three structurally different variants of an "Actuation" UX mounted
below the real `build_binding_editor()` for a grid key, switchable via a
floating bottom pill (adapted from the skill's `?variant=` convention — this
is a GTK4 desktop app, not a routed web page, so the switcher is Prev/Next
buttons plus Left/Right arrow keys instead of a URL param). No real hardware
in this environment: a `Gtk.Scale` stands in for "the physical key" — drag
it to simulate a press — plus an auto-sweep toggle for a hands-off preview.
Wipe me: nothing here persists past process exit, and none of it should be
promoted as-is (see each variant's own notes on what to fold into
`binding_editor.py` for real).

Run:
    python3 gui/prototype_19_depth_ux.py

Variants:
    A — Inline, single marker, raw 0-255, hysteresis derived automatically.
    B — Inline, two explicit markers, percentage units, discoverability badge.
    C — Separate "Calibrate" overview across all 20 keys, presets + advanced.

What this prototype is modeling, for the D-Bus-shape question ticket 19
also asks to settle: `SimDepth.subscribe`/`unsubscribe` below stand in for
a proposed `StartDepthStream(input)` / `StopDepthStream(input)` pair plus a
`DepthChanged(input, depth)` signal, scoped to the requesting client's own
bus connection (auto-stopped on disconnect, mirroring how
`SetOutputSuppressed` is request-scoped rather than globally toggled) and
independent of `StopAllToggles`/output-suppression's lifecycle — streaming
depth doesn't need to suppress output and vice versa. `SimDepth`'s 33ms
timer models a ~30Hz rate limit on that signal, well under the ~1ms-per-
change rate ticket 13 measured on real hardware, so the wire doesn't get
the firehose the ticket's "always-on was rejected" note warns about. This
prototype starts the (simulated) stream when the widget mounts and stops it
when the window closes, standing in for "starts when the editor opens for a
grid Input, stops when it closes."
"""

from __future__ import annotations

import math

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, GLib, Gtk

from .binding_editor import build_binding_editor
from .daemon_stub import DaemonStub
from .gtk_utils import clear_children
from .inputs import input_label

PROTOTYPE_INPUT = "grid_r2c3"

CSS = """
.heading { font-weight: bold; }
.dim { opacity: 0.6; font-size: smaller; }
.actuation-section { padding: 8px; margin-top: 4px; background-color: alpha(currentColor, 0.05); border-radius: 6px; }
.badge { border-radius: 999px; padding: 0px 6px; font-size: smaller; font-weight: bold; }
.badge-analog { background-color: #2ecc71; color: black; }
.badge-digital { background-color: #c0392b; color: white; }
.digital-note { color: #e5883b; font-size: smaller; }
.digital-note-overlay { color: #e5883b; font-size: smaller; font-weight: bold; background-color: alpha(black, 0.55); border-radius: 4px; padding: 2px 8px; }
.switcher-pill { background-color: alpha(currentColor, 0.08); border: 1px solid alpha(currentColor, 0.25); border-radius: 999px; padding: 6px 14px; }
.variant-label { font-weight: bold; }
.marker-legend { font-size: smaller; opacity: 0.8; }
.depth-track-bg { background-color: alpha(currentColor, 0.12); border-radius: 4px; }
.depth-track-fill { background-color: #4a90e2; border-radius: 4px; }
.depth-track-dim { opacity: 0.35; }
.marker-actuation { background-color: #2ecc71; }
.marker-release { background-color: #e6991a; }
"""


# --- Simulated depth stream + capture mode (models the missing D-Bus surface) ---


class SimDepth:
    """Stands in for a live-depth D-Bus subscription. `value` is 0-255,
    `None` while capture mode is "digital" (no depth sensor active)."""

    def __init__(self):
        self.value: int | None = 0
        self._subs: list = []
        self._timer_id: int | None = None
        self._t = 0.0

    def subscribe(self, cb) -> None:
        self._subs.append(cb)
        cb(self.value)

    def unsubscribe(self, cb) -> None:
        if cb in self._subs:
            self._subs.remove(cb)

    def set_value(self, v: int | None) -> None:
        self.value = None if v is None else max(0, min(255, int(v)))
        for cb in list(self._subs):
            cb(self.value)

    def set_auto(self, on: bool) -> None:
        if on and self._timer_id is None:
            self._timer_id = GLib.timeout_add(33, self._tick)  # ~30Hz, the modeled rate limit
        elif not on and self._timer_id is not None:
            GLib.source_remove(self._timer_id)
            self._timer_id = None

    def _tick(self) -> bool:
        self._t += 0.06
        self.set_value(int((math.sin(self._t) + 1) / 2 * 255))
        return True


class CaptureModeState:
    def __init__(self):
        self.mode = "analog"
        self._subs: list = []

    def subscribe(self, cb) -> None:
        self._subs.append(cb)
        cb(self.mode)

    def toggle(self) -> None:
        self.mode = "digital" if self.mode == "analog" else "analog"
        for cb in list(self._subs):
            cb(self.mode)


# --- Shared low-level primitive: value <-> pixel math + fill/track drawing ---
# (Deliberately NOT shared: marker count, units, chrome, digital-mode
# handling, discoverability — those are exactly what each variant disagrees
# about, per the prototype skill's "don't share the layout" rule.)


TRACK_WIDTH = 320


class DepthTrack(Gtk.Overlay):
    """A horizontal 0-255 track built from plain widgets (this environment's
    pycairo has no `gi._gi_cairo` bridge, so `Gtk.DrawingArea` draw funcs
    can't take a `cairo.Context` here — plain boxes sidestep that and are
    arguably more portable for a prototype anyway). `markers` is a live list
    of {"value": int, "css": str, "draggable": bool} dicts; dragging updates
    the nearest draggable marker's "value" in place and calls
    `on_marker_moved(index, value)`."""

    def __init__(self, markers: list[dict], on_marker_moved, height: int = 16):
        super().__init__()
        self.markers = markers
        self.on_marker_moved = on_marker_moved
        self.height = height
        self.live_value: int | None = 0
        # TRACK_WIDTH is only a *minimum*/pre-realize fallback — the user
        # wants the bar as wide as the window, so it hexpands to fill its
        # container and all pixel math below reads the real allocated width
        # via `_track_width()` rather than the constant (the first version
        # of this fix pinned halign=START to make the constant true, which
        # fought the width the user actually asked for).
        self.set_size_request(TRACK_WIDTH, height)
        self.set_hexpand(True)

        track_bg = Gtk.Box(css_classes=["depth-track-bg"], hexpand=True)
        track_bg.set_size_request(TRACK_WIDTH, height)
        self.set_child(track_bg)

        self.fill = Gtk.Box(css_classes=["depth-track-fill"], halign=Gtk.Align.START, valign=Gtk.Align.FILL)
        self.fill.set_size_request(0, height)
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
        self.add_controller(drag)
        self._drag_index: int | None = None
        self._drag_start_value = 0

        # No generic "my allocated size changed" signal exists on a plain
        # Gtk.Widget in GTK4 (that's a LayoutManager concern) — a cheap
        # 200ms resync while mapped is simpler than a custom LayoutManager
        # for a throwaway prototype, and keeps the bar/markers correct
        # across an interactive window resize, not just at initial map.
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
        return self.get_width() or TRACK_WIDTH

    def set_live_value(self, v: int | None) -> None:
        self.live_value = v
        self.fill.set_size_request(round(self._value_to_x(v)) if v is not None else 0, self.height)
        self.fill.set_visible(v is not None)

    def sync_markers(self) -> None:
        """Repositions every marker widget from `self.markers`' current
        values — needed after an external mutation (e.g. a "reset to
        default" button) or a resize, since dragging is the only other path
        that moves a marker widget."""
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


def make_scrubber(depth: SimDepth) -> Gtk.Widget:
    """"The physical key", simulated — drag to push a live depth value,
    or let it auto-sweep. Stands in for the real capture path."""
    row = Gtk.Box(spacing=8)
    scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 0, 255, 1)
    scale.set_hexpand(True)
    scale.set_value(0)
    scale.connect("value-changed", lambda s: depth.set_value(int(s.get_value())))
    row.append(Gtk.Label(label="Simulated key ⤵"))
    row.append(scale)
    auto_btn = Gtk.ToggleButton(label="Auto-sweep")

    def on_auto(b):
        on = b.get_active()
        depth.set_auto(on)
        scale.set_sensitive(not on)

    auto_btn.connect("toggled", on_auto)
    row.append(auto_btn)
    return row


def make_mode_toggle(capture_mode: CaptureModeState) -> Gtk.Widget:
    btn = Gtk.Button(label="Flip capture mode (demo)")
    btn.connect("clicked", lambda b: capture_mode.toggle())
    return btn


# --- Variant A: inline, one marker, raw units, hysteresis derived ---

RELEASE_GAP = 16  # fixed hysteresis gap; not independently settable in A


def build_variant_a(state: dict) -> Gtk.Widget:
    depth: SimDepth = state["depth"]
    capture_mode: CaptureModeState = state["capture_mode"]

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.add_css_class("actuation-section")
    box.append(Gtk.Label(label="Actuation point", xalign=0, css_classes=["heading"]))

    actuation = state["actuation"].setdefault(PROTOTYPE_INPUT, {"actuation": 128, "release": 112})
    markers = [{"value": actuation["actuation"], "css": "marker-actuation", "draggable": True}]
    track = DepthTrack(markers, on_marker_moved=lambda i, v: None)
    value_label = Gtk.Label(xalign=0)
    value_label.add_css_class("dim")

    def refresh_label():
        a = markers[0]["value"]
        rel = max(0, a - RELEASE_GAP)
        value_label.set_label(f"Actuation {a} / 255  ·  Release {rel} (auto, -{RELEASE_GAP})")

    def on_moved(i, v):
        actuation["actuation"] = v
        actuation["release"] = max(0, v - RELEASE_GAP)
        refresh_label()
        print(f"[variant A] {PROTOTYPE_INPUT} actuation={actuation}")

    track.on_marker_moved = on_moved
    refresh_label()
    box.append(track)
    box.append(value_label)

    note = Gtk.Label(xalign=0, wrap=True)
    note.add_css_class("digital-note")

    def on_mode(mode):
        digital = mode == "digital"
        note.set_visible(digital)
        note.set_label("Analog capture unavailable — bar shows the last-known value; drag disabled.")
        track.set_sensitive(not digital)
        depth.subscribe(track.set_live_value) if not digital else None

    capture_mode.subscribe(on_mode)
    box.append(note)
    box.append(make_scrubber(depth))
    box.append(make_mode_toggle(capture_mode))
    depth.subscribe(track.set_live_value)

    box.append(
        Gtk.Label(
            label="Fold-in note: one draggable marker in binding_editor.py's per-Input section; "
            "release stays implicit, never shown as a number a user can be wrong about.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )
    return box


# --- Variant B: inline, two explicit markers, percentage units, badge ---


def build_variant_b(state: dict) -> Gtk.Widget:
    depth: SimDepth = state["depth"]
    capture_mode: CaptureModeState = state["capture_mode"]

    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.add_css_class("actuation-section")
    header = Gtk.Box(spacing=6)
    header.append(Gtk.Label(label="Actuation & release", css_classes=["heading"]))
    mode_badge = Gtk.Label(css_classes=["badge"])
    header.append(mode_badge)
    box.append(header)

    actuation = state["actuation"].setdefault(PROTOTYPE_INPUT, {"actuation": 128, "release": 112})
    markers = [
        {"value": actuation["actuation"], "css": "marker-actuation", "draggable": True},
        {"value": actuation["release"], "css": "marker-release", "draggable": True},
    ]
    track = DepthTrack(markers, on_marker_moved=lambda i, v: None)

    # The digital-mode note sits centered *over* the (greyed-out) track
    # rather than as a separate line beneath it, per the user's reaction to
    # the first pass.
    track_overlay = Gtk.Overlay(hexpand=True)
    track_overlay.set_child(track)
    note = Gtk.Label(
        label="No depth — analog capture unavailable",
        wrap=True,
        halign=Gtk.Align.CENTER,
        valign=Gtk.Align.CENTER,
        css_classes=["digital-note-overlay"],
    )
    track_overlay.add_overlay(note)

    value_label = Gtk.Label(xalign=0)
    value_label.add_css_class("dim")
    legend = Gtk.Label(xalign=0, use_markup=True, css_classes=["marker-legend"])
    legend.set_markup(
        '<span foreground="#2ecc71">green</span> = actuation (fires Down)   ·   '
        '<span foreground="#e6991a">amber</span> = release (fires Up)'
    )

    def pct(v: int) -> str:
        return f"{round(v / 255 * 100)}%"

    def refresh_label():
        value_label.set_label(f"Actuation {pct(markers[0]['value'])}   Release {pct(markers[1]['value'])}")

    def on_moved(i, v):
        if i == 0 and v <= markers[1]["value"]:
            v = markers[1]["value"] + 1
            markers[0]["value"] = v
        if i == 1 and v >= markers[0]["value"]:
            v = markers[0]["value"] - 1
            markers[1]["value"] = v
        actuation["actuation"], actuation["release"] = markers[0]["value"], markers[1]["value"]
        refresh_label()
        print(f"[variant B] {PROTOTYPE_INPUT} actuation={actuation}")

    track.on_marker_moved = on_moved
    refresh_label()
    box.append(track_overlay)
    box.append(legend)
    box.append(value_label)

    reset_btn = Gtk.Button(label="Reset to Profile default (55% / 44%)")

    def on_reset(b):
        markers[0]["value"], markers[1]["value"] = 128, 112
        track.sync_markers()
        actuation["actuation"], actuation["release"] = 128, 112
        refresh_label()

    reset_btn.connect("clicked", on_reset)
    box.append(reset_btn)

    def on_mode(mode):
        digital = mode == "digital"
        mode_badge.set_label("digital" if digital else "analog")
        mode_badge.remove_css_class("badge-analog")
        mode_badge.remove_css_class("badge-digital")
        mode_badge.add_css_class("badge-digital" if digital else "badge-analog")
        note.set_visible(digital)
        track.add_css_class("depth-track-dim") if digital else track.remove_css_class("depth-track-dim")
        track.set_sensitive(not digital)

    capture_mode.subscribe(on_mode)
    box.append(make_scrubber(depth))
    box.append(make_mode_toggle(capture_mode))
    depth.subscribe(track.set_live_value)

    box.append(
        Gtk.Label(
            label="Fold-in note: badge on any Input that supports analog is the discoverability "
            "answer here — appears in Device Overview too, not just this editor.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )
    return box


# --- Variant C: separate calibration overview across all 20 keys ---

PRESETS = {"Light": (90, 78), "Medium": (128, 112), "Heavy": (180, 160)}


def build_variant_c(state: dict, parent_window: Gtk.Window) -> Gtk.Widget:
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    box.add_css_class("actuation-section")
    box.append(Gtk.Label(label="Actuation", xalign=0, css_classes=["heading"]))
    box.append(
        Gtk.Label(
            label="Per-key tuning lives in one place, not scattered across 20 editor popovers.",
            xalign=0,
            wrap=True,
            css_classes=["dim"],
        )
    )
    open_btn = Gtk.Button(label="Calibrate actuation…")
    open_btn.connect("clicked", lambda b: open_calibration_dialog(state, parent_window))
    box.append(open_btn)
    return box


def open_calibration_dialog(state: dict, parent: Gtk.Window) -> None:
    depth: SimDepth = state["depth"]
    capture_mode: CaptureModeState = state["capture_mode"]

    win = Gtk.Window(transient_for=parent, modal=True, title="Calibrate actuation")
    win.set_default_size(420, 420)
    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
    outer.set_margin_top(10)
    outer.set_margin_bottom(10)
    outer.set_margin_start(10)
    outer.set_margin_end(10)
    win.set_child(outer)

    mode_note = Gtk.Label(xalign=0, wrap=True, css_classes=["digital-note"])
    outer.append(mode_note)

    grid = Gtk.Grid(row_spacing=4, column_spacing=4)
    outer.append(grid)
    mini_bars: dict[str, Gtk.LevelBar] = {}
    for r in range(4):
        for c in range(5):
            inp = f"grid_r{r + 1}c{c + 1}"
            cell = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
            lbl = Gtk.Label(label=input_label(inp), css_classes=["dim"])
            lvl = Gtk.LevelBar(min_value=0, max_value=255)
            lvl.set_size_request(48, 10)
            cell.append(lbl)
            cell.append(lvl)
            grid.attach(cell, c, r, 1, 1)
            mini_bars[inp] = lvl

    def on_depth(v):
        mode_note.set_visible(v is None)
        mode_note.set_label("No depth — Daemon is in digital mode. Showing each key's last saved point.")
        if v is not None:
            mini_bars[PROTOTYPE_INPUT].set_value(v)

    depth.subscribe(on_depth)

    detail = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    outer.append(Gtk.Separator())
    outer.append(Gtk.Label(label=f"Selected: {input_label(PROTOTYPE_INPUT)}", xalign=0, css_classes=["heading"]))
    outer.append(detail)

    actuation = state["actuation"].setdefault(PROTOTYPE_INPUT, {"actuation": 128, "release": 112})
    advanced_revealer = Gtk.Revealer()
    raw_label = Gtk.Label(xalign=0, css_classes=["dim"])

    def apply_preset(name):
        actuation["actuation"], actuation["release"] = PRESETS[name]
        raw_label.set_label(f"raw: actuation={actuation['actuation']} release={actuation['release']}")
        print(f"[variant C] {PROTOTYPE_INPUT} preset={name} -> {actuation}")

    preset_row = Gtk.Box(spacing=6)
    for name in PRESETS:
        btn = Gtk.Button(label=name)
        btn.connect("clicked", lambda b, n=name: apply_preset(n))
        preset_row.append(btn)
    detail.append(preset_row)

    advanced_toggle = Gtk.ToggleButton(label="Advanced (raw value)")
    advanced_toggle.connect("toggled", lambda b: advanced_revealer.set_reveal_child(b.get_active()))
    detail.append(advanced_toggle)
    advanced_revealer.set_child(raw_label)
    detail.append(advanced_revealer)
    raw_label.set_label(f"raw: actuation={actuation['actuation']} release={actuation['release']}")

    def on_close(*_a):
        depth.unsubscribe(on_depth)

    win.connect("close-request", lambda w: (on_close(), False)[1])
    win.present()


# --- Switcher chrome ---

VARIANTS = [
    ("A", "Inline · one marker · raw 0-255"),
    ("B", "Inline · two markers · percent + badge"),
    ("C", "Separate calibration overview · presets"),
]


def build_window(app: Gtk.Application) -> Gtk.ApplicationWindow:
    provider = Gtk.CssProvider()
    provider.load_from_data(CSS.encode())
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
    )

    win = Gtk.ApplicationWindow(application=app, title="Ticket 19 prototype — trigger point UX")
    win.set_default_size(420, 640)

    stub = DaemonStub()
    config = stub.get_config()
    state = {
        "depth": SimDepth(),
        "capture_mode": CaptureModeState(),
        "actuation": {},  # models GetConfig()'s not-yet-serialized default_actuation/actuation_overrides
    }

    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
    win.set_child(outer)

    content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    content.set_margin_top(8)
    content.set_margin_bottom(8)
    content.set_margin_start(8)
    content.set_margin_end(8)
    outer.append(content)

    index = {"i": 0}

    def render():
        clear_children(content)
        editor = build_binding_editor(stub, config, "Default", "base", PROTOTYPE_INPUT, on_saved=lambda: None)
        content.append(editor)
        key, label = VARIANTS[index["i"]]
        if key == "A":
            content.append(build_variant_a(state))
        elif key == "B":
            content.append(build_variant_b(state))
        else:
            content.append(build_variant_c(state, win))
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
    app = Gtk.Application(application_id="com.acheron.prototype.ticket19")
    app.connect("activate", lambda a: build_window(a).present())
    app.run(None)


if __name__ == "__main__":
    main()
