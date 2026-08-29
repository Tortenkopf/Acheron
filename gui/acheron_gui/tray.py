"""The real system tray icon (ticket 36), replacing `device_overview.py`'s
old in-window `build_tray_mock` placeholder: a hand-rolled, in-process
`org.kde.StatusNotifierItem` service via `dasbus`, backed by a minimal
`com.canonical.dbusmenu` object for the popup menu — not `AppIndicator3`/
`AyatanaAppIndicator3` (ticket 11's Reopened/Resolution: that library's
typelib hard-depends on GTK 3.0, which cannot load in the same process as
this GTK4 GUI).

No `from __future__ import annotations` here (unlike every other module in
this package) — dasbus's `@dbus_interface` XML generator reads the *real*
type-hint objects off `@property`/method signatures at class-definition
time (`Str`, `Bool`, ...); the `from __future__` form stringifies every
annotation instead, which breaks that generator outright (confirmed
directly: it raises `TypeError: Invalid DBus type 'Str'` the moment the
class is defined).

**State source**: `TrayIcon` runs no D-Bus subscriptions of its own — ticket
36's design hooks it into `app.py`'s existing `status`/`rebuild()` instead,
via one `update(config, profile, status)` call made alongside the main
window's own rebuild. One source of truth, no duplicate signal wiring
(mirrors `app.py`'s own `_wire_status_tracking` docstring reasoning).

The menu tree itself (`tray_menu.MenuModel`) is plain Python, decoupled from
the `GLib.Variant`/dasbus marshaling this module does at the D-Bus boundary
— the same split `wire.py` draws between "what the wire needs" and "what
the GUI edits" (`tray_menu.py`'s own module docstring has the rest of that
story, including why item ids are freely reassigned on every rebuild).
"""

import os
import sys
from typing import Callable, Dict, List, Tuple

from dasbus.connection import SessionMessageBus
from dasbus.server.interface import dbus_interface, dbus_signal, returns_multiple_arguments
from dasbus.typing import Bool, Int32, ObjPath, Str, Structure, UInt32, Variant, get_variant

import gi

gi.require_version("GLib", "2.0")
from gi.repository import GLib

from .daemon_client import DaemonClient, DaemonError
from .device_overview import STATUS_STATES
from .systemd_client import SystemdClient
from .tray_menu import ROOT_ID, MenuModel, build_menu_items

ITEM_OBJECT_PATH = "/StatusNotifierItem"
MENU_OBJECT_PATH = "/MenuBar"
WATCHER_BUS_NAME = "org.kde.StatusNotifierWatcher"
WATCHER_OBJECT_PATH = "/StatusNotifierWatcher"

# The three status-dot SVGs the SNI host renders in the panel. They ship in
# this package's own `icons/` dir as read-only *source* only — at runtime
# `TrayIcon` syncs them out to a stable per-user data dir
# (`_resolve_icon_theme_path`) and the host is only ever pointed there.
#
# Why not just point `IconThemePath` at this package's `icons/` dir: when
# the GUI runs from a git checkout (`python3 gui/main.py`, the normal dev
# path) that dir *is* the working tree, and the GNOME Shell panel keeps a
# live file-watch on `IconThemePath` — overwriting an SVG in place there
# while the GUI runs hard-crashed the whole session (ticket 97). The
# installed launch path (`~/.local/lib/acheron/`) wouldn't hit that, but
# resolving to one stable location regardless keeps the two paths identical.
BUNDLED_ICON_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "icons")

# The direct equivalent of `AppIndicator3.set_icon_theme_path` — one flat
# directory of `<name>.svg` files, live-verified against the real GNOME
# Shell panel as the layout that actually resolves. Ticket 36's own build
# note assumed the standard freedesktop nested layout instead
# (`icons/scalable/apps/<name>.svg` straight under `IconThemePath`, the
# shape a real installed icon *theme* uses) — that assumption doesn't hold
# here because the real consumer isn't `Gtk.IconTheme` at all: GNOME
# Shell's `ubuntu-appindicators` extension looks icons up through its own
# `St.IconTheme`, constructed with `set_search_path([IconThemePath])`
# (`appIndicator.js`'s `_createIconTheme`/`_getIconData`) and no theme-name
# or size/context subdirectory expectation. Confirmed the hard way: a
# headless `Gtk.IconTheme.has_icon()` probe said a `hicolor/scalable/apps/`
# nested layout should resolve, but the real panel still rendered the
# generic "icon not found" fallback for it — only dropping the SVGs flat
# directly into this one directory made the real green/orange/red circle
# actually render live. A later commissioned icon replaces these same flat
# `<name>.svg` files, not a freedesktop-theme-shaped path.

# Mirrors `STATUS_STATES`' three reachable states exactly — placeholder
# filled-circle SVGs at its own hex values (ticket 11's resolution).
ICON_NAMES = {
    "running_connected": "acheron-running-connected",
    "running_disconnected": "acheron-running-disconnected",
    "not_running": "acheron-not-running",
}


def _resolve_icon_theme_path() -> str:
    """The flat directory the SNI host reads the status-dot SVGs from —
    always a stable per-user data dir, NEVER this package's own `icons/`
    (see the note above and ticket 97). Honors `$ACHERON_TRAY_ICON_DIR`
    (mirrors the launcher's `$ACHERON_GUI_LIB`), then `$XDG_DATA_HOME`,
    then the `~/.local/share` default. `install.sh` populates the same
    default path up front."""
    override = os.environ.get("ACHERON_TRAY_ICON_DIR")
    if override:
        return override
    data_home = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    return os.path.join(data_home, "acheron", "tray-icons")


def _sync_bundled_icons(dest_dir: str) -> None:
    """Copies the bundled status-dot SVGs into `dest_dir`, but only the ones
    that are missing or whose bytes differ from the bundled copy — a
    steady-state launch writes nothing. Each write is a temp file +
    `os.replace`, so a host watching the directory never sees a partial
    file and never the truncate-in-place that crashed the session
    (ticket 97). Best-effort: a failure here is logged, not raised — the
    host just falls back to its generic "no icon" rendering, exactly as it
    would have before this sync existed."""
    try:
        os.makedirs(dest_dir, exist_ok=True)
        for name in ICON_NAMES.values():
            filename = f"{name}.svg"
            src = os.path.join(BUNDLED_ICON_DIR, filename)
            if not os.path.isfile(src):
                continue
            with open(src, "rb") as handle:
                wanted = handle.read()
            dest = os.path.join(dest_dir, filename)
            try:
                with open(dest, "rb") as handle:
                    if handle.read() == wanted:
                        continue
            except FileNotFoundError:
                pass
            tmp = f"{dest}.{os.getpid()}.tmp"
            with open(tmp, "wb") as handle:
                handle.write(wanted)
            os.replace(tmp, dest)
    except OSError as err:
        print(f"acheron-gui: could not sync tray icons to {dest_dir}: {err}", file=sys.stderr)

_Layout = Tuple[Int32, Structure, List[Variant]]


def _property_variant(value) -> Variant:
    if isinstance(value, bool):
        return get_variant(Bool, value)
    if isinstance(value, str):
        return get_variant(Str, value)
    raise TypeError(f"unsupported dbusmenu property value: {value!r}")


def _properties_variant(properties: dict) -> Dict[str, Variant]:
    return {name: _property_variant(value) for name, value in properties.items()}


def _layout_variant(layout: tuple) -> tuple:
    """Recursively converts one native `(id, properties, children)` layout
    node (`tray_menu.MenuModel.get_layout`'s own return shape) into the
    nested structure `(ia{sv}av)` needs: each entry in `children` must
    already be a fully-built `GLib.Variant` (the array's element type is a
    generic `v`, so `get_variant` can't infer each child's own inner
    `(ia{sv}av)` struct type the way it can for a single, fully-typed
    top-level call) — the node itself stays a plain tuple, since whatever
    wraps it (a sibling `children` list, or `GetLayout`'s own typed return)
    provides that outer wrapping instead."""
    item_id, properties, children = layout
    return (
        item_id,
        _properties_variant(properties),
        [GLib.Variant("(ia{sv}av)", _layout_variant(child)) for child in children],
    )


@dbus_interface("com.canonical.dbusmenu")
class _DBusMenuService:
    """The `Menu` object `StatusNotifierItem.Menu` points at — a minimal
    static-tree `com.canonical.dbusmenu` implementation (ticket 36):
    `GetLayout`/`GetGroupProperties`/`GetProperty`/`Event`/`AboutToShow`
    plus the `LayoutUpdated`/`ItemsPropertiesUpdated` signals. All the
    actual tree logic lives in `tray_menu.MenuModel`; this class only does
    the `GLib.Variant` marshaling at the D-Bus boundary."""

    def __init__(self, model: MenuModel):
        self._model = model

    @property
    def Version(self) -> UInt32:
        return 3

    @property
    def TextDirection(self) -> Str:
        return "ltr"

    @property
    def Status(self) -> Str:
        return "normal"

    @property
    def IconThemePath(self) -> List[Str]:
        return []

    @returns_multiple_arguments
    def GetLayout(
        self, parentId: Int32, recursionDepth: Int32, propertyNames: List[Str]
    ) -> Tuple[UInt32, _Layout]:
        revision, layout = self._model.get_layout(parentId, recursionDepth, list(propertyNames))
        return revision, _layout_variant(layout)

    def GetGroupProperties(
        self, ids: List[Int32], propertyNames: List[Str]
    ) -> List[Tuple[Int32, Structure]]:
        pairs = self._model.get_group_properties(list(ids), list(propertyNames))
        return [(item_id, _properties_variant(properties)) for item_id, properties in pairs]

    def GetProperty(self, id: Int32, name: Str) -> Variant:  # noqa: A002 - matches the spec's arg name
        return _property_variant(self._model.get_property(id, name))

    def Event(self, id: Int32, eventId: Str, data: Variant, timestamp: UInt32) -> None:  # noqa: A002
        self._model.event(id, eventId, data, timestamp)

    def AboutToShow(self, id: Int32) -> Bool:  # noqa: A002
        return self._model.about_to_show(id)

    @dbus_signal
    def LayoutUpdated(_self, revision: UInt32, parent: Int32):
        pass

    @dbus_signal
    def ItemsPropertiesUpdated(
        _self,
        updatedProps: List[Tuple[Int32, Structure]],
        removedProps: List[Tuple[Int32, List[Str]]],
    ):
        # Ticket 36: never actually emitted — every relevant change does a
        # full rebuild + LayoutUpdated instead (see tray_menu.py's module
        # docstring). Declared anyway for a spec-correct introspection.
        pass


@dbus_interface("org.kde.StatusNotifierItem")
class _StatusNotifierItemService:
    """The tray item itself. `IconName` is the only property that actually
    changes at runtime (`TrayIcon.update` mutates the model this reads
    from); `Category`/`Id`/`Title`/`Status`/`ItemIsMenu` are fixed —
    `Status` is always `"Active"`, matching ticket 11's decision not to have
    a `NeedsAttention`/blinking variant. `Activate`/`SecondaryActivate`/
    `ContextMenu`/`Scroll` are all real, exported no-ops: ticket 11 found
    the SNI protocol draws no click-vs-menu distinction worth designing
    (`ItemIsMenu = True` tells a well-behaved host to just open `Menu` on
    primary click instead of calling `Activate`), so nothing beyond a valid
    RPC endpoint is needed here."""

    def __init__(self, tray_icon: "TrayIcon"):
        self._tray_icon = tray_icon

    @property
    def Category(self) -> Str:
        return "ApplicationStatus"

    @property
    def Id(self) -> Str:
        return "acheron"

    @property
    def Title(self) -> Str:
        return "Acheron"

    @property
    def Status(self) -> Str:
        return "Active"

    @property
    def IconName(self) -> Str:
        return self._tray_icon.icon_name

    @property
    def IconThemePath(self) -> Str:
        return self._tray_icon.icon_theme_path

    @property
    def Menu(self) -> ObjPath:
        return ObjPath(MENU_OBJECT_PATH)

    @property
    def ItemIsMenu(self) -> Bool:
        return True

    def ContextMenu(self, x: Int32, y: Int32) -> None:
        pass

    def Activate(self, x: Int32, y: Int32) -> None:
        pass

    def SecondaryActivate(self, x: Int32, y: Int32) -> None:
        pass

    def Scroll(self, delta: Int32, orientation: Str) -> None:
        pass

    NewIcon = dbus_signal()
    NewTitle = dbus_signal()

    @dbus_signal
    def NewStatus(_self, status: Str):
        pass


class TrayIcon:
    """Owns the tray's `StatusNotifierItem`/`DBusMenu` D-Bus objects and the
    plain-Python state both are computed from. Constructed once at GUI
    launch (`app.py`'s `do_activate`); `update(config, profile, status)` is
    called alongside `app.py`'s own `rebuild()` — see module docstring.

    `bus` is injectable (mirrors `DaemonClient`/`SystemdClient`'s own
    optional-proxy constructors) so tests can supply a fake double instead
    of registering a throwaway icon on the real desktop's tray every test
    run.
    """

    def __init__(
        self,
        client: DaemonClient,
        systemd_client: SystemdClient,
        on_show_window: Callable[[], None],
        on_quit: Callable[[], None],
        bus=None,
    ):
        self._client = client
        self._systemd_client = systemd_client
        self._on_show_window = on_show_window
        self._on_quit = on_quit
        self._status = "not_running"
        self._menu_model = MenuModel()

        # Ticket 97: the SNI host reads the status-dot SVGs from a stable
        # per-user data dir, never this package's `icons/` (which may be a
        # live git checkout). Sync the bundled copies out on every launch,
        # cheaply (a no-op once they're current).
        self._icon_theme_path = _resolve_icon_theme_path()
        _sync_bundled_icons(self._icon_theme_path)

        self._item_service = _StatusNotifierItemService(self)
        self._menu_service = _DBusMenuService(self._menu_model)

        self._bus = bus or SessionMessageBus()
        self._bus.publish_object(ITEM_OBJECT_PATH, self._item_service)
        self._bus.publish_object(MENU_OBJECT_PATH, self._menu_service)

        watcher = self._bus.get_proxy(WATCHER_BUS_NAME, WATCHER_OBJECT_PATH)
        watcher.RegisterStatusNotifierItem(ITEM_OBJECT_PATH)

    @property
    def icon_name(self) -> str:
        return ICON_NAMES[self._status]

    @property
    def icon_theme_path(self) -> str:
        return self._icon_theme_path

    def update(self, config: dict, profile: str, status: str) -> None:
        """Rebuilds the menu tree from scratch and bumps its revision
        (ticket 36's full-rebuild convention — see `tray_menu.py`), then
        pushes the new icon/title/status to the host. `profiles` comes from
        `config` as-is, including while it's `app.py`'s own stale
        `last_known`/`PLACEHOLDER_CONFIG` during a Daemon outage — same
        "keep showing the last-known data" tradeoff `app.py`'s `rebuild()`
        already accepts for the main window."""
        self._status = status
        daemon_running = status != "not_running"
        profiles = list(config["profiles"])

        def toggle_daemon(daemon_running: bool = daemon_running) -> None:
            try:
                if daemon_running:
                    self._systemd_client.stop_daemon()
                else:
                    self._systemd_client.start_daemon()
            except GLib.Error as err:
                print(f"acheron-gui: tray Pause/Resume Daemon failed: {err}", file=sys.stderr)

        items = build_menu_items(
            status,
            profile,
            profiles,
            daemon_running,
            self._on_show_window,
            self._switch_profile,
            toggle_daemon,
            self._on_quit,
        )
        self._menu_model.rebuild(items)
        self._menu_service.LayoutUpdated(self._menu_model.revision, ROOT_ID)

        self._item_service.NewIcon()
        self._item_service.NewTitle()
        self._item_service.NewStatus("Active")

    def _switch_profile(self, name: str) -> None:
        try:
            self._client.switch_profile(name)
        except DaemonError as err:
            # No error-surfacing UI exists in a native tray menu (unlike
            # the sidebar/old tray mock's inline error_label) — best-effort,
            # same "must not crash the caller" bar as `_ensure_daemon_started_
            # on_launch`.
            print(f"acheron-gui: tray Switch Profile failed: {err}", file=sys.stderr)
