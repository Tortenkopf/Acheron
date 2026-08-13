#!/usr/bin/env python3
"""
PROTOTYPE — throwaway code, not production. Do not import from real Acheron code.

Answers: where/how does the GUI surface Daemon-running + Device-connected
status (added in ticket 10's systemd correction and ticket 08's
`device_connected`/`DeviceConnectionChanged` correction)?
Ticket: .scratch/tartarus-keybinder/issues/12-design-daemon-device-status-indicators.md

Three structurally different variants, switchable via the floating bottom bar
(Left/Right arrow keys, or the arrow buttons — ignored while an Entry has
focus) — reusing Device Overview/DaemonStub/CSS from the ticket-09 prototype
rather than rebuilding it:

  A — Header badge only. A status chip above Device Overview; the tray mock
      is untouched (profile/layer only, no status). Grid stays fully
      clickable regardless of status — editing Bindings is just editing
      data, per ticket 03/09, and never required a live Daemon/device.
  B — Tray icon state only. No persistent indicator in the main window at
      all; the tray mock's icon/text/colour changes to reflect status. You
      have to check the tray to know. Grid stays fully clickable.
  C — Both, plus the grid visually disables itself (dimmed + a centered
      overlay message, via Gtk.Overlay) whenever status isn't
      running+connected. This is the one variant that also takes a stance on
      a second question the ticket's title doesn't ask outright: should
      editing be blocked while nothing's connected?

A `StatusStub` simulates the three reachable states (`device_connected` is
meaningless when the Daemon isn't running, so this is one 3-way state, not
two independent booleans) via a debug button row in the bottom bar — click
through them to see each variant react live, the same way ticket 09's
exploration let you click through layout variants.

Run:
    python3 prototype/12-daemon-device-status-indicators/prototype.py [--variant A|B|C]
"""

import argparse
import pathlib
import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gtk

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent / "09-gui-information-architecture"))
import prototype as ia  # ticket 09's GUI-IA prototype — reused, not duplicated

VARIANTS = ["A", "B", "C"]
VARIANT_NAMES = {
    "A": "A — Header badge only",
    "B": "B — Tray icon state only",
    "C": "C — Both + grid disables itself",
}

# (state_key, label, colour, tray glyph) — device_connected only means
# anything when the Daemon is running, so this is one 3-way state.
STATUS_STATES = [
    ("running_connected", "Connected", "#4caf50", "\U0001f3ae"),
    ("running_disconnected", "Daemon running — device disconnected", "#ff9800", "\U0001f50c"),
    ("not_running", "Daemon not running", "#f44336", "\U0001f480"),
]


class StatusStub:
    """Simulates the NameOwnerChanged watch (ticket 10) + device_connected /
    DeviceConnectionChanged (ticket 08's correction) — no real D-Bus here."""

    def __init__(self):
        self.index = 0

    def state(self):
        return STATUS_STATES[self.index]

    def set(self, i):
        self.index = i
        _, label, *_ = self.state()
        print(f"[status] -> {label}   (simulated NameOwnerChanged / DeviceConnectionChanged)")


STATUS = StatusStub()


def status_markup(status):
    _, label, colour, _glyph = status
    return f"<span foreground='{colour}'>●</span> {label}"


def build_status_badge(status):
    box = Gtk.Box(spacing=6)
    box.add_css_class("status-badge")
    lbl = Gtk.Label()
    lbl.set_markup(status_markup(status))
    box.append(lbl)
    box.set_margin_top(6)
    box.set_margin_bottom(2)
    box.set_margin_start(12)
    return box


CSS_EXTRA = """
.status-badge { font-size: 1.05em; }
.status-banner { padding: 6px 12px; border-radius: 6px; }
.dim-overlay { background-color: alpha(black, 0.45); border-radius: 6px; }
.dim-overlay-label { color: white; font-weight: bold; }
.switcher-bar {
    background-color: alpha(currentColor, 0.08);
    border-radius: 10px;
    padding: 6px 10px;
    box-shadow: 0 2px 6px alpha(black, 0.25);
}
"""


def build_variant_view(variant, state, status, on_change):
    """Builds Device Overview (via ticket 09's build_main_view, reused
    unmodified) then layers this ticket's status treatment on top per
    variant, rather than forking build_main_view itself."""
    ui_state = {"table_open": False}
    root = ia.build_main_view(state, on_change, ui_state)

    tray_box = root.get_last_child()
    tray_heading = tray_box.get_first_child()

    if variant == "A":
        badge = build_status_badge(status)
        outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        outer.append(badge)
        outer.append(root)
        return outer

    if variant == "B":
        tray_status_lbl = Gtk.Label()
        _, label, colour, glyph = status
        tray_status_lbl.set_markup(f"{glyph}  <span foreground='{colour}'>{label}</span>")
        tray_box.insert_child_after(tray_status_lbl, tray_heading)
        return root

    # Variant C — both, plus the grid disables itself when unavailable.
    badge = build_status_badge(status)
    tray_status_lbl = Gtk.Label()
    _, label, colour, glyph = status
    tray_status_lbl.set_markup(f"{glyph}  <span foreground='{colour}'>{label}</span>")
    tray_box.insert_child_after(tray_status_lbl, tray_heading)

    outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    outer.append(badge)

    healthy = status[0] == "running_connected"
    root.set_sensitive(healthy)
    if healthy:
        outer.append(root)
    else:
        overlay = Gtk.Overlay()
        overlay.set_child(root)
        dim = Gtk.Box(halign=Gtk.Align.FILL, valign=Gtk.Align.FILL)
        dim.add_css_class("dim-overlay")
        msg = Gtk.Label(label=("Daemon not running — start it to edit Bindings" if status[0] == "not_running"
                                else "Device disconnected — plug in the Tartarus Pro to edit Bindings"))
        msg.add_css_class("dim-overlay-label")
        # halign/valign alone only position a widget *within space it has
        # claimed* — without hexpand/vexpand it claims just its own natural
        # size, so the box packs it at the start instead of centering it.
        msg.set_hexpand(True)
        msg.set_vexpand(True)
        msg.set_halign(Gtk.Align.CENTER)
        msg.set_valign(Gtk.Align.CENTER)
        dim.append(msg)
        overlay.add_overlay(dim)
        outer.append(overlay)
    return outer


def build_bottom_bar(get_variant, set_variant, get_status_index, set_status_index, on_change):
    bar = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
    bar.add_css_class("switcher-bar")

    switch_row = Gtk.Box(spacing=10)
    prev_btn = Gtk.Button(label="◀")
    label = Gtk.Label(label=VARIANT_NAMES[get_variant()])
    next_btn = Gtk.Button(label="▶")

    def cycle(delta):
        i = VARIANTS.index(get_variant())
        i = (i + delta) % len(VARIANTS)
        set_variant(VARIANTS[i])
        label.set_label(VARIANT_NAMES[get_variant()])
        on_change()

    prev_btn.connect("clicked", lambda b: cycle(-1))
    next_btn.connect("clicked", lambda b: cycle(1))
    switch_row.append(prev_btn)
    switch_row.append(label)
    switch_row.append(next_btn)
    bar.append(switch_row)

    sim_row = Gtk.Box(spacing=6)
    sim_row.append(Gtk.Label(label="Simulate:"))
    for i, (_key, name, *_rest) in enumerate(STATUS_STATES):
        b = Gtk.Button(label=name)
        if i == get_status_index():
            b.add_css_class("suggested-action")

        def on_click(btn, i=i):
            set_status_index(i)
            on_change()

        b.connect("clicked", on_click)
        sim_row.append(b)
    bar.append(sim_row)

    return bar


class PrototypeApp(Gtk.Application):
    def __init__(self, initial_variant):
        super().__init__(application_id="com.acheron.prototype.status_indicators")
        self.variant = initial_variant

    def do_activate(self):
        provider = Gtk.CssProvider()
        provider.load_from_string(ia.CSS + CSS_EXTRA)
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )

        win = Gtk.ApplicationWindow(application=self, title="Acheron — Status Indicators Prototype (ticket 12)")
        win.set_default_size(960, 760)

        overlay = Gtk.Overlay()
        content_box = Gtk.Box()
        overlay.set_child(content_box)

        def rebuild():
            child = content_box.get_first_child()
            while child is not None:
                nxt = child.get_next_sibling()
                content_box.remove(child)
                child = nxt
            content_box.append(build_variant_view(self.variant, ia.STATE, STATUS.state(), rebuild))
            print(
                f"\n=== variant={self.variant} status={STATUS.state()[0]} "
                f"active_profile={ia.STATE.active_profile} active_layer={ia.STATE.active_layer} ==="
            )

        bottom_bar = build_bottom_bar(
            lambda: self.variant,
            lambda v: setattr(self, "variant", v),
            lambda: STATUS.index,
            STATUS.set,
            rebuild,
        )
        bottom_bar.set_halign(Gtk.Align.CENTER)
        bottom_bar.set_valign(Gtk.Align.END)
        bottom_bar.set_margin_bottom(14)
        overlay.add_overlay(bottom_bar)

        key_controller = Gtk.EventControllerKey()

        def on_key(controller, keyval, keycode, state):
            focus = win.get_focus()
            if focus is not None and (isinstance(focus, Gtk.Entry) or type(focus).__name__ == "Text"):
                return False
            if keyval == Gdk.KEY_Left:
                idx = VARIANTS.index(self.variant)
                self.variant = VARIANTS[(idx - 1) % len(VARIANTS)]
                rebuild()
                return True
            if keyval == Gdk.KEY_Right:
                idx = VARIANTS.index(self.variant)
                self.variant = VARIANTS[(idx + 1) % len(VARIANTS)]
                rebuild()
                return True
            return False

        key_controller.connect("key-pressed", on_key)
        win.add_controller(key_controller)

        rebuild()
        win.set_child(overlay)
        win.present()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--variant", choices=VARIANTS, default="A")
    args = parser.parse_args()
    app = PrototypeApp(args.variant)
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
