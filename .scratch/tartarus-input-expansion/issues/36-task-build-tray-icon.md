Type: task

## Question

Build the real system tray icon, replacing `gui/acheron_gui/device_overview.py`'s in-window
`build_tray_mock` placeholder, against the design [Decide the tray icon's look and behavior](./11-decide-tray-icon-look-and-behavior.md)
settled in full:

- Wire `AppIndicator3`/`AyatanaAppIndicator3` for real (`gir1.2-ayatanaappindicator3-0.1` is now
  installed on this dev machine — a standard Ubuntu `main`-repo package, worth a line in the
  install docs).
- Minimize-to-tray: a `close-request` handler on the main window hides it instead of destroying
  it; suppress quit-on-last-window-closed; only the tray menu's Quit item exits the GUI process.
- Menu, top to bottom: status line → Show Window → Switch Profile ▸ (submenu of current Profiles)
  → Pause Daemon / Resume Daemon (session-only `systemctl --user stop`/`start acheron-daemon`,
  label flips with state) → Quit.
- Three placeholder icon assets: filled circles at `STATUS_STATES`' exact hex values
  (`#4caf50`/`#ff9800`/`#f44336`), bundled as SVGs under `gui/acheron_gui/icons/`, loaded via
  `set_icon_theme_path` (not installed into the system `hicolor` theme).
- **Note for whoever later swaps in a commissioned final icon** (not this ticket's job, but the
  placeholders should land in the right slot for a clean drop-in replacement): ship it as a single
  scalable SVG at `gui/acheron_gui/icons/scalable/apps/<name>.svg` (the standard freedesktop
  icon-theme layout `set_icon_theme_path` expects) — SNI hosts rasterize it on demand at whatever
  size/HiDPI-scale they need, so one file covers every panel. Design it bold and simple (flat
  colors, no fine linework or embedded text): it typically renders as small as 16-24px logical
  size before scaling.
- Tooltip: `"Acheron — <status label>"`, reusing `STATUS_STATES`' label text verbatim.

Live-verify in a real GNOME Shell panel on this machine: icon appears and updates through all
three states (Daemon stop/start, device unplug/replug), Show Window raises the hidden window,
Switch Profile actually switches, Pause/Resume Daemon actually stops/starts the unit and the
icon/tooltip reflect it, Quit actually exits the GUI process while leaving the Daemon running,
window-close hides rather than quits.

## Answer

