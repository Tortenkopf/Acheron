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

        rebuild()
        win.set_child(content_box)
        win.present()


def main() -> None:
    app = AcheronApplication()
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
