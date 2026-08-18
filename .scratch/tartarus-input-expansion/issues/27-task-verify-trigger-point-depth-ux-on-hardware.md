Type: task
Blocked by: 26

## Question

Complete [ticket 26](./26-task-build-trigger-point-depth-ux.md)'s own live-hardware
verification checklist, which that session skipped rather than walked through live (the
session found a real, connected Tartarus Pro and a real running `acheron-daemon` on this
machine, but judged swapping the user's live input-device driver out from under them without
asking first outside its call — same shape as [ticket 22](./22-task-build-analog-capture-source.md)
deferring to [ticket 24](./24-task-verify-analog-capture-source-on-hardware.md)).

Checklist (ticket 19/26's original ask):

- **Real depth driving the bar**: open a Grid key's editor in the real GUI, press it at
  varying depth, confirm the live bar tracks smoothly (not just jumps digital-style) and the
  badge reads "analog".
- **Real actuation/release persistence**: drag both markers, confirm the Down/Up thresholds
  actually move on the physical key (not just visually) and survive closing/reopening the
  editor. Try "Reset to Profile default", "Set as Profile default", and "Reset all keys to
  Profile default" too — confirm each's effect on a second Grid key's own editor.
- **The digital-mode fallback actually reachable**: the section's own "Force digital capture"
  checkbox should flip the badge to "digital", greyed bar, and the centered overlay warning
  — and the Grid keys should keep working via plain evdev passthrough while it's checked.
- **The live capture-mode badge flipping on a real mode change**: with an editor open, force a
  capture-mode transition some other way (unplug/replug, or `SetForceDigital` from a second
  `busctl`/GUI-adjacent call) and confirm... actually, per this map's Notes and ticket 26's own
  build, a capture-mode transition drives a full `rebuild()` and will *close* whatever popover
  is open, by design (same as every other status-driven rebuild) — so what to actually confirm
  here is that the badge shows the *correct* mode the next time an editor is reopened, not that
  it flips live under an open popover. Confirm that.

Steps to actually run this: `cd daemon && cargo build --release`, then either the repo's
`install.sh` (rebuilds + reinstalls the binary but does **not** restart an already-running
unit — the user needs `systemctl --user restart acheron-daemon` themselves afterward to
actually pick up the new binary) or a manual `cp target/release/acheron-daemon ~/.local/bin/`
+ restart. Launch the GUI with `cd gui && .venv/bin/python main.py`.

HITL — needs the real, connected Tartarus Pro and a live GUI session, same as tickets 22/23/24.
