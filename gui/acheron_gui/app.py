"""The GTK4 `Gtk.Application` wiring Device Overview to a `DaemonClient` —
by default the real `DBusDaemonClient` against `com.acheron.Daemon`, per
ticket 16. On launch, opens straight to Device Overview reflecting
`GetConfig()`/`GetState()` — no separate onboarding wizard, even against
the all-passthrough seed `Default` Profile (issue 11's first-run answer).

Ticket 18: also subscribes to `ActiveLayerChanged` so a real Mode-key
hold/release (while the active Profile's `mode_key_role` is `LayerSwitch`)
flips `ui_state["selected_layer"]` and rebuilds — the "GUI's tab indicator
flips to Held" half of the live demo.

Ticket 19: also subscribes to `ActiveProfileChanged` and rebuilds on it —
covers a switch made from the tray's own real (non-GUI-process) icon, or any
other D-Bus client, not just this window's own sidebar/tray-mock controls.
`rebuild()` re-fetches `GetState()` on every call, so the newly active
Profile is picked up with no separate `ui_state` key needed (unlike
`selected_layer`, which is view state with no Daemon-side equivalent to
re-fetch).

Both signal callbacks defer their rebuild to `GLib.idle_add` rather than
calling it inline. `Gio.DBusProxy.call_sync` (every `DaemonClient` mutation)
blocks by iterating the same `GMainContext` a `"g-signal"` callback also
runs on — so calling `rebuild()` (more blocking `call_sync`s) directly from
inside a signal callback nests a second synchronous D-Bus round-trip inside
the first one's still-unfinished wait. This is a real, reproduced hang for
`SwitchProfile`: the Daemon used to emit `ActiveProfileChanged` before
replying, so the GUI's own blocking `SwitchProfile` call could see the
signal — and run this callback — before that same call had returned.
Deferring via `idle_add` lets the in-flight call unwind first (the Daemon
was also fixed to reply before signaling, but this GUI-side guard is kept
too, since any future signal wired to fire around a client's own in-flight
call would hit the identical hazard).

Ticket 25: `_wire_focus_tracking` drives ticket 24's `SetOutputSuppressed`
from this window's own live `is-active` state, closing the ticket 22 freeze
and ticket 23 stray-output risk end-to-end rather than leaving them reachable
only via a manual D-Bus call.
"""

from __future__ import annotations

import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
gi.require_version("GLib", "2.0")
from gi.repository import Gdk, Gtk, GLib

from .daemon_client import DaemonClient, DBusDaemonClient
from .device_overview import build_main_view
from .gtk_utils import clear_children

CSS = """
.heading { font-weight: bold; }
.dim { opacity: 0.6; font-size: smaller; }
.sidebar { padding: 8px; background-color: alpha(currentColor, 0.06); border-radius: 6px; }
/* Gtk.MenuButton's CSS node is named "menubutton", not "button" — these must
   be bare class selectors, not element-qualified, or they silently never match. */
.bound { border: 2px solid #4caf50; }
.empty { opacity: 0.75; }
.mode-key, .mode-key > button { border-radius: 999px; }
.tray-mock { border: 1px dashed alpha(currentColor, 0.35); border-radius: 8px; padding: 8px; }
.error { color: #e53935; font-size: smaller; }
"""


def _wire_focus_tracking(window, client: DaemonClient) -> None:
    """Pushes `window`'s live focus state to `client.set_output_suppressed`
    on every change, plus once immediately — covering the GUI launching
    already focused, not just later transitions. `notify::is-active` carries
    no value of its own (it's a GTK-computed, read-only property; nothing
    can set it directly, in production or in a test), so the same handler
    reads the current state back via `is_active()` and is also called
    directly here for the initial push, rather than duplicating the read in
    a separate one-shot call that could drift out of sync with it.

    On every transition *to* focused, also calls `client.stop_all_toggles()`
    right after suppressing — a deliberate, separate guard live-hardware
    testing showed suppression alone can't provide (ticket 25): a Toggle
    already outputting a real held key *before* the GUI gains focus has
    already armed the OS's own key-repeat for it, and suppression only gates
    *future* writes, so it can't retroactively undo that. The two known
    freeze triggers this closes: (a) a Toggle already running when the GUI
    regains focus — no live key-repeat left for the window's activation
    handshake to race against; (b) a Toggle stopped *while* the GUI has
    focus — `stop_all_toggles`'s force-release always reaches `uinput` even
    while suppressed (`daemon/src/injector.rs`'s `ForceRelease`, unlike a
    plain `KeyState` write), so a key genuinely armed by a spontaneous
    `is-active` flicker (real, observed on this compositor even with no
    deliberate focus change) never gets left stuck down. No corresponding
    resume on focus-*loss*, by design: nobody wants a Toggle silently still
    running in the background just because the GUI happened to have focus
    when it started — matching `spec.md`'s Toggle-stop-conditions list,
    which now includes "the GUI's own window gains focus" alongside the
    same-key-press and Profile-switch conditions.

    The actual Daemon calls are deferred via `GLib.idle_add`, same as
    `on_layer_changed`/`on_profile_changed` above and for the same reason:
    `notify::is-active` can fire while some other blocking `call_sync` (e.g.
    `switch_profile`) is still in flight, since both run on the same
    `GMainContext` — calling straight through here would nest a second
    blocking D-Bus round-trip inside the first one's still-unfinished wait,
    the identical hazard that module docstring describes.

    `window` is duck-typed (`is_active()` + `connect()`) rather than
    annotated `Gtk.Window`, so tests can drive it with a plain fake exposing
    the same two members instead of a real windowing system, which has no
    way to force `is-active` true/false headlessly.
    """

    def push_focus_state(*_args) -> None:
        focused = window.is_active()

        def apply():
            client.set_output_suppressed(focused)
            if focused:
                client.stop_all_toggles()
            return GLib.SOURCE_REMOVE

        GLib.idle_add(apply)

    window.connect("notify::is-active", push_focus_state)
    push_focus_state()


class AcheronApplication(Gtk.Application):
    def __init__(self, client: DaemonClient | None = None):
        super().__init__(application_id="com.acheron.gui")
        self._client = client or DBusDaemonClient()

    def do_activate(self):
        provider = Gtk.CssProvider()
        provider.load_from_string(CSS)
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )

        win = Gtk.ApplicationWindow(application=self, title="Acheron")
        win.set_default_size(920, 680)

        content_box = Gtk.Box()
        # GUI-only view state (not Daemon state) that must survive a
        # rebuild — otherwise reopening the Action Table sidebar, or
        # re-expanding one of its rows, and then editing a Binding would
        # snap them shut again, since rebuild() reconstructs the whole
        # widget tree (including a fresh Gtk.Revealer/Gtk.Expander, which
        # default closed) from scratch (ticket 09). `selected_layer` is the
        # same kind of view state (ticket 18) — which Layer Device
        # Overview/Action Table show/edit — except it's also kept in sync
        # with the Daemon's live Layer via the signal subscription below.
        ui_state = {"table_open": False, "expanded_rows": set(), "selected_layer": "base"}

        def rebuild():
            clear_children(content_box)
            config = self._client.get_config()
            profile, layer, _active_toggles, _device_connected = self._client.get_state()
            content_box.append(build_main_view(self._client, config, profile, layer, rebuild, ui_state))

        def on_layer_changed(layer: str):
            def apply():
                ui_state["selected_layer"] = layer
                rebuild()
                return GLib.SOURCE_REMOVE

            GLib.idle_add(apply)

        def on_profile_changed(_name: str):
            def apply():
                rebuild()
                return GLib.SOURCE_REMOVE

            GLib.idle_add(apply)

        self._client.subscribe_layer_changed(on_layer_changed)
        self._client.subscribe_profile_changed(on_profile_changed)
        _wire_focus_tracking(win, self._client)

        rebuild()
        win.set_child(content_box)
        win.present()


def main() -> None:
    app = AcheronApplication()
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
