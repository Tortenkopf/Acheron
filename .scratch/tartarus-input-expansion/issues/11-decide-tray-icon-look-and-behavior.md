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

