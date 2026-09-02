# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

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
covers a switch made from the tray icon's own Switch Profile submenu (ticket
36), or any other D-Bus client, not just this window's own sidebar.
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

Ticket 20: `_wire_status_tracking` subscribes to the Daemon-presence
(`NameOwnerChanged`) and `DeviceConnectionChanged` signals and keeps a live
`{"daemon_running", "device_connected"}` dict in sync, rebuilding on every
transition — the same deferred-via-`GLib.idle_add` pattern as the two
callbacks above, and for the same reentrancy reason. `rebuild()` only calls
`get_config()`/`get_state()` while `daemon_running` is true (those calls
would simply fail otherwise); while it isn't, or on a failed call racing a
just-reported disconnect, it keeps showing the last successfully fetched
Config (`device_overview.PLACEHOLDER_CONFIG` if there's never been one)
underneath the status chip's dimmed overlay — `build_status_wrapped_view`
disables the whole thing either way, so stale data being visible-but-inert
is harmless.

Ticket 21: `_ensure_daemon_started_on_launch` is the GUI's half of the
login-autostart-plus-safety-net design (spec.md "Packaging and lifecycle") —
called once, synchronously, at the start of the one-time `_build_main_window`
(ticket 105 moved the whole build out of `do_activate`, which `Gio` re-emits
on every secondary launch), before the status/focus wiring even subscribes. `systemd --user`'s own
`WantedBy=default.target` is the primary autostart trigger; this call only
exists to recover a Daemon that's crashed into `failed` (systemd's own
`StartLimitBurst` guard latches it there, per install.sh's unit) or that
somehow isn't running yet, without the user ever touching a terminal.

Ticket 105: `Gio.Application` emits `activate` on *every* invocation, not
just the first — a second `acheron-gui` process, or `gtk-launch
acheron.desktop`, hands the running primary an `activate` (only a GNOME
app-grid *click* is shell-serviced and skips D-Bus entirely). So the whole
build lives in `_build_main_window`, run once via the `self._main_window`
guard in `_activate_window`; `do_activate` itself now only re-presents that
one window — shown first, since `_wire_window_close_to_hide` may have
hidden it to the tray.
"""

from __future__ import annotations

import sys
from typing import Callable

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
gi.require_version("GLib", "2.0")
from gi.repository import Gdk, Gio, Gtk, GLib

from . import __version__
from .about_dialog import build_about_dialog
from .daemon_client import DaemonClient, DaemonError, DBusDaemonClient
from .device_overview import PLACEHOLDER_CONFIG, build_status_wrapped_view, compute_status
from .gtk_utils import clear_children
from .systemd_client import DBusSystemdClient, SystemdClient
from .tray import TrayIcon

CSS = """
.heading { font-weight: bold; }
.dim { opacity: 0.6; font-size: smaller; }
.sidebar { padding: 8px; background-color: alpha(currentColor, 0.06); border-radius: 6px; }
/* Gtk.MenuButton's CSS node is named "menubutton", not "button" — these must
   be bare class selectors, not element-qualified, or they silently never match. */
.bound { border: 2px solid #4caf50; }
.empty { opacity: 0.75; }
.mode-key { border-radius: 999px; }
.error { color: #e53935; font-size: smaller; }
.status-badge { font-size: 1.05em; }
.dim-overlay { background-color: alpha(black, 0.45); border-radius: 6px; }
.dim-overlay-label { color: white; font-weight: bold; }
/* Ticket 26: the real Actuation & release editor (binding_editor.py),
   ported from ticket 19's prototype/19-trigger-point-depth-ux variant B. */
.actuation-section { padding: 8px; margin-top: 4px; background-color: alpha(currentColor, 0.05); border-radius: 6px; }
.sub-heading { font-weight: bold; }
.badge { border-radius: 999px; padding: 0px 6px; font-size: smaller; font-weight: bold; }
.badge-analog { background-color: #2ecc71; color: black; }
.badge-digital { background-color: #c0392b; color: white; }
.digital-note-overlay { color: #e5883b; font-size: smaller; font-weight: bold; background-color: alpha(black, 0.55); border-radius: 4px; padding: 2px 8px; }
.marker-legend { font-size: smaller; opacity: 0.8; }
.depth-track-bg { background-color: alpha(currentColor, 0.12); border-radius: 4px; }
.depth-track-fill { background-color: #4a90e2; border-radius: 4px; }
.depth-track-dim { opacity: 0.35; }
.marker-actuation { background-color: #2ecc71; }
.marker-release { background-color: #e6991a; }
/* Ticket 42: the real key/mouse-button picker (key_picker.py), ported from
   ticket 32's winning variant A prototype. */
.warning { background-color: alpha(#e6991a, 0.18); border-radius: 6px; padding: 6px 8px; font-size: smaller; }
/* Ticket 55: the Stepper library's one-shot "Moved off '<name>'" steal
   notice (library_view.py), styled distinctly from .warning/.error since
   it's neither a problem nor a mistake, just an FYI. */
.toast { background-color: alpha(#4a90e2, 0.18); border-radius: 6px; padding: 6px 8px; font-size: smaller; }
.picker-panel { padding: 8px; background-color: alpha(currentColor, 0.05); border-radius: 6px; }
.section-label { font-size: smaller; opacity: 0.65; font-weight: bold; }
.keycap { min-height: 25px; padding: 2px 4px; font-size: 12px; }
.keycap-mod { background-color: alpha(#4a90e2, 0.22); }
.keycap-mouse { background-color: alpha(#8e44ad, 0.22); }
.keycap-mm { background-color: alpha(#27ae60, 0.22); }
/* Ticket 43: the real controller-button picker (controller_picker.py),
   ported from ticket 38's winning variant A prototype. */
.pad-body { background-color: alpha(currentColor, 0.06); border-radius: 18px; }
.padbtn { min-width: 0; min-height: 0; font-size: 11px; padding: 2px 4px; }
.padbtn-face { background-color: alpha(#4a90e2, 0.22); }
.padbtn-shoulder { background-color: alpha(#e67e22, 0.22); }
.padbtn-stick { background-color: alpha(#8e44ad, 0.22); }
.padbtn-dpad { background-color: alpha(#27ae60, 0.22); }
/* Ticket 40: the real Chord recording flow (device_overview.py) — a
   distinct border colour per state, matching the ticket 30 prototype's own
   green-selected/amber-previewed scheme (.bound already claims a plain
   green border for an ordinary Binding, so selected uses a heavier one to
   stay visually distinct from it). */
.chord-selected { border: 3px solid #2ecc71; }
.chord-preview { border: 3px solid #e6991a; }
/* Ticket 71: the real axis-target diagram picker (axis_picker.py) and
   Device Overview's Axis-assigned grid-key treatment — one shared purple
   accent (#8e44ad, ticket 60's Answer) ties the picker's "current pick"
   highlight to the grid's "this key is Axis-assigned" stripe. */
.axis-target-current { background-color: alpha(#8e44ad, 0.45); }
.axis-stripe {
    background-image: repeating-linear-gradient(
        45deg, alpha(#8e44ad, 0.35) 0px, alpha(#8e44ad, 0.35) 4px,
        transparent 4px, transparent 8px
    );
}
/* Ticket 102: the About dialog (about_dialog.py). */
.about-title { font-size: 1.6em; font-weight: bold; }
.quote { font-style: italic; opacity: 0.85; }
/* tartarus-status-leds ticket 04: the Status LEDs lozenge group in the
   Device Overview (device_overview.py::build_status_leds_section). Lit =
   full-saturation colour fill + white border + glow; unlit = heavily
   desaturated + flat. The lit/unlit contrast is the primary state signal,
   so it's deliberately strong, not a faint brightness shift. `.status-led`
   plus a per-colour class, with `.lit` toggled from the active Profile's
   stored `status_leds`. */
.status-led { min-width: 52px; min-height: 22px; padding: 0; border-radius: 999px; border: 2px solid transparent; opacity: 0.4; }
.status-led.lit { opacity: 1; border-color: #ffffff; }
.status-led-orange { background-image: none; background-color: alpha(#ff9800, 0.14); }
.status-led-green { background-image: none; background-color: alpha(#4caf50, 0.14); }
.status-led-blue { background-image: none; background-color: alpha(#2196f3, 0.14); }
.status-led-orange.lit { background-color: #ff9800; box-shadow: 0 0 8px 2px alpha(#ff9800, 0.75); }
.status-led-green.lit { background-color: #4caf50; box-shadow: 0 0 8px 2px alpha(#4caf50, 0.75); }
.status-led-blue.lit { background-color: #2196f3; box-shadow: 0 0 8px 2px alpha(#2196f3, 0.75); }
"""


def _build_primary_menu() -> Gio.Menu:
    """Ticket 102: the header-bar primary menu. One item today ("About
    Acheron" -> the `app.about` action added in `_build_main_window`), but a
    `Gio.Menu` rather than a bare button because it's the intended home for
    future global actions (Quit, Preferences)."""
    menu = Gio.Menu()
    menu.append("About Acheron", "app.about")
    return menu


def _about_dialog_state(client: DaemonClient, daemon_running: bool) -> dict | None:
    """The `GetState()` snapshot the About dialog reads its Daemon version
    and the connected device's firmware/serial from — `None` when the
    Daemon isn't running, or when the call races its disappearance, so the
    dialog shows "not running" / "Not connected" instead of erroring.

    Split out and tested against `DaemonStub` for the same reason the other
    `app.py` helpers are: the real menu-action wiring in `_build_main_window`
    needs a registered `Gtk.Application` and a live session bus to drive."""
    if not daemon_running:
        return None
    try:
        return client.get_state()
    except (DaemonError, GLib.Error):
        return None


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


def _wire_status_tracking(client: DaemonClient, on_change: Callable[[], None]) -> dict:
    """Subscribes to ticket 20's Daemon-presence and device-connection
    signals (plus ticket 26's `CaptureModeChanged`) and keeps a live
    `{"daemon_running": bool, "device_connected": bool, "capture_mode":
    str}` dict in sync, calling `on_change()` on every transition.

    Both start `False` — conservative until the first real signal lands,
    since `subscribe_daemon_running_changed`'s underlying `Gio.bus_watch_name`
    resolves the actual state asynchronously rather than synchronously
    within this call (see `daemon_client.DBusDaemonClient`). A vanished
    Daemon also forces `device_connected` back to `False`: there's no live
    poll loop to ask once it's gone, and "not running" already implies
    "not connected" per `device_overview.compute_status`.

    Deferred via `GLib.idle_add`, same reentrancy guard `_wire_focus_tracking`
    documents: both signals can arrive while some other blocking `call_sync`
    is still in flight, since callbacks and `call_sync` share one
    `GMainContext`.

    Standalone/testable the same way `_wire_focus_tracking` is — a fake
    Daemon (`DaemonStub`) drives it directly via `simulate_daemon_stopped`/
    `_started`/`simulate_device_disconnected`/`_connected`, no real
    `Gtk.Application` main loop needed.
    """
    status = {"daemon_running": False, "device_connected": False, "capture_mode": "digital"}

    def on_running_changed(running: bool) -> None:
        def apply():
            status["daemon_running"] = running
            if not running:
                status["device_connected"] = False
            on_change()
            return GLib.SOURCE_REMOVE

        GLib.idle_add(apply)

    def on_device_connection_changed(connected: bool) -> None:
        def apply():
            status["device_connected"] = connected
            on_change()
            return GLib.SOURCE_REMOVE

        GLib.idle_add(apply)

    def on_capture_mode_changed(mode: str) -> None:
        # Ticket 26: the Actuation & release editor's badge needs to flip
        # live while an editor popover is open. A full `rebuild()` (same as
        # every other live signal here) is the simplest way to get that —
        # unlike depth (~30Hz, handled by `binding_editor.py`'s own
        # map/unmap-scoped `StartDepthStream` subscription instead), a
        # capture-mode transition is rare enough that closing whatever
        # popover happens to be open is an acceptable, already-established
        # tradeoff (every other status transition here does the same).
        def apply():
            status["capture_mode"] = mode
            on_change()
            return GLib.SOURCE_REMOVE

        GLib.idle_add(apply)

    client.subscribe_daemon_running_changed(on_running_changed)
    client.subscribe_device_connection_changed(on_device_connection_changed)
    client.subscribe_capture_mode_changed(on_capture_mode_changed)
    return status


def _wire_window_close_to_hide(window) -> None:
    """Ticket 36's minimize-to-tray: the titlebar close button hides the
    main window instead of destroying it. Returning `True` from
    `"close-request"` stops GTK's default handler, which would otherwise
    destroy the window and drop it from the `Gtk.Application`'s own window
    list — and since that list is what `GApplication`'s inherited hold
    count is keyed to, destroying the last window would quit the whole
    process. A hidden-but-still-added window keeps that hold count
    non-zero, so no separate "suppress quit-on-last-window-closed" step is
    needed beyond this — only the tray menu's own Quit item (`self.quit()`)
    actually exits the GUI process now; the Daemon is unaffected either
    way, per ticket 11's original design (it's already an independent
    `systemd --user` service, not something this process owns).

    `window` is duck-typed (`connect()` + `set_visible()`) rather than
    annotated `Gtk.Window`, matching `_wire_focus_tracking`'s own reasoning:
    tests drive it with a plain fake rather than a real windowing system,
    which has no headless way to simulate a titlebar close.
    """

    def on_close_request(_window) -> bool:
        window.set_visible(False)
        return True

    window.connect("close-request", on_close_request)


def _ensure_daemon_started_on_launch(systemd_client: SystemdClient) -> None:
    """Ticket 21's GUI-side safety net: on its own launch, ask systemd to
    clear any latched `failed` state and (re)start the Daemon unit —
    `ResetFailedUnit` then `StartUnit`, over the session D-Bus connection, no
    `systemctl` shell-out. Best-effort on failure (e.g. the unit isn't
    installed yet, or systemd is unreachable): this must never block the GUI
    from opening, since the status chip (ticket 20) already gives the user a
    live answer if the Daemon still isn't up afterward.

    Failures are still printed to stderr rather than swallowed outright — a
    live crash-recovery demo (deliberately tripping `StartLimitBurst`, then
    relaunching the GUI) caught a real bug here (the wrong systemd method
    name, `systemd_client.py`'s docstring has the story) that a fully silent
    `except: pass` would have made much slower to notice, since the Daemon
    just stayed down with no error visible anywhere.
    """
    try:
        systemd_client.ensure_daemon_started()
    except GLib.Error as err:
        print(f"acheron-gui: could not ensure the Daemon is started: {err}", file=sys.stderr)


def _present_window(win) -> None:
    """Re-surface the main window. It may be hidden to the tray rather than
    destroyed (`_wire_window_close_to_hide` keeps it added to the app but
    invisible), so it's explicitly shown before being presented — not just
    `present()`'d, which is what ticket 90's launcher check expected on a
    second launch and ticket 105 found it wasn't getting.

    `win` is duck-typed (`set_visible()` + `present()`) so tests can drive
    it with a plain fake, same reasoning as `_wire_focus_tracking`'s own
    `window` parameter.
    """
    win.set_visible(True)
    win.present()


def _activate_window(existing, build, present):
    """`Gio.Application` emits `activate` on *every* invocation — the first
    launch, but also a second `acheron-gui` process or `gtk-launch
    acheron.desktop` handing off to the already-running primary (ticket
    105). Only the first activation builds anything: `build()` creates the
    window and, on the way, the tray icon, CSS provider, D-Bus
    subscriptions and daemon-start safety net — all genuinely
    once-per-process (re-running it re-exported the tray's SNI object and
    raised `g-io-error-quark`, aborting before the window was ever raised
    and leaking a zombie second `ApplicationWindow`). Every later
    activation just re-surfaces the window that already exists.

    Split out as a plain function, tested with fakes, for the same reason
    `_wire_focus_tracking` and friends are: driving the real `do_activate`
    would need a registered `Gtk.Application`, a live session bus for the
    tray, and a mapped top-level window — none of which a headless test
    has.
    """
    win = existing if existing is not None else build()
    present(win)
    return win


class AcheronApplication(Gtk.Application):
    def __init__(
        self,
        client: DaemonClient | None = None,
        systemd_client: SystemdClient | None = None,
        tray_bus=None,
    ):
        super().__init__(application_id="com.acheron.gui")
        self._client = client or DBusDaemonClient()
        self._systemd_client = systemd_client or DBusSystemdClient()
        # Injectable session bus for the tray icon (mirrors `client`/
        # `systemd_client` above) — only set by tests, which must never
        # register a throwaway SNI on the developer's real panel.
        self._tray_bus = tray_bus
        # The one main window, created lazily on the first `do_activate`
        # and reused by every later one (ticket 105).
        self._main_window = None

    def do_activate(self):
        self._main_window = _activate_window(
            self._main_window, self._build_main_window, _present_window
        )

    def _build_main_window(self):
        """The one-time setup: window, tray icon, CSS, D-Bus subscriptions
        and the daemon-start safety net. Run exactly once per process — see
        `_activate_window`."""
        _ensure_daemon_started_on_launch(self._systemd_client)

        provider = Gtk.CssProvider()
        provider.load_from_string(CSS)
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(), provider, Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        )

        win = Gtk.ApplicationWindow(application=self, title="Acheron")
        # Ticket 88: grown from 920×680 so the larger fixed-size Device
        # Overview row (100×100 grid/wheel/diamond/Mode key, 150×100 key 20 —
        # ticket 87) plus the 220px Profile sidebar and 220px Chords section
        # all fit at first launch without GTK shrinking any button below its
        # new fixed size. Measured live against the real running window.
        win.set_default_size(1400, 860)
        _wire_window_close_to_hide(win)

        # Ticket 102: a HeaderBar titlebar carrying the primary menu — just
        # "About Acheron" today, but built as a `Gio.Menu` since it's the
        # intended home for future global actions. The window's close button
        # moves into this header bar; `_wire_window_close_to_hide` above
        # still catches `close-request` (a window signal, independent of the
        # titlebar widget), so ticket 36's minimize-to-tray is unaffected.
        header = Gtk.HeaderBar()
        menu_button = Gtk.MenuButton(
            icon_name="open-menu-symbolic", menu_model=_build_primary_menu()
        )
        menu_button.set_tooltip_text("Main menu")
        header.pack_end(menu_button)
        win.set_titlebar(header)

        # Ticket 36: the real system tray icon — a standalone D-Bus service
        # (`org.kde.StatusNotifierItem` + `com.canonical.dbusmenu`), not a
        # widget in `content_box`'s own tree. Held on `self` so it outlives
        # `do_activate` (nothing else keeps it referenced otherwise); kept
        # in sync by `rebuild()`'s own `update()` call below rather than any
        # D-Bus subscriptions of its own, per ticket 36's design.
        self._tray_icon = TrayIcon(
            self._client,
            self._systemd_client,
            lambda: _present_window(win),
            self.quit,
            bus=self._tray_bus,
        )

        content_box = Gtk.Box()
        # GUI-only view state (not Daemon state) that must survive a
        # rebuild — otherwise switching to the Library destination, or
        # switching back to Grid, would snap back to Grid every time, since
        # rebuild() reconstructs the whole widget tree from scratch (ticket
        # 09; the Grid/Library switcher itself is ticket 48). `selected_layer`
        # is the same kind of view state (ticket 18) — which Layer Device
        # Overview shows/edits — except it's also kept in sync with the
        # Daemon's live Layer via the signal subscription below.
        ui_state = {"dest": "grid", "selected_layer": "base"}
        # Last successfully fetched Config/profile/layer (ticket 20) — kept
        # so the dimmed grid still has *something* to render while the
        # Daemon isn't reachable, rather than Device Overview vanishing
        # outright. `PLACEHOLDER_CONFIG` covers the one gap this can't: the
        # GUI launching before the Daemon has ever answered a single
        # GetConfig().
        last_known = {"config": PLACEHOLDER_CONFIG, "profile": "Default", "layer": "base"}

        def rebuild():
            clear_children(content_box)
            if status["daemon_running"]:
                try:
                    config = self._client.get_config()
                    state = self._client.get_state()
                    profile = state["profile"]
                    layer = state["layer"]
                    device_connected = state["device_connected"]
                    capture_mode = state["capture_mode"]
                except (DaemonError, GLib.Error):
                    # A rare race: the Daemon vanished between
                    # subscribe_daemon_running_changed reporting it up and
                    # this call landing. Keep showing last_known under the
                    # overlay rather than crashing the GUI over it — the
                    # presence watch's own "vanished" callback will drive
                    # another rebuild() once it catches up.
                    pass
                else:
                    # Committed together, only once both calls succeeded —
                    # a get_state() failure right after a successful
                    # get_config() must not leave last_known["profile"]/
                    # ["layer"] stale against a newer config that may no
                    # longer even contain that profile (build_main_view
                    # would then KeyError looking it up).
                    last_known["config"] = config
                    last_known["profile"] = profile
                    last_known["layer"] = layer
                    status["device_connected"] = device_connected
                    status["capture_mode"] = capture_mode
            current_status = compute_status(status["daemon_running"], status["device_connected"])
            self._tray_icon.update(last_known["config"], last_known["profile"], current_status)
            content_box.append(
                build_status_wrapped_view(
                    self._client,
                    last_known["config"],
                    last_known["profile"],
                    last_known["layer"],
                    current_status,
                    rebuild,
                    ui_state,
                    status["capture_mode"],
                )
            )

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
        status = _wire_status_tracking(self._client, rebuild)
        _wire_focus_tracking(win, self._client)

        # Ticket 102: the `app.about` action the header-bar menu targets.
        # Opens a fresh modal About dialog reading one `GetState()` snapshot
        # (or `None` when the Daemon's down) for its version / device lines.
        about_action = Gio.SimpleAction.new("about", None)
        about_action.connect(
            "activate",
            lambda _a, _p: build_about_dialog(
                win,
                gui_version=__version__,
                state=_about_dialog_state(self._client, status["daemon_running"]),
            ).present(),
        )
        self.add_action(about_action)

        # `_wire_status_tracking`/`_wire_focus_tracking` above may have
        # already queued their initial state via `GLib.idle_add` (e.g. the
        # real `Gio.bus_watch_name`'s "already running" announce is often
        # this fast). Draining those now, before this first `rebuild()`,
        # means the very first paint reflects that real state instead of
        # `status`'s conservative `False`/`False` default flashing
        # "Daemon not running" for a frame on every launch — the same
        # pattern `gui/tests/test_app.py`'s `_pump_idle_callbacks` uses.
        context = GLib.MainContext.default()
        while context.iteration(False):
            pass

        rebuild()
        win.set_child(content_box)
        return win


_USAGE = (
    "Usage: acheron-gui [--version] [--help]\n"
    "\n"
    "Launches the Acheron GTK GUI, which talks to acheron-daemon over D-Bus.\n"
    "There are no options that change how it runs."
)


def main() -> None:
    # A tiny arg surface, deliberately handled before Gtk.Application so it
    # works with no display and without acquiring the bus name — this is what
    # `packaging/test_install.sh` runs as its launcher smoke check (ticket
    # 96), so a launcher that can't even start Python fails CI.
    arg = sys.argv[1] if len(sys.argv) > 1 else ""
    if arg in ("--version", "-V"):
        print(f"acheron-gui {__version__}")
        return
    if arg in ("--help", "-h"):
        print(_USAGE)
        return
    app = AcheronApplication()
    app.run([sys.argv[0]])


if __name__ == "__main__":
    main()
