"""The GTK4 `Gtk.Application` wiring Device Overview to a `DaemonClient` —
by default the real `DBusDaemonClient` against `com.acheron.Daemon`, per
ticket 16. On launch, opens straight to Device Overview reflecting
`GetConfig()`/`GetState()` — no separate onboarding wizard, even against
the all-passthrough seed `Default` Profile (issue 11's first-run answer).

Ticket 18: also subscribes to `ActiveLayerChanged` so a real Mode-key
hold/release (while the active Profile's `mode_key_role` is `LayerSwitch`)
flips `ui_state["selected_layer"]` and rebuilds — the "GUI's tab indicator
flips to Held" half of the live demo.
"""

from __future__ import annotations

import sys

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gdk, Gtk

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
            ui_state["selected_layer"] = layer
            rebuild()

        self._client.subscribe_layer_changed(on_layer_changed)

        rebuild()
        win.set_child(content_box)
        win.present()


def main() -> None:
    app = AcheronApplication()
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
