Type: task

## Question

Build the real system tray icon, replacing `gui/acheron_gui/device_overview.py`'s in-window
`build_tray_mock` placeholder, against the design [Decide the tray icon's look and behavior](./11-decide-tray-icon-look-and-behavior.md)
settled in full, including its 2026-08-21 resolution — `AppIndicator3`/`AyatanaAppIndicator3` is
**not** used (it hard-depends on GTK3, which cannot load in the same process as this GTK4 GUI;
see the ticket's Reopened/Resolution sections for the full finding). Instead:

- Implement a hand-rolled, in-process `org.kde.StatusNotifierItem` service via **dasbus**
  (`python3-dasbus`, a standard Ubuntu `main`-repo package, already installed on this dev
  machine — worth a line in the install docs; `libayatana-appindicator3-1`/
  `gir1.2-ayatanaappindicator3-0.1` are no longer needed). Register with
  `org.kde.StatusNotifierWatcher` (`RegisterStatusNotifierItem`) once at GUI launch. Relevant
  `StatusNotifierItem` properties: `Category`, `Id`, `Title`, `Status` (always `"Active"`),
  `IconName`+`IconThemePath`, `Menu` (an object path), `ItemIsMenu`; `Activate`/
  `SecondaryActivate`/`ContextMenu`/`Scroll` methods all just take x/y (no click-vs-menu
  distinction to implement); `NewIcon`/`NewStatus`/`NewTitle` signals push icon-state changes to
  the host. Exact interface definitions: `github.com/ubuntu/gnome-shell-extension-appindicator/
  interfaces-xml/{StatusNotifierItem,StatusNotifierWatcher}.xml`.
- Implement a minimal `com.canonical.dbusmenu` object backing `Menu`: `GetLayout`/
  `GetGroupProperties`/`GetProperty`/`Event`/`AboutToShow` methods (`AboutToShow` always reports
  `needUpdate=False` — the tree is static/eagerly built, never lazy-populated),
  `LayoutUpdated`/`ItemsPropertiesUpdated` signals. Interface definition:
  `.../interfaces-xml/DBusMenu.xml`. On any relevant change (Profile created/renamed/deleted,
  Daemon pause/resume, status transition) rebuild the whole item tree from scratch and bump
  `LayoutUpdated`'s revision — mirrors this codebase's existing full-rebuild convention
  (`app.py`'s own `rebuild()`), not incremental per-item property patches.
- Menu content, top to bottom: status line → Show Window → Switch Profile ▸ (submenu of current
  Profiles) → Pause Daemon / Resume Daemon (label flips with state) → Quit.
- **State source**: the tray module hooks into `app.py`'s existing `status`/`rebuild()` — expose
  an `update(config, profile, status)` call invoked alongside the main window's own rebuild,
  rather than running independent D-Bus subscriptions. Show Window and Quit are plain in-process
  calls (`win.present()` / `self.quit()`) now that everything is one process.
- Pause/Resume Daemon is session-only (`StopUnit`/`StartUnit` over the same session-bus proxy
  `DBusSystemdClient` (`gui/acheron_gui/systemd_client.py`) already holds — add
  `stop_daemon()`/`start_daemon()` alongside its existing `ensure_daemon_started()`); the unit
  stays login-enabled either way.
- Minimize-to-tray: a `close-request` handler on the main window hides it instead of destroying
  it; suppress quit-on-last-window-closed; only the tray menu's Quit item exits the GUI process.
- Three placeholder icon assets: filled circles at `STATUS_STATES`' exact hex values
  (`#4caf50`/`#ff9800`/`#f44336`), bundled as SVGs under `gui/acheron_gui/icons/`, referenced via
  `IconName`+`IconThemePath` (the direct equivalent of `AppIndicator3`'s `set_icon_theme_path`,
  not installed into the system `hicolor` theme).
- **Note for whoever later swaps in a commissioned final icon** (not this ticket's job, but the
  placeholders should land in the right slot for a clean drop-in replacement): ship it as a single
  scalable SVG at `gui/acheron_gui/icons/scalable/apps/<name>.svg` (the standard freedesktop
  icon-theme layout `IconThemePath` expects) — SNI hosts rasterize it on demand at whatever
  size/HiDPI-scale they need, so one file covers every panel. Design it bold and simple (flat
  colors, no fine linework or embedded text): it typically renders as small as 16-24px logical
  size before scaling.
- Tooltip: `"Acheron — <status label>"`, reusing `STATUS_STATES`' label text verbatim — still
  export the `ToolTip` property (spec-correct, works under KDE/XFCE), but GNOME's
  `ubuntu-appindicators` extension has `ToolTip`/`NewToolTip` commented out ("we don't support
  tooltip") in its own consumer interface, so it will not visibly render on this dev machine;
  drop that one checklist item below rather than treat a silent no-op as a bug.

Live-verify in a real GNOME Shell panel on this machine: icon appears and updates through all
three states (Daemon stop/start, device unplug/replug), Show Window raises the hidden window,
Switch Profile actually switches, Pause/Resume Daemon actually stops/starts the unit and the
icon reflects it, Quit actually exits the GUI process while leaving the Daemon running,
window-close hides rather than quits.

## Answer

