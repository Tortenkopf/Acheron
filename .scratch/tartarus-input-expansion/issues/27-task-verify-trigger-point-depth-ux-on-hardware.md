Type: task
Blocked by: 26
Status: resolved

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

Answer:

- **Real depth driving the bar**: Pressing a grid key drives the blue bar at varying depth.
  Smoothly as ecpected. The badge does read analog.
- **Real actuation/release persistence**: I put the markers to opposite ends of the bar and
  was able to confirm, that both thresholds behave as expected and was able to verify this
  through real haptic feedback while typing into a text editor.

  The "Set as Profile default", and "Reset all keys to
  Profile default" buttons however seem to be having no effect.
- **The digital-mode fallback actually reachable**: the "Force digital capture"
  checkbox does flip the badge to "digital", greyes out the bar and displayes the centered overlay warning.
  — the Grid keys do keep working via evdev and loose their depth functionality as expected.

  However: Checking the checkbox also imedeately closes the binding dialog. When it is reopened afterwards
  the checkbox remains unchecked, but can then be checked without closing the dialog. Unchecking the
  box afte that keeps the dialog open and switches back to analog mode as expected.
- **The live capture-mode badge flipping on a real mode change**:
  I have not been able to test this yet.

## Investigation (code, no hardware)

Followed up on the two "no effect"/asymmetric findings by reading the code (not yet
re-verified live):

- **The Force-digital-capture asymmetry is a real, now-fixed bug**: `GetConfig()` never
  serialized `Config.force_digital` — `wire::config_to_dict()` only ever wrote
  `schema_version`/`active_profile`/`profiles`, and `daemon_stub.py` had a comment saying
  this was deliberate ("nothing for this stub to reflect back"). So `binding_editor.py`'s
  checkbox always constructed as `Gtk.CheckButton`'s default (unchecked), regardless of the
  Daemon's real persisted value. This exactly reproduces the reported sequence: check → a
  real Digital transition fires `CaptureModeChanged` → app `rebuild()` closes the popover
  (by design, ticket 26). Reopen → checkbox shows unchecked even though `force_digital` is
  still `true` server-side. Check it again → `SetForceDigital(true)` is sent but the mode is
  *already* Digital, so `handle_capture_mode_change`'s dedup (`if mode == *capture_mode { return }`) emits no signal → no rebuild → dialog stays open, matching "can then be checked
  without closing the dialog." Fixed: `config_to_dict()` now serializes `force_digital`
  (daemon/src/dbus/wire.rs), and `build_actuation_section` seeds the checkbox from
  `config.get("force_digital", False)` before wiring `"toggled"` (gui/acheron_gui/
  binding_editor.py). `daemon_stub.py` updated to track and return it. New coverage:
  `config_to_dict_serializes_force_digital` (Rust) and
  `test_force_digital_checkbox_seeds_from_the_persisted_preference` (Python). 177 Rust + 80
  Python tests green. **Not yet re-verified against real hardware** — needs a rebuild/
  reinstall/restart and a retest of the checklist's third item, this time also confirming
  both directions (check *and* uncheck) now close the dialog symmetrically, since the
  "already in that mode, no-op" case that let unchecking appear not to close should no
  longer be reachable via the checkbox once it accurately reflects state.
- **"Set as Profile default" / "Reset all keys to Profile default" — real bug, found and
  fixed after the user confirmed both only took effect after a full GUI restart** (checked
  against other keys' editors, and with the staleness edge case above ruled out — still
  needed a restart). Root cause: every Grid key's popover is pre-built once, from a single
  `GetConfig()` snapshot, during the app's own `rebuild()` (`app.py`'s `last_known["config"]`)
  — and unlike `capture_mode`, there is no Daemon signal for a `default_actuation`/override
  change. `set_binding`/`clear_binding` (the main Save/Clear buttons) already call the
  `on_saved` callback (`popover.popdown()` + the app's `rebuild()`) on success, but
  `build_actuation_section`'s own handlers never had access to it, so nothing ever told the
  app its cached config was stale — hence "only works after restarting the GUI." Fixed by
  threading `on_saved` into `build_actuation_section` and calling it after a successful
  `set_default_actuation`/`reset_actuation_points` (gui/acheron_gui/binding_editor.py) —
  both affect *other* keys' popovers, so they now pop this one down and force the next one
  open to read fresh data, same as Save/Clear. `set_actuation_point`/`clear_actuation_point`
  ("Reset to Profile default" and drag-driven edits) are deliberately left alone: they only
  ever affect the current key, already update this popover's own markers directly, and must
  not close on every drag-end or the live depth-editing UX breaks. New coverage:
  `test_set_as_profile_default_closes_the_popover_and_refreshes_the_cached_config`,
  `test_reset_all_keys_to_profile_default_closes_the_popover_and_refreshes_the_cached_config`,
  and `test_reset_to_profile_default_does_not_close_the_popover` (locks in the asymmetry
  deliberately, so a future change doesn't silently make drag-driven edits close the popover
  too). 83 Python tests green. **Not yet re-verified against real hardware** — needs a
  rebuild/reinstall/restart and confirmation that a second key's editor now shows the new
  default/cleared overrides immediately (no GUI restart), and that this closing behavior
  (new, wasn't asked about in the original checklist) feels right rather than surprising.

## Re-verification on real hardware (post-fix)

All four checklist items now confirmed against the real, connected Tartarus Pro on the
rebuilt/reinstalled/restarted Daemon and a fresh GUI session:

- **Real depth driving the bar**: confirmed in the original pass, unaffected by these fixes.
- **Real actuation/release persistence**: thresholds confirmed via haptic feedback in the
  original pass. **"Set as Profile default" / "Reset all keys to Profile default" now
  confirmed working** — both show up immediately in a second key's editor, no GUI restart
  needed. The new popover-closing behavior was accepted as expected, not surprising.
- **The digital-mode fallback**: confirmed in the original pass. **The Force-digital-capture
  checkbox fix confirmed** — it's a persistent override (not a transient state): once
  checked, the Daemon stays in Digital across a device unplug/replug until explicitly
  unchecked again, which is by design (the map's Notes' "user only ever switches analog
  off" escape hatch), not a live-mode-detection gap.
- **The live capture-mode badge on a real mode change**: confirmed by testing the *automatic*
  fallback path instead of the checkbox (which is covered by the item above) — with "Force
  digital capture" unchecked, unplugging the device drove an automatic Analog→Digital
  fallback and replugging drove an automatic reconnect-triggered Digital→Analog upgrade,
  both independent of the checkbox; reopening an editor afterward correctly showed the
  "analog" badge.

Ticket 26's build (real Actuation & release editor, live depth channel, capture-mode badge,
digital-mode fallback) is now fully live-hardware-verified, with two real bugs this
verification surfaced along the way fixed and covered by tests: `force_digital` not being
serialized by `GetConfig()`, and the actuation-section's profile-wide mutations never
refreshing the app's cached config. 177 Rust + 83 Python tests green.
