Type: task
Status: open
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
