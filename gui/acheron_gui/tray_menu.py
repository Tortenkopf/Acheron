"""Pure-Python model of the tray's `com.canonical.dbusmenu` item tree — no
GLib/dasbus involved, so it's unit-testable directly (`tray.py`'s
dasbus-exported service wraps this for the real D-Bus marshaling, the same
split `wire.py` draws between "what the wire needs" and "what the GUI edits").

Ticket 36: on any relevant change (Profile created/renamed/deleted, Daemon
pause/resume, status transition) the whole item tree is rebuilt from scratch
and `MenuModel.revision` bumped — mirrors this codebase's existing
full-rebuild convention (`app.py`'s own `rebuild()`) rather than incremental
per-item `ItemsPropertiesUpdated` patches. Item ids are therefore only
stable for the lifetime of one tree — freely reassigned on every rebuild —
since `LayoutUpdated`'s revision bump is what tells the host to re-fetch the
whole thing rather than trust its old ids.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Callable

from .device_overview import STATUS_STATES

ROOT_ID = 0

# `com.canonical.dbusmenu`'s own defaults for an omitted property (per the
# GNOME extension's `dbusMenu.js` DEFAULT_VALUES) — `get_property`/
# `get_group_properties` fall back to these rather than requiring every
# item to spell them out.
_DEFAULT_PROPERTIES = {"visible": True, "enabled": True, "label": ""}


@dataclass
class MenuItem:
    id: int
    properties: dict
    children: list[int] = field(default_factory=list)
    on_activate: Callable[[], None] | None = None


def build_menu_items(
    status: str,
    profile: str,
    profiles: list[str],
    daemon_running: bool,
    on_show_window: Callable[[], None],
    on_switch_profile: Callable[[str], None],
    on_toggle_daemon: Callable[[], None],
    on_quit: Callable[[], None],
) -> dict[int, MenuItem]:
    """Builds the static item tree, top to bottom per ticket 36: status
    line -> Show Window -> Switch Profile (submenu of `profiles`) ->
    Pause/Resume Daemon (label flips with `daemon_running`) -> Quit.

    The active `profile`'s own submenu entry is disabled — the same
    "no-op on the Profile already active" convention `build_profile_sidebar`'s
    delete button and the old `build_tray_mock`'s quick-switch already used,
    rather than sending a redundant `SwitchProfile` that would force-stop
    every running Toggle for no actual switch (ticket 19).
    """
    items: dict[int, MenuItem] = {}
    next_id = [1]

    def add(properties: dict, children: list[int] | None = None, on_activate=None) -> int:
        item_id = next_id[0]
        next_id[0] += 1
        items[item_id] = MenuItem(item_id, properties, children or [], on_activate)
        return item_id

    status_label, _colour, _glyph = STATUS_STATES[status]
    status_id = add({"label": status_label, "enabled": False})
    show_window_id = add({"label": "Show Window"}, on_activate=on_show_window)

    profile_ids = [
        add(
            {"label": name, "enabled": name != profile},
            on_activate=lambda name=name: on_switch_profile(name),
        )
        for name in profiles
    ]
    switch_profile_id = add(
        {"label": "Switch Profile", "children-display": "submenu"}, children=profile_ids
    )

    pause_resume_id = add(
        {"label": "Pause Daemon" if daemon_running else "Resume Daemon"},
        on_activate=on_toggle_daemon,
    )
    quit_id = add({"label": "Quit"}, on_activate=on_quit)

    items[ROOT_ID] = MenuItem(
        ROOT_ID,
        {},
        [status_id, show_window_id, switch_profile_id, pause_resume_id, quit_id],
    )
    return items


class MenuModel:
    """Holds the current item tree plus a monotonic revision counter
    (`LayoutUpdated`'s own argument) — bumped on every `rebuild()`, never
    patched incrementally (see module docstring). Starts as an empty root
    with no children, since the real tree only exists once `TrayIcon`'s
    first `update()` call rebuilds it (mirrors `app.py`'s own
    `PLACEHOLDER_CONFIG` gap-before-first-fetch pattern)."""

    def __init__(self) -> None:
        self.revision = 0
        self.items: dict[int, MenuItem] = {ROOT_ID: MenuItem(ROOT_ID, {}, [])}

    def rebuild(self, items: dict[int, MenuItem]) -> None:
        self.items = items
        self.revision += 1

    def get_layout(self, parent_id: int, recursion_depth: int, property_names: list[str]):
        """Returns `(revision, layout)`, `layout` a plain native
        `(id, properties, children)` nested-tuple tree — no `GLib.Variant`
        here, `tray.py`'s dasbus wrapper does that marshaling, matching
        `wire.py`'s own split between plain-Python shapes and wire encoding.

        `recursion_depth < 0` means unlimited (the spec's convention, and
        what the real host's own `GetLayout(0, -1, ...)` call passes); `0`
        means "just this item, no children". An empty `property_names`
        means "every property" (also spec convention), matching the real
        host's initial `GetLayout(0, -1, ['type', 'children-display'])`
        scan behavior when it does ask for a subset.
        """

        def build(item_id: int, depth: int):
            item = self.items[item_id]
            properties = (
                dict(item.properties)
                if not property_names
                else {k: v for k, v in item.properties.items() if k in property_names}
            )
            children = (
                [build(child_id, depth - 1) for child_id in item.children] if depth != 0 else []
            )
            return (item_id, properties, children)

        return self.revision, build(parent_id, recursion_depth)

    def get_group_properties(self, ids: list[int], property_names: list[str]):
        result = []
        for item_id in ids:
            item = self.items.get(item_id)
            if item is None:
                continue
            properties = (
                dict(item.properties)
                if not property_names
                else {k: v for k, v in item.properties.items() if k in property_names}
            )
            result.append((item_id, properties))
        return result

    def get_property(self, item_id: int, name: str):
        return self.items[item_id].properties.get(name, _DEFAULT_PROPERTIES.get(name))

    def event(self, item_id: int, event_id: str, _data, _timestamp: int) -> None:
        if event_id != "clicked":
            return
        item = self.items.get(item_id)
        if item is not None and item.on_activate is not None:
            item.on_activate()

    def about_to_show(self, _item_id: int) -> bool:
        # Ticket 36: the tree is static/eagerly rebuilt on every relevant
        # change (see module docstring), never lazily populated on
        # submenu-open — so there's never anything new to report here.
        return False
