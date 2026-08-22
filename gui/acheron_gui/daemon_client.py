"""The real `com.acheron.Daemon` D-Bus client: a synchronous `Gio.DBusProxy`
(per spec.md's GUI-side client choice — PyGObject's own proxy, already in
the GUI's dependency tree, not `dbus-python`/`dbus-fast`/`dbus-next`) that
speaks the surface ticket 15 built. `DaemonClient` is the structural
interface both this and `daemon_stub.DaemonStub` satisfy — the "swappable
fake backend" seam ticket 16 asks for — so the rest of the GUI never
branches on which one it's holding.

Every call here is atomic/immediately-applied (issue 08): there is no
draft/save step at the D-Bus layer, only in the GUI's own popover state
before "Save" is clicked.
"""

from __future__ import annotations

from typing import Callable, Protocol

import gi

gi.require_version("Gio", "2.0")
gi.require_version("GLib", "2.0")
from gi.repository import Gio, GLib

from . import wire

BUS_NAME = "com.acheron.Daemon"
OBJECT_PATH = "/com/acheron/Daemon"
INTERFACE = "com.acheron.Daemon"


class DaemonError(Exception):
    """Base for the named `com.acheron.Daemon.Error.*` set (issue 08) —
    callers catch this (or a specific subclass) rather than a bare
    `GLib.Error`, so D-Bus plumbing doesn't leak past this module."""


class NotFoundError(DaemonError):
    pass


class AlreadyExistsError(DaemonError):
    pass


class InvalidBindingError(DaemonError):
    pass


class DaemonIoError(DaemonError):
    pass


_ERROR_SUFFIXES = {
    "NotFound": NotFoundError,
    "AlreadyExists": AlreadyExistsError,
    "InvalidBinding": InvalidBindingError,
    "IoError": DaemonIoError,
}


def _translate_error(err: GLib.Error) -> Exception:
    name = Gio.DBusError.get_remote_error(err)
    if name is None:
        return err
    prefix = f"{INTERFACE}.Error."
    if name.startswith(prefix):
        exc_type = _ERROR_SUFFIXES.get(name[len(prefix) :], DaemonError)
        return exc_type(Gio.DBusError.strip_remote_error(err))
    return err


class DaemonClient(Protocol):
    """The structural interface the GUI codes against — satisfied by both
    `DBusDaemonClient` (real) and `daemon_stub.DaemonStub` (fake, for
    tests)."""

    def get_config(self) -> dict: ...

    def get_state(self) -> dict: ...

    def set_binding(self, input_str: str, layer: str, binding: dict) -> None: ...

    def clear_binding(self, input_str: str, layer: str) -> None: ...

    def set_mode_key_role(self, role: str) -> None: ...

    def create_profile(self, name: str) -> None: ...

    def delete_profile(self, name: str) -> None: ...

    def rename_profile(self, old_name: str, new_name: str) -> None: ...

    def create_macro(self, name: str, steps: list[dict]) -> str: ...

    def rename_macro(self, macro_id: str, new_name: str) -> None: ...

    def delete_macro(self, macro_id: str) -> None: ...

    def switch_profile(self, name: str) -> None: ...

    def set_output_suppressed(self, suppressed: bool) -> None: ...

    def stop_all_toggles(self) -> None: ...

    def set_actuation_point(self, input_str: str, actuation: int, release: int) -> None: ...

    def clear_actuation_point(self, input_str: str) -> None: ...

    def set_default_actuation(self, actuation: int, release: int) -> None: ...

    def reset_actuation_points(self) -> None: ...

    def set_force_digital(self, force: bool) -> None: ...

    def start_depth_stream(self, input_str: str, on_depth: Callable[[int], None]) -> None: ...

    def stop_depth_stream(self, input_str: str) -> None: ...

    def subscribe_layer_changed(self, callback: Callable[[str], None]) -> None: ...

    def subscribe_profile_changed(self, callback: Callable[[str], None]) -> None: ...

    def subscribe_daemon_running_changed(self, callback: Callable[[bool], None]) -> None: ...

    def subscribe_device_connection_changed(self, callback: Callable[[bool], None]) -> None: ...

    def subscribe_capture_mode_changed(self, callback: Callable[[str], None]) -> None: ...


class DBusDaemonClient:
    """Talks to the real Daemon process over the session bus. Every method
    is a blocking `call_sync` — acceptable here because every one of these
    calls is a local IPC round-trip the Daemon answers immediately (no
    draft/save step, no long-running work on the other end), so blocking
    the GTK main loop for it is not the same risk it would be for a network
    call."""

    def __init__(self, proxy: Gio.DBusProxy | None = None):
        self._proxy = proxy or Gio.DBusProxy.new_for_bus_sync(
            Gio.BusType.SESSION,
            Gio.DBusProxyFlags.NONE,
            None,
            BUS_NAME,
            OBJECT_PATH,
            INTERFACE,
            None,
        )
        # `start_depth_stream`'s local routing seam (ticket 26): at most one
        # editor is ever the live depth-stream target, mirroring the real
        # Daemon's own last-write-wins `StartDepthStream` semantics — see
        # that method's docstring for why this can't be a plain
        # `_connect_signal` call made fresh per popover.
        self._depth_callback: Callable[[int], None] | None = None
        self._depth_target: str | None = None
        self._depth_signal_connected = False

    def get_config(self) -> dict:
        (config,) = self._call("GetConfig", None)
        return config

    def get_state(self) -> dict:
        (state,) = self._call("GetState", None)
        return state

    def set_binding(self, input_str: str, layer: str, binding: dict) -> None:
        parameters = GLib.Variant(
            "(ssa{sv})", (input_str, layer, wire.binding_to_variant(binding))
        )
        self._call("SetBinding", parameters)

    def clear_binding(self, input_str: str, layer: str) -> None:
        self._call("ClearBinding", GLib.Variant("(ss)", (input_str, layer)))

    def set_mode_key_role(self, role: str) -> None:
        self._call("SetModeKeyRole", GLib.Variant("(s)", (role,)))

    def create_profile(self, name: str) -> None:
        self._call("CreateProfile", GLib.Variant("(s)", (name,)))

    def delete_profile(self, name: str) -> None:
        self._call("DeleteProfile", GLib.Variant("(s)", (name,)))

    def rename_profile(self, old_name: str, new_name: str) -> None:
        self._call("RenameProfile", GLib.Variant("(ss)", (old_name, new_name)))

    def create_macro(self, name: str, steps: list[dict]) -> str:
        variant_steps = [wire.macro_step_to_variant(step) for step in steps]
        parameters = GLib.Variant("(saa{sv})", (name, variant_steps))
        (macro_id,) = self._call("CreateMacro", parameters)
        return macro_id

    def rename_macro(self, macro_id: str, new_name: str) -> None:
        self._call("RenameMacro", GLib.Variant("(ss)", (macro_id, new_name)))

    def delete_macro(self, macro_id: str) -> None:
        self._call("DeleteMacro", GLib.Variant("(s)", (macro_id,)))

    def switch_profile(self, name: str) -> None:
        self._call("SwitchProfile", GLib.Variant("(s)", (name,)))

    def set_output_suppressed(self, suppressed: bool) -> None:
        self._call("SetOutputSuppressed", GLib.Variant("(b)", (suppressed,)))

    def stop_all_toggles(self) -> None:
        self._call("StopAllToggles", None)

    def set_actuation_point(self, input_str: str, actuation: int, release: int) -> None:
        self._call("SetActuationPoint", GLib.Variant("(syy)", (input_str, actuation, release)))

    def clear_actuation_point(self, input_str: str) -> None:
        self._call("ClearActuationPoint", GLib.Variant("(s)", (input_str,)))

    def set_default_actuation(self, actuation: int, release: int) -> None:
        self._call("SetDefaultActuation", GLib.Variant("(yy)", (actuation, release)))

    def reset_actuation_points(self) -> None:
        self._call("ResetActuationPoints", None)

    def set_force_digital(self, force: bool) -> None:
        self._call("SetForceDigital", GLib.Variant("(b)", (force,)))

    def start_depth_stream(self, input_str: str, on_depth: Callable[[int], None]) -> None:
        """Starts (or retargets) live depth streaming for `input_str`,
        routing `DepthChanged` values matching it to `on_depth`. The
        underlying `"g-signal"` listener is wired at most once, lazily, and
        kept forever — `build_binding_editor` is rebuilt eagerly for every
        Grid key on every app `rebuild()` (not lazily on popover open), so a
        fresh `_connect_signal` call per call here would leak one GObject
        signal connection per rebuild. Retargeting `_depth_callback`/
        `_depth_target` instead mirrors the real Daemon's own single-current-
        stream, last-write-wins shape (`StartDepthStream`'s docstring) — the
        client side only ever needs to route to whichever editor most
        recently asked, exactly like the server side only ever streams to
        whichever connection most recently asked."""
        self._depth_target = input_str
        self._depth_callback = on_depth
        if not self._depth_signal_connected:
            self._connect_signal("DepthChanged", self._on_depth_changed)
            self._depth_signal_connected = True
        self._call("StartDepthStream", GLib.Variant("(s)", (input_str,)))

    def stop_depth_stream(self, input_str: str) -> None:
        self._depth_target = None
        self._depth_callback = None
        self._call("StopDepthStream", GLib.Variant("(s)", (input_str,)))

    def _on_depth_changed(self, input_str: str, depth: int) -> None:
        if self._depth_callback is not None and input_str == self._depth_target:
            self._depth_callback(depth)

    def subscribe_layer_changed(self, callback: Callable[[str], None]) -> None:
        """Wires ticket 18's `ActiveLayerChanged` signal to `callback(layer)`
        — `Gio.DBusProxy` re-emits every D-Bus signal it receives as its own
        `"g-signal"` GObject signal, so this is a plain filtered connect
        rather than a second subscription mechanism."""
        self._connect_signal("ActiveLayerChanged", callback)

    def subscribe_profile_changed(self, callback: Callable[[str], None]) -> None:
        """Wires ticket 19's `ActiveProfileChanged` signal to
        `callback(name)`, same mechanism as `subscribe_layer_changed`."""
        self._connect_signal("ActiveProfileChanged", callback)

    def subscribe_daemon_running_changed(self, callback: Callable[[bool], None]) -> None:
        """Live `NameOwnerChanged` watch on `com.acheron.Daemon`'s own bus
        name (ticket 20) — not a one-shot check on window open, since the
        Daemon can crash while the window stays open. `Gio.bus_watch_name`
        is GLib's own idiom for this: `on_appeared`/`on_vanished` fire once
        with the name's current owned/unowned state as soon as it's known,
        then again on every subsequent transition, so this single watch
        covers both "was already running when the GUI launched" and "started
        or crashed later" with no separate one-shot check needed."""

        def on_appeared(_connection, _name, _owner):
            callback(True)

        def on_vanished(_connection, _name):
            callback(False)

        Gio.bus_watch_name(
            Gio.BusType.SESSION, BUS_NAME, Gio.BusNameWatcherFlags.NONE, on_appeared, on_vanished
        )

    def subscribe_device_connection_changed(self, callback: Callable[[bool], None]) -> None:
        """Wires ticket 20's `DeviceConnectionChanged` signal to
        `callback(connected)`, same mechanism as `subscribe_layer_changed`."""
        self._connect_signal("DeviceConnectionChanged", callback)

    def subscribe_capture_mode_changed(self, callback: Callable[[str], None]) -> None:
        """Wires ticket 17/23's `CaptureModeChanged` signal to
        `callback(mode)` — called once, at the app level (`app.py`'s
        `_wire_status_tracking`), same as `subscribe_device_connection_changed`,
        not per-editor: a mode transition drives a full `rebuild()` rather
        than a targeted in-place update, so the Actuation & release section's
        badge only needs its *initial* value threaded in as a plain
        parameter, not a live subscription of its own (ticket 26 — unlike
        depth, which changes far too often for a rebuild-per-tick to be
        usable)."""
        self._connect_signal("CaptureModeChanged", callback)

    def _connect_signal(self, wanted_signal_name: str, callback: Callable) -> None:
        def on_g_signal(_proxy, _sender_name, signal_name, parameters):
            if signal_name == wanted_signal_name:
                callback(*parameters.unpack())

        self._proxy.connect("g-signal", on_g_signal)

    def _call(self, method: str, parameters: GLib.Variant | None) -> tuple:
        try:
            result = self._proxy.call_sync(method, parameters, Gio.DBusCallFlags.NONE, -1, None)
        except GLib.Error as err:
            raise _translate_error(err) from err
        return result.unpack()
