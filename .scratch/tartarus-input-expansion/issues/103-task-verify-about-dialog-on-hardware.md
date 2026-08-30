Type: task
Status: resolved
Assignee: Charon (2026-08-30)
Blocked by: 102

## Question

Verify [ticket 102](./102-task-build-about-dialog.md)'s About dialog live, against the real
Tartarus Pro + the daily-driver Daemon, the same discipline as every other build→verify pair
on this map (26→27, 93→94, 89→95).

Checklist:

- **Entry point**: the header bar renders on the main window; the primary menu button opens;
  "About Acheron" launches the dialog. The dialog is modal and stays `transient-for` the
  main window (centres on it, no separate taskbar entry).
- **Ticket 36 minimize-to-tray still works** with the new `Gtk.HeaderBar` — closing the main
  window hides it to the tray rather than quitting, and the tray "Show" restores it.
- **Device fields, device connected**: "Firmware" and "Serial" show the connected unit's
  real values (cross-check the serial against the sticker on the device; cross-check firmware
  against Razer Synapse on Windows if available, else just sanity-check the format).
- **Device fields, device disconnected**: unplug the Tartarus Pro — the dialog (reopened, and
  if feasible while open) shows "Not connected" / "—" for both, no crash, no stale value.
  Replug and confirm the real values return.
- **Version lines**: "Acheron <version>" matches `gui/acheron_gui/__init__.py`'s
  `__version__`; the "Daemon <version>" line matches the running Daemon's `Cargo.toml`
  version; a from-checkout run shows the `-dev+<hash>` suffix and an installed run shows the
  bare `1.0.0`.
- **Legal section**: the copyright / no-warranty / redistribution block reads exactly as
  ticket 102 specifies; "View Licence" opens the **full bundled GPLv3 text** (scrollable to
  the end); the gnu.org link opens in a browser.
- **Links**: the Wikipedia, ultramonaka, and Matt Pocock links all open in the default
  browser.
- **Formatting**: the river quote is verbatim with its `...` ellipses; placeholder rows show
  visible "TBD"; nothing is clipped at the default window size or on a resize.

Capture screenshots (connected + disconnected states). `config.toml` restored byte-identical
and the daemon put back on its normal profile afterward. GUI + Daemon suites green.

This is the last ticket in the About-dialog cluster (99 → {100 → 101} → 102 → 103).

## Answer

**Every checklist item passed live on the real Tartarus Pro + the daily-driver Daemon**,
tested by the user on **both** the installed `acheron-gui` and a from-checkout
`python3 gui/main.py` run. Ticket 102's About dialog is hardware-confirmed; the
99 → {100 → 101} → 102 → 103 cluster is closed.

Setup: `./install.sh` from the checkout (rebuilt the Daemon at `b002c8d`, synced the GUI,
bundled `LICENSE`), `systemctl --user restart acheron-daemon`.

- **Entry point**: the `Gtk.HeaderBar` renders on the main window; the `open-menu-symbolic`
  button opens the primary menu; "About Acheron" launches the dialog, modal and
  `transient-for` the main window (centres on it, no separate taskbar entry).
- **Ticket 36 minimize-to-tray** still works with the new header bar — closing the main
  window (close button now inside the `Gtk.HeaderBar`) hides it to the tray rather than
  quitting; tray "Show" restores it. The `close-request` handler was unaffected as
  predicted.
- **Device fields, connected**: Firmware `v1.2`, Serial `PM2443F36300141` — the serial
  matches the sticker on the device.
- **Device fields, disconnected**: unplugging the Tartarus Pro and reopening the dialog
  shows "Not connected" for both rows, no crash, no stale value; replugging brings the real
  values back.
- **Version lines**: installed run shows bare "Acheron 1.0.0"; from-checkout run shows
  "Acheron 1.0.0-dev+b002c8d"; the "Daemon 1.0.0-dev+b002c8d" line matches the running
  Daemon (an `install.sh`-built dev binary).
- **Legal**: the copyright / no-warranty / redistribution block reads as ticket 102
  specifies; "View Licence" opens the full bundled GPLv3 text, scrollable to the end; the
  gnu.org link opens in a browser.
- **Links**: the Wikipedia, ultramonaka, and Matt Pocock links all open in the default
  browser.
- **Formatting**: the river quote is verbatim with both `...` ellipses; the "TBD"
  placeholder rows are visible; nothing clipped at the default size or on resize.

No screenshots captured — the user verified visually in place and did not need them.
`~/.config/acheron/config.toml` untouched by the read-only dialog (md5
`2a6249ee3e69c67dabcade827a7f1d1a` throughout); active Profile left as the user's own
choice. GUI suite **355 passed**, Daemon suite **380 passed**, `packaging/test_install.sh`
green (including the ticket-102 `LICENSE`-bundling assertion).

Note for the record: during AFK prep this session briefly stopped the systemd
`acheron-daemon` unit (a stray `acheron-daemon --version` probe — no such flag — plus a
too-broad `pkill`); it exited cleanly ("relocking device and exiting"), was restarted, and
the device reconnected fine. No lasting effect.
