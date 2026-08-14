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

from typing import Protocol

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

    def get_state(self) -> tuple[str, str, list[str], bool]: ...

    def set_binding(self, input_str: str, binding: dict) -> None: ...

    def clear_binding(self, input_str: str) -> None: ...


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

    def get_config(self) -> dict:
        (config,) = self._call("GetConfig", None)
        return config

    def get_state(self) -> tuple[str, str, list[str], bool]:
        return self._call("GetState", None)

    def set_binding(self, input_str: str, binding: dict) -> None:
        parameters = GLib.Variant("(sa{sv})", (input_str, wire.binding_to_variant(binding)))
        self._call("SetBinding", parameters)

    def clear_binding(self, input_str: str) -> None:
        self._call("ClearBinding", GLib.Variant("(s)", (input_str,)))

    def _call(self, method: str, parameters: GLib.Variant | None) -> tuple:
        try:
            result = self._proxy.call_sync(method, parameters, Gio.DBusCallFlags.NONE, -1, None)
        except GLib.Error as err:
            raise _translate_error(err) from err
        return result.unpack()
