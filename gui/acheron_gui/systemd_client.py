"""GUI-side client for systemd's own D-Bus API (`org.freedesktop.systemd1`),
used only for ticket 21's launch-time safety net: on its own launch, the GUI
asks systemd to clear any latched `failed` state on the Daemon's unit and
(re)start it, over the same session D-Bus this process already talks to
`com.acheron.Daemon` over — no `systemctl` shell-out, no subprocess.
`StartUnit` is a no-op if the unit's already running (systemd's own
contract), so no status check is needed first; `ResetFailedUnit` is what
lets a prior crash — which latches the unit in `failed` per install.sh's
`Restart=on-failure`/`StartLimitBurst` guard — actually restart instead of
being refused by systemd's own start-limit.

Live-hardware testing (deliberately tripping `StartLimitBurst`, then
relaunching the GUI) caught the ticket/spec's own `Manager.ResetFailed(unit)`
call as wrong: `Manager.ResetFailed()` takes *no* arguments and clears every
failed unit on the bus, not one — calling it with a unit-name string is a
`GLib.Error` (`InvalidArgs`), silently swallowed by
`_ensure_daemon_started_on_launch`'s best-effort handling, which is exactly
how this went unnoticed until a real `failed`-state demo. The correct
per-unit call, confirmed live via `busctl --user introspect` against the
running `org.freedesktop.systemd1.Manager`, is `ResetFailedUnit(s
unit_name)`.

Structural `SystemdClient` Protocol mirrors `daemon_client.DaemonClient`'s
shape — `DBusSystemdClient` (real) vs a plain fake for tests — so `app.py`'s
launch-time wiring never branches on which one it's holding.

Ticket 36: `stop_daemon`/`start_daemon` back the tray menu's Pause/Resume
Daemon item — session-only (`systemctl --user stop`/`start`, not `disable`/
`enable`), so the unit stays login-enabled either way and a paused Daemon
still comes back automatically at the next login (ticket 11's resolution).
Plain `StopUnit`/`StartUnit`, unlike `ensure_daemon_started`'s `ResetFailedUnit`
dance: there's no latched `failed` state to clear here, since Pause always
starts from a cleanly-running unit.
"""

from __future__ import annotations

from typing import Protocol

import gi

gi.require_version("Gio", "2.0")
gi.require_version("GLib", "2.0")
from gi.repository import Gio, GLib

SYSTEMD_BUS_NAME = "org.freedesktop.systemd1"
SYSTEMD_OBJECT_PATH = "/org/freedesktop/systemd1"
SYSTEMD_MANAGER_INTERFACE = "org.freedesktop.systemd1.Manager"
DAEMON_UNIT = "acheron-daemon.service"


class SystemdClient(Protocol):
    def ensure_daemon_started(self) -> None: ...

    def stop_daemon(self) -> None: ...

    def start_daemon(self) -> None: ...


class DBusSystemdClient:
    """Talks to the real `systemd --user` manager over the session bus via
    `Gio.DBusProxy` — the same client choice spec.md makes for the Daemon's
    own D-Bus surface.

    The proxy is built lazily, on the first `ensure_daemon_started()` call,
    rather than in `__init__` — this client is constructed unconditionally
    in `AcheronApplication.__init__` (ticket 21), well before `do_activate`'s
    try/except gets a chance to run, so a proxy-construction failure (e.g.
    the session bus itself being unreachable) must surface from the same
    call `_ensure_daemon_started_on_launch` already guards, not from
    construction, or it would crash the whole GUI on launch — exactly the
    "must never block the GUI from opening" case this client exists to
    avoid."""

    def __init__(self, proxy: Gio.DBusProxy | None = None):
        self._proxy = proxy

    def ensure_daemon_started(self) -> None:
        proxy = self._proxy or self._connect()
        self._proxy = proxy
        proxy.call_sync(
            "ResetFailedUnit",
            GLib.Variant("(s)", (DAEMON_UNIT,)),
            Gio.DBusCallFlags.NONE,
            -1,
            None,
        )
        proxy.call_sync(
            "StartUnit",
            GLib.Variant("(ss)", (DAEMON_UNIT, "replace")),
            Gio.DBusCallFlags.NONE,
            -1,
            None,
        )

    def stop_daemon(self) -> None:
        proxy = self._proxy or self._connect()
        self._proxy = proxy
        proxy.call_sync(
            "StopUnit",
            GLib.Variant("(ss)", (DAEMON_UNIT, "replace")),
            Gio.DBusCallFlags.NONE,
            -1,
            None,
        )

    def start_daemon(self) -> None:
        proxy = self._proxy or self._connect()
        self._proxy = proxy
        proxy.call_sync(
            "StartUnit",
            GLib.Variant("(ss)", (DAEMON_UNIT, "replace")),
            Gio.DBusCallFlags.NONE,
            -1,
            None,
        )

    def _connect(self) -> Gio.DBusProxy:
        return Gio.DBusProxy.new_for_bus_sync(
            Gio.BusType.SESSION,
            Gio.DBusProxyFlags.NONE,
            None,
            SYSTEMD_BUS_NAME,
            SYSTEMD_OBJECT_PATH,
            SYSTEMD_MANAGER_INTERFACE,
            None,
        )
