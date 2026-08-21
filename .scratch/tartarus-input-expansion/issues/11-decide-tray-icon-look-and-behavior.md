Type: grilling
Status: resolved

## Question

Design the real tray icon's look and behavior, replacing `gui/acheron_gui/device_overview.py`'s in-window `build_tray_mock` placeholder. Builds on two prior decisions rather than starting blank: [Design Daemon/device status indicators](../tartarus-keybinder/issues/12-design-daemon-device-status-indicators.md) already settled *what* status the tray line reflects (Daemon-running/Device-connected, mirroring the header badge), and [Determine GNOME/Wayland-specific assumptions](./issues/10-research-de-display-server-compatibility.md) already settled the library (`AppIndicator3`/`AyatanaAppIndicator3`, portable via `StatusNotifierItem`, needing the `ubuntu-appindicators` extension only under GNOME). This ticket is about the parts that aren't decided yet: how it actually looks and behaves as a real tray icon rather than an in-window widget.

Settle at least:

- **Icon states**: does the icon itself change (color/glyph) per Daemon-running/Device-connected combination, the way the header badge does, or does a single static icon rely on tooltip/menu text for state? If it does change, what are the actual visual states and their icon assets?
- **Menu contents**: the mock currently has a `Gtk.MenuButton` popover showing only a status line. Does the real menu need more — Quit, Open GUI (raise/focus the main window), a Profile-switch shortcut, anything else — or does it stay intentionally minimal, matching the mock?
- **Click behavior**: typical SNI-tray convention distinguishes left-click (often "activate," e.g. raise the main window) from right-click (menu). Decide what each does here, or whether everything routes through one menu.
- **Tooltip content**: does hovering show anything beyond what the menu/icon state already conveys?
- **Icon asset needs**: does this require actual icon image files (and if so, light/dark-theme variants, since tray icon theming conventions vary by DE), or can it reuse an existing asset/generate one simply?

Likely warrants `/prototype` per the "how should it look/behave" test — decide during the session whether a throwaway prototype is worth building before locking the design.

Resolving this spawns the actual build ticket (Task, design already fully specified by then) that wires this against `AppIndicator3`/`AyatanaAppIndicator3` and verifies live in the system tray.

## Answer

Grilling session, 2026-08-20. Two grounding facts found before asking anything, both load-bearing on the design: (1) `AyatanaAppIndicator3`'s typelib wasn't installed in this dev sandbox (`gir1.2-ayatanaappindicator3-0.1` — a standard Ubuntu `main`-repo package, not a PPA; the user installed it mid-session) — `libayatana-appindicator3-1` itself was already present; (2) `AppIndicator3.set_icon_full()` takes an icon name/path, not Pango markup, so the emoji glyphs (`🎮`/`🔌`/`💀`) `STATUS_STATES` already uses for the header badge/tray mock can't be reused as literal tray-icon glyphs — a per-state tray icon needs real image assets.

**Lifecycle — minimize-to-tray.** Closing the main window (titlebar ✕) hides it rather than quitting the `Gtk.Application`; the app stays resident with the icon in the tray, and only the tray menu's own **Quit** item actually exits the GUI process. The Daemon is unaffected either way — it's already an independent `systemd --user` service (per `.scratch/tartarus-keybinder/issues/10-decide-systemd-service-packaging.md`), not something the GUI process owns. Chosen because the ticket's own candidate menu item ("Open GUI") only has meaning under this model, and the whole point of a tray icon here is answering "is my Daemon alive/connected" and offering a quick Profile switch without a window open.

**Click behavior — no left/right distinction exists to design.** Checked directly against GNOME's `ubuntu-appindicators` extension source (`/usr/share/gnome-shell/extensions/ubuntu-appindicators@ubuntu.com/appIndicator.js` + `indicatorStatusIcon.js`): it introspects the SNI's D-Bus interface for an `Activate` method to decide whether primary-click can raise the app, and `AppIndicator3` never exports one — by original design, this library is menu-only, with no click-to-activate signal at all. So every click, left or right, opens the same menu; "Show Window" is just its top item, not a click gesture. This isn't a decision, it's a fact about the chosen library (already locked in by [Determine GNOME/Wayland-specific assumptions](./10-research-de-display-server-compatibility.md)).

**Menu**, top to bottom: status line (existing 3-way `STATUS_STATES` text) → **Show Window** → **Switch Profile** ▸ (submenu listing current Profiles, replacing the tray mock's flat quick-switch list now that the menu has three other fixed items competing for space) → **Pause Daemon** / **Resume Daemon** (single item, label flips with state) → **Quit**. Pause/Resume is session-only (`systemctl --user stop`/`start acheron-daemon`) — the unit stays login-enabled either way, so a paused Daemon still comes back automatically at the next login. Rejected also toggling autostart (`disable`/`enable`) from the same control: that's a rarer, heavier action better left to a terminal, and folding it into a tray toggle risks an accidental "why didn't Acheron come back after reboot."

**Icon states — change per-state, matching the header badge.** Placeholder assets: plain filled circles at `STATUS_STATES`' exact hex values (`#4caf50`/`#ff9800`/`#f44336`), bundled as SVGs under `gui/acheron_gui/icons/` and loaded via `set_icon_theme_path` rather than installed into the system `hicolor` icon theme (avoids an `install.sh` step for icons alone). No light/dark-theme variants needed — these are full-color (non-symbolic) icons, not subject to the monochrome-inversion theming that only applies to symbolic icons. The user will commission a real icon later; this placeholder is deliberately the simplest legible shape at typical 16-22px panel size (a thin ring was considered and rejected — risks disappearing at that size).

**Tooltip**: `"Acheron — <status label>"`, reusing `STATUS_STATES`' label text verbatim (e.g. "Acheron — Connected"). Free to add, covers a glance at the color without knowing what it means yet.

**No `/prototype` ticket** — decided directly rather than building a throwaway harness. A system tray icon's design surface is fixed by OS/DE chrome (icon + menu, no free layout) rather than the open-ended "how should it look" surface prior UX prototypes (Binding editor, Chord recording) needed, and the spawned build ticket already carries mandatory live verification in the real system tray.

Spawns [Build and verify the real tray icon](./issues/36-task-build-tray-icon.md) — design is now fully specified; that ticket wires `AppIndicator3`/`AyatanaAppIndicator3` for real (replacing `build_tray_mock`), builds the three placeholder SVGs, and live-verifies against a real GNOME Shell panel (this machine now has the typelib installed).

## Reopened — 2026-08-21, during ticket 36's build session

**The library choice this ticket built on top of is unusable as designed.** Confirmed directly in this Python environment: `AyatanaAppIndicator3`'s typelib hard-depends on GTK **3.0**, and `gi` refuses to load GTK 3.0 and GTK 4.0 in the same process, in either import order —

```
gi.require_version('Gtk','4.0'); from gi.repository import Gtk
gi.require_version('AyatanaAppIndicator3','0.1')
→ gi.RepositoryError: Requiring namespace 'Gtk' version '3.0', but '4.0' is already loaded
```

reversing the import order fails the same way. `acheron_gui/app.py` is GTK4 (`gi.require_version("Gtk", "4.0")`), so `AppIndicator3`/`AyatanaAppIndicator3` cannot be loaded in-process alongside it — not a packaging gap, a hard incompatibility. Neither this ticket's own grilling session nor [ticket 10's research](./10-research-de-display-server-compatibility.md) actually imported the library next to GTK4 to check; both reasoned from the D-Bus-level `StatusNotifierItem` portability story, which is real, but doesn't cover the in-process GTK version conflict.

This invalidates this ticket's "library choice is settled, only look/feel is undecided" framing — the framing itself needs redeciding, not just re-verifying. Everything else this ticket settled (minimize-to-tray lifecycle, menu contents, icon states, tooltip, no left/right-click distinction) is about *StatusNotifierItem* as a protocol and stays valid regardless of which library/process implements it.

Two realistic paths, surfaced before reopening (user chose to resolve this as its own session rather than decide inline while building ticket 36):
- **Separate GTK3 helper process**: a small second process hosts the real `AppIndicator3` icon + `libdbusmenu-gtk3` menu (both already installed: `gir1.2-ayatanaappindicator3-0.1`, `libdbusmenu-gtk3-4`), talking to the Daemon/systemd over D-Bus directly for status/actions, and to the main GTK4 app only for Show-Window/Quit via GApplication's built-in D-Bus activation/actions. Reuses real, spec-correct SNI+menu code; adds a second process to manage/package/autostart.
- **Hand-rolled in-process SNI service**: implement `org.kde.StatusNotifierItem` and `com.canonical.dbusmenu` directly over `Gio.DBus`, no GTK3 involved, single process. More net-new protocol code to get right against GNOME Shell's specific `ubuntu-appindicators` extension, higher bug risk, no process-management complexity.

Status set back to `reopened` rather than `resolved`; [ticket 36](./issues/36-task-build-tray-icon.md) is blocked on this being resolved again.

### Resolution — grilling session, 2026-08-21

**Hand-rolled, in-process `org.kde.StatusNotifierItem` service — no `AppIndicator3`, no GTK3, no second process.** Chosen over the separate-GTK3-helper-process alternative after establishing the actual protocol facts (sourced directly from the interface XML the GNOME `ubuntu-appindicators` extension implements, mirroring KDE's canonical spec — `org.kde.StatusNotifierItem`, `org.kde.StatusNotifierWatcher`, `com.canonical.dbusmenu`, all fetched from `github.com/ubuntu/gnome-shell-extension-appindicator/interfaces-xml/`): the GTK3 dependency lives entirely inside `libappindicator`'s client convenience wrapper, not in the protocol itself. GNOME Shell has **no native SNI support at all** — it depends wholly on the `ubuntu-appindicators`/"AppIndicator and KStatusNotifierItem Support" extension acting as both Watcher and Host, and that extension only cares that *something* on the session bus correctly implements `org.kde.StatusNotifierItem` and calls `RegisterStatusNotifierItem` — it has no idea what produced that D-Bus object. So hand-rolling costs zero desktop-compatibility ground versus the original `AppIndicator3` plan: extension still required under GNOME (verified live on this machine either way), still asserted-but-unverified-native on KDE/XFCE (no such hardware available to actually test, same bar ticket 10 already accepted).

**What the service actually is**, confirmed against the real interface definitions:
- `org.kde.StatusNotifierItem` — exported by the GUI process itself (the "item"). Relevant properties: `Category`, `Id`, `Title`, `Status` (always reported `"Active"` — no `NeedsAttention`/blinking use, matches the original answer's decision not to have an attention-icon variant), `IconName` + `IconThemePath` (the direct equivalent of `set_icon_theme_path` — the three placeholder SVGs plan carries over unchanged), `Menu` (an object path), `ItemIsMenu`. Methods `Activate`/`SecondaryActivate`/`ContextMenu`/`Scroll` all just take x/y — reconfirms the original answer's finding that there's no real click-vs-menu behavior to design. Signals `NewIcon`/`NewStatus`/`NewTitle` push live icon-state changes to the host.
- `org.kde.StatusNotifierWatcher` — a well-known bus name; the item calls `RegisterStatusNotifierItem` on it once at GUI launch.
- `com.canonical.dbusmenu` — a second object, pointed to by `Menu`, backing the actual popup: `GetLayout`/`GetGroupProperties`/`GetProperty`/`Event`/`AboutToShow` methods, `LayoutUpdated`/`ItemsPropertiesUpdated` signals. This is the real net-new protocol surface (a revisioned tree of items with properties) — everything except a static tree implementation is out of scope here (`AboutToShow` always reports `needUpdate=False`, no lazy-populated submenus needed).
- **Tooltip caveat, new finding**: the extension's own consumer XML has `ToolTip`/`NewToolTip` commented out with the note *"we don't support tooltip, so no need to go through it"* — under GNOME specifically, a tray tooltip will not visibly render regardless of implementation. Still exported (spec-correct, works under KDE/XFCE hosts that do support it, costs nothing), but ticket 36's live-verification checklist can't confirm it visually on this machine — drop that one checklist item rather than treat it as a bug if it's silently a no-op under GNOME.

**Implementation choices settled alongside the architecture:**
- **`python3-dasbus`** for the D-Bus service (a standard Ubuntu `main`-repo package, already present on this dev machine, same tier as the now-unneeded `gir1.2-ayatanaappindicator3-0.1`) — its declarative class-based interface export meaningfully reduces the hand-written GVariant marshaling needed for the DBusMenu layout protocol's nested tuple structures, over raw `Gio.DBus` (which would need hand-written introspection XML too, despite PyGObject/Gio already being a hard dependency).
- **Menu content — full rebuild + `LayoutUpdated` revision bump** on any relevant change (Profile created/renamed/deleted, Daemon pause/resume, status transition), rather than incremental `ItemsPropertiesUpdated` deltas against stable item ids — mirrors this codebase's existing full-rebuild convention (`app.py`'s own `rebuild()` on every Config/state change; ticket 26's explicit choice of full-rebuild for rare transitions like `CaptureModeChanged`). Nothing in this menu updates at a frequency that would make incremental patching worth its extra bookkeeping.
- **State source — the tray hooks into `app.py`'s existing `status`/`rebuild()`**, exposing an `update(config, profile, status)` call invoked alongside the main window's own rebuild rather than running independent D-Bus subscriptions. One source of truth, no duplicate signal wiring.
- **Lifecycle simplifies for free**: minimize-to-tray stays exactly as originally decided (window-close hides, only the tray's own Quit exits), but Show Window and Quit are now trivial in-process calls (`win.present()` / `self.quit()`) — the GApplication D-Bus-activation bridge the two-process alternative would have needed is moot once everything is one process.
- `SystemdClient` gains `stop_daemon()`/`start_daemon()` (`StopUnit`/`StartUnit` over the same session-bus proxy `DBusSystemdClient` already holds) alongside its existing `ensure_daemon_started()`, for the Pause/Resume menu item.

Ticket 36 rewritten to match this design (dasbus-based in-process service, not `AppIndicator3`) and unblocked.

