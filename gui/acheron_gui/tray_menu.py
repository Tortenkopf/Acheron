# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Pure-Python model of the tray's `com.canonical.dbusmenu` item tree — no
GLib/dasbus involved, so it's unit-testable directly (`tray.py`'s
dasbus-exported service wraps this for the real D-Bus marshaling, the same
split `wire.py` draws between "what the wire needs" and "what the GUI edits").

Ticket 36: on any relevant change (Profile created/renamed/deleted, Daemon
pause/resume, status transition) the whole item tree is rebuilt from scratch
and `MenuModel.revision` bumped — mirrors this codebase's existing
full-rebuild convention (`app.py`'s own `rebuild()`).

Ticket 98: the rebuild + `LayoutUpdated` is not enough on its own. GNOME's
`ubuntu-appindicators` host, after a `LayoutUpdated`, re-reads only `type`
and `children-display` from `GetLayout` and pulls a full property set for
*new* item ids only — an already-known item's changed `label`/`enabled`
(the Pause↔Resume flip, the active-Profile greying) reaches it through an
`ItemsPropertiesUpdated` signal or not at all. So `rebuild()` also diffs
the previous tree against the new one and hands `tray.py` the per-item
property delta to emit. For that diff to line up across rebuilds, item ids
are assigned by *role*, not allocation order: the five fixed rows (status,
Show Window, Switch Profile, Pause/Resume, Quit) keep ids 1-5 for the life
of the process and the Profile entries take 6, 7, 8, … in order — so a
Profile switch or a Daemon pause leaves every id exactly where the host
last saw it, and only a genuine Profile add/remove shifts anything (which
`LayoutUpdated`'s revision bump already covers).
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

# Role-assigned item ids (ticket 98): the five fixed rows keep these ids for
# the whole process so the SNI host's id-keyed property bookkeeping survives
# a rebuild; Profile entries take `FIRST_PROFILE_ID`, +1, +2, … in order.
STATUS_ID = 1
SHOW_WINDOW_ID = 2
SWITCH_PROFILE_ID = 3
PAUSE_RESUME_ID = 4
QUIT_ID = 5
FIRST_PROFILE_ID = 6


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

    Item ids are role-assigned, not allocation-ordered (ticket 98 — see the
    module docstring): fixed rows hold `STATUS_ID`..`QUIT_ID`, Profile
    entries `FIRST_PROFILE_ID` upward in `profiles` order.
    """
    items: dict[int, MenuItem] = {}

    status_label, _colour, _glyph = STATUS_STATES[status]
    items[STATUS_ID] = MenuItem(STATUS_ID, {"label": status_label, "enabled": False})
    items[SHOW_WINDOW_ID] = MenuItem(
        SHOW_WINDOW_ID, {"label": "Show Window"}, on_activate=on_show_window
    )

    profile_ids: list[int] = []
    for offset, name in enumerate(profiles):
        profile_id = FIRST_PROFILE_ID + offset
        profile_ids.append(profile_id)
        items[profile_id] = MenuItem(
            profile_id,
            {"label": name, "enabled": name != profile},
            on_activate=lambda name=name: on_switch_profile(name),
        )

    items[SWITCH_PROFILE_ID] = MenuItem(
        SWITCH_PROFILE_ID,
        {"label": "Switch Profile", "children-display": "submenu"},
        children=profile_ids,
    )
    items[PAUSE_RESUME_ID] = MenuItem(
        PAUSE_RESUME_ID,
        {"label": "Pause Daemon" if daemon_running else "Resume Daemon"},
        on_activate=on_toggle_daemon,
    )
    items[QUIT_ID] = MenuItem(QUIT_ID, {"label": "Quit"}, on_activate=on_quit)

    items[ROOT_ID] = MenuItem(
        ROOT_ID,
        {},
        [STATUS_ID, SHOW_WINDOW_ID, SWITCH_PROFILE_ID, PAUSE_RESUME_ID, QUIT_ID],
    )
    return items


class MenuModel:
    """Holds the current item tree plus a monotonic revision counter
    (`LayoutUpdated`'s own argument) — bumped on every `rebuild()`. Starts
    as an empty root with no children, since the real tree only exists once
    `TrayIcon`'s first `update()` call rebuilds it (mirrors `app.py`'s own
    `PLACEHOLDER_CONFIG` gap-before-first-fetch pattern)."""

    def __init__(self) -> None:
        self.revision = 0
        self.items: dict[int, MenuItem] = {ROOT_ID: MenuItem(ROOT_ID, {}, [])}

    def rebuild(
        self, items: dict[int, MenuItem]
    ) -> tuple[list[tuple[int, dict]], list[tuple[int, list[str]]]]:
        """Swaps in a fresh item tree and bumps `revision`. Returns the
        `ItemsPropertiesUpdated` delta `(changed, removed)` for `tray.py`
        to emit (ticket 98 — see module docstring): every item id present
        in *both* the previous and new tree whose properties differ, as
        `(id, new_properties)` in `changed` plus `(id, [dropped_names])` in
        `removed`. A genuinely new id is left out — `LayoutUpdated` makes
        the host fetch that one's properties itself."""
        previous = self.items
        self.items = items
        self.revision += 1

        changed: list[tuple[int, dict]] = []
        removed: list[tuple[int, list[str]]] = []
        for item_id, item in items.items():
            was = previous.get(item_id)
            if was is None or was.properties == item.properties:
                continue
            changed.append((item_id, dict(item.properties)))
            dropped = [name for name in was.properties if name not in item.properties]
            if dropped:
                removed.append((item_id, dropped))
        return changed, removed

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
