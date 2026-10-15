# 08: Only signal the tray host when the tray actually changed

Spec: [`../spec.md`](../spec.md). Filed from ticket 07's hardware verification.

**Found by:** ticket 07's live session. With the fixed GUI hidden in the tray, about 960
`ActiveLayerChanged` signals (each Mode-key press and each release) left GUI memory roughly flat,
but the tray menu's `LayoutUpdated` revision had climbed to 962. The Acheron tray menu stopped
opening, and then the whole desktop stopped responding to clicks while the pointer still moved.
Clicks came back the moment the GUI process was killed. `TrayIcon.update()` runs on every trigger
(ticket 06 keeps that so the tray stays live while hidden). Each call rebuilds the menu and emits
`LayoutUpdated`, `NewIcon`, `NewTitle` and `NewStatus`, even when nothing the tray shows changed,
and a Layer change never changes the tray. GNOME's `ubuntu-appindicators` extension runs inside
gnome-shell and re-fetches the menu and icon for every one of those signals (measured at about
4 ms of gnome-shell CPU per `update()`). This predates the leak fix; the old GUI ran out of memory
before anyone saw it.

**What to build:** `TrayIcon.update(config, profile, status)` does nothing (no menu rebuild, no
revision bump, no D-Bus signal) when the status, the active Profile and the Profile list are all
the same as at the last `update()`. Anything that changes one of them behaves exactly as today.

**Blocked by:** None.

**Status:** done (2026-10-15)

- [x] Repeated `update()` calls with unchanged status, Profile and Profile list emit no
      `LayoutUpdated`, `ItemsPropertiesUpdated`, `NewIcon`, `NewTitle` or `NewStatus`, and leave
      the menu revision unchanged.
- [x] The first `update()` still publishes the menu and icon.
- [x] A change to the status, the active Profile or the Profile list (added, removed, renamed)
      still rebuilds and signals as today.
- [x] Tested against `test_tray.py`'s fake bus.

## Comments

- 2026-10-15: Done. `TrayIcon.update()` returns early when `(status, profile, Profile names)`
  matches what the last call published. `_shown` is recorded only after the signals go out, so a
  failure part-way retries on the next call. The Profile list compares as an ordered list because
  menu ids follow its order. `build_menu_items` reads nothing else from `config`, and the item's
  title and status are constants, so nothing the tray shows can go stale. Three fake-bus tests in
  `test_tray.py`: no signals or revision bump on an unchanged call, the first call publishes,
  and status/Profile/Profile-list (add, rename, remove) changes still signal. GUI suite 658 green.
  Two-axis `/code-review`: the name, typing and test-idiom nits and the record-after-publish
  ordering were applied. Still to do: ticket 07's live retest with gnome-shell CPU logged.
