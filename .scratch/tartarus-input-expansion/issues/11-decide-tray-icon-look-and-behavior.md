Type: grilling

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

