Type: task
Status: resolved

## Question

Two tray-indicator menu bugs reported by the user (tray built in ticket 36):

1. The currently-active Profile is supposed to be greyed out in the Switch Profile
   submenu. In fact it is always whichever Profile was active when the GUI started —
   the greying never moves on a Profile switch (from the tray *or* the main window).
2. The Pause/Resume Daemon row should read "Resume Daemon" once the Daemon is paused.
   It always reads "Pause Daemon" regardless of the Daemon's actual state.

Both are the same underlying defect: after the first menu population, no property
change on an already-shown item ever reaches the panel.

### Root cause

`tray_menu.MenuModel` and `tray.TrayIcon.update()` were correct in isolation —
`build_menu_items` produces the right labels/`enabled` flags and every `update()`
does a full rebuild + `LayoutUpdated` with a bumped revision (ticket 36's
convention). But that convention doesn't match what GNOME's `ubuntu-appindicators`
host actually does. Confirmed against the extension's `dbusMenu.js`:

- On `LayoutUpdated` it re-fetches the layout with
  `GetLayout(0, -1, ['type', 'children-display'])` — i.e. it pulls back **only**
  `type` and `children-display`, never `label`/`enabled`.
- For a menu item id it already knows, it applies just those two properties from
  the re-scan. It pulls a *full* property set (`GetGroupProperties`) for **new**
  ids only.
- An already-known item's changed `label`/`enabled` therefore reaches it through
  an `ItemsPropertiesUpdated` signal **or not at all** — and `tray.py` never
  emitted that signal (its own docstring said "never actually emitted").

So the menu froze at its GUI-launch state. Reopening the menu didn't help — the
extension only re-reads properties it was told changed.

A second latent issue: item ids were allocated in tree order, so the fixed rows'
ids shifted with the Profile count (`switch`/`pause`/`quit` were 4/5/6 with one
Profile, 5/6/7 with two). The host keys its bookkeeping by id, so an
`ItemsPropertiesUpdated` patch has to target ids the host still recognises.

### Scope

- **`tray_menu.py`** — assign item ids by *role*: the five fixed rows hold ids 1-5
  for the process lifetime, Profile entries take 6, 7, 8, … in `profiles` order.
  `MenuModel.rebuild()` returns the `(changed, removed)` property delta (every id
  in both the old and new tree whose properties differ; a genuinely new id is
  omitted — `LayoutUpdated` covers it).
- **`tray.py`** — `TrayIcon.update()` emits `ItemsPropertiesUpdated(changed,
  removed)` after `LayoutUpdated` whenever the delta is non-empty. Structural
  changes (Profile add/remove) still ride the rebuild + `LayoutUpdated`.
- **Tests** — `test_tray_menu.py`: fixed-row ids stable across Profile counts;
  `rebuild()` delta reports the Pause/Resume flip and the active-Profile greying,
  is empty on a no-op, omits a newly-added Profile. `test_tray.py`:
  `update()` emits `ItemsPropertiesUpdated` for a label flip and for the greying,
  not on the first build, not on a no-op.

### Out of scope

- The status-dot icon assets (ticket 27c9b1f / ticket 11) — untouched.
- Live hardware re-verification — folded into a ticket-50-style pass if one is
  re-run (see Answer).

## Answer

Fixed as scoped. The menu now updates live because the host is told, explicitly,
which known items changed.

### What changed

- **`gui/acheron_gui/tray_menu.py`**
  - Module-level `STATUS_ID`/`SHOW_WINDOW_ID`/`SWITCH_PROFILE_ID`/`PAUSE_RESUME_ID`/
    `QUIT_ID` = 1-5 and `FIRST_PROFILE_ID` = 6. `build_menu_items` now assigns
    these directly instead of an allocation-order `add()` helper — fixed rows keep
    their id for the life of the process, so only a real Profile add/remove shifts
    anything.
  - `MenuModel.rebuild()` returns `(changed, removed)`: `changed` is
    `[(id, new_properties), …]` for every id present in both the previous and new
    tree whose `properties` dict differs; `removed` is `[(id, [dropped_names]), …]`
    (in practice always empty now that per-id key sets are stable). A brand-new id
    is left out on purpose.
  - Module + method docstrings rewritten to explain the host's actual
    re-fetch behaviour rather than the old "full rebuild is enough" claim.
- **`gui/acheron_gui/tray.py`**
  - `TrayIcon.update()` captures `rebuild()`'s delta and, when non-empty, emits
    `self._menu_service.ItemsPropertiesUpdated([...variants...], removed)` right
    after `LayoutUpdated`.
  - `_DBusMenuService.ItemsPropertiesUpdated`'s "never actually emitted" comment
    replaced with what it now carries.
- **`gui/tests/test_tray_menu.py`** — 5 new tests (stable ids, three delta shapes,
  new-Profile omission).
- **`gui/tests/test_tray.py`** — 3 new tests (label-flip signal, greying signal,
  no signal on first build / no-op), plus a `_unwrap` variant helper.

### Verification (this session)

- Full GUI suite: `325 passed`.
- Manual repro against the fake bus: before — `ItemsPropertiesUpdated` never fired
  across a pause and a Profile switch. After — pause emits a patch for the status
  row (id 1) and the Pause→Resume row (id 4); a Default→Gaming switch emits a patch
  for ids 6 and 7 with the `enabled` flags swapped. First build emits nothing.
- **Live-verified by the user** on the real GNOME panel: both reported symptoms
  are gone — the active-Profile greying follows a switch and the Pause/Resume
  Daemon label flips with the Daemon's actual state.

Status: resolved
