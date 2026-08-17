Type: task
Blocked by: 22

## Question

Wire the analog `CaptureSource` [ticket 22](./22-task-build-analog-capture-source.md) built
into `main.rs`'s actual startup/runtime, add the udev rule, extend `install.sh`, and make
`force_digital`/`capture_mode` genuinely live end to end. HITL — this is the integration and
verification ticket; almost everything here needs the real, connected Tartarus Pro, root
access for the udev rule, and a human confirming behavior across power-cycle/reconnect/unclean
shutdown, which can't be scripted unattended.

Read [ticket 18](./18-rework-capture-path-for-analog.md)'s `## Answer` §6-8 first (live
source swap, fatality taxonomy, udev rule shape) — this ticket implements those, doesn't
re-decide them.

## Do this

**`main.rs` supervisor loop** (ticket 18 §6): today, `main` runs three tasks in one
`tokio::select!` (capture, injector, dispatch) where any of the three finishing is fatal.
Restructure so capture runs inside a small supervisor that:

- On startup, attempts `AnalogCaptureSource` unless `Config.force_digital` is set; on
  failure/fallback (per ticket 22's grid-task behavior), runs `EvdevCaptureSource` instead —
  and reports which one it landed in.
- Can swap live: a `SetForceDigital` call reaching dispatch needs a way to signal the
  supervisor to relock-and-stop the current source and start the other, without dropping
  `event_tx`/`connection_tx`/`rx_commands` or restarting dispatch/injector/the D-Bus server.
  (One route: the supervisor owns its own persistent clone of `event_tx`/`connection_tx` so
  the channels never see a spurious close during the gap between stopping one source and
  starting the next; dispatch signals the supervisor via a new channel dedicated to this,
  separate from the `Command` channel it already owns.)
- Retry-to-upgrade (digital → analog) only at the three moments ticket 18 §6 settled:
  startup, a genuine device reconnect, an explicit `SetForceDigital(false)` — no background
  polling.
- Fatality stays as ticket 18 §7 describes: dispatch or injector exiting is still fatal; a
  capture-side swap or fallback is not.
- On clean shutdown (whatever `main.rs`'s shutdown path already is, or SIGTERM if none exists
  yet — check first rather than assuming), relock the device to mode `0x00` if it's currently
  unlocked (a standing map decision, not new to this ticket).

**Make `capture_mode`/`CaptureModeChanged` live**: dispatch's `GetState` currently hardcodes
`"digital"` (ticket 21). Wire it to the supervisor's actual current source, firing
`CaptureModeChanged` on every transition — same pattern `handle_connection_change` already
uses for `DeviceConnectionChanged`.

**Make `SetForceDigital` live**: dispatch's handler (ticket 21, currently persists-only)
triggers the supervisor swap described above.

**udev rule** (ticket 18 §8): new file, likely `packaging/60-acheron-tartarus-pro.rules` or
similar (match `install.sh`'s existing `packaging/` convention — check
`packaging/acheron-daemon.service` for where that lives). Matches `idVendor=="1532"`,
`idProduct=="0244"`, grants group `plugdev` `MODE="0660"` on both hidraw interfaces.

**`install.sh`**: add a step that copies the rule to `/etc/udev/rules.d/` and runs
`udevadm control --reload-rules --trigger` via `sudo`, catching failure and printing manual
instructions (the exact rule path and the two commands) rather than aborting the rest of the
install. This is the "privileged install step" the map's Destination already accounts for.

**End-to-end verification against the real device** — all HITL, with the Daemon actually
running (not a standalone test harness like ticket 22's):

- Fresh start with the udev rule installed: Daemon lands in analog, `GetState().capture_mode`
  reports `"analog"`, all 20 grid keys work via their configured Bindings, Mode key/thumbstick/
  wheel keep working unaffected.
- `SetForceDigital(true)` while running: live swap to digital, grid keys keep working (via
  evdev passthrough/Bindings again), `CaptureModeChanged` fires, `SetForceDigital(false)`
  swaps back to analog without a Daemon restart.
- Power-cycle the device while running: reconnects in digital mode (ticket 16's finding),
  Daemon detects the reconnect and re-attempts analog automatically, succeeds, `capture_mode`
  reflects it.
- Kill the Daemon uncleanly (`kill -9`) while in analog mode, then restart it: ticket 16 found
  a fresh process's re-lock/re-unlock recovers cleanly even though the previous process never
  sent mode `0x00` — confirm the new process actually does this rather than leaving the user
  with the 20 dead grid keys ticket 16 documented as the unclean-death failure mode.
- Temporarily remove/rename the udev rule (simulating a user who hasn't installed it) and
  confirm the Daemon degrades to digital silently, with the 20 grid keys still fully
  functional via their existing Bindings (the property the map's Notes calls out as worth not
  losing).

## Answer

_(unresolved)_
