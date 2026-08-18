Type: task
Blocked by: 22, 24
Status: resolved

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

Built and verified live against the real Tartarus Pro (device access pre-granted this session).

**`capture::supervisor::run`** (new module) owns *which* `CaptureSource` runs, per ticket 18
§6: on startup, attempts `AnalogCaptureSource` unless `force_digital`; gives it up to 6s
(`ANALOG_STARTUP_GRACE`) to reach a fully-connected grid before falling back to
`EvdevCaptureSource::all` — the only external signal available for "hidraw permission missing"
is "the grid's presence slot never went true," since ticket 22's absence-retry bucket treats
`PermissionDenied` exactly like a not-yet-plugged-in device. `SetForceDigital` reaches it via a
new `capture_control_tx`/`rx` channel (dispatch forwards on every successful persist); its own
`CaptureMode` pushes into a new `capture_mode_tx`/`rx` channel dispatch consumes for
`GetState`/`CaptureModeChanged` (mirrors the existing `device_connected`/
`DeviceConnectionChanged` pattern exactly, including a `handle_capture_mode_change` twin of
`handle_connection_change`). A genuine reconnect (`Some(false) -> true`) while running Digital
*only because* Analog didn't come up retries Analog, gated by both `ever_connected` (see the bug
below) and never while `force_digital` is set.

**A real EVIOCGRAB hazard, found during design, not live**: ticket 18 §6's "stop one JoinSet and
start the other" only works if "stop" actually releases every grab before the next source tries
to acquire the same nodes — otherwise the incoming source's own `grab()` fails `EBUSY` (not an
absence condition, fatal). `tokio::JoinHandle::abort()` doesn't preempt `spawn_blocking` tasks
mid-syscall, so it can't provide this on its own. Fixed by adding real cooperative cancellation
to both `evdev_source` and `analog`: evdev nodes go non-blocking + poll-with-timeout (matching
`analog`'s existing hidraw pattern, sharing `poll_readable`, `interruptible_sleep` for
absence-retry waits), every blocking loop checks a shared `Arc<AtomicBool>` shutdown flag, and
`join_first` — now shutdown-aware — drains every sibling task before returning once the flag is
set, so `handle.await` in the supervisor only resolves once every grab is genuinely released.
Confirmed live: swapping repeatedly showed exactly one fd per node afterward, never duplicates.

**Two more bugs found only by running it, both fixed and reverified live:**
- **SIGTERM/SIGINT hung the process indefinitely** whenever any capture task was still
  legitimately retrying (e.g. the grid stuck on `PermissionDenied`) — `#[tokio::main]`'s
  generated wrapper drops the multi-thread `Runtime` at the end of `main`, and that `Drop`
  blocks waiting for *every* outstanding task (nothing SIGTERM touched had been told to stop).
  Fixed by ending the process with `std::process::exit` after the best-effort relock, skipping
  Rust's destructors/task-draining entirely — the immediate-exit reading of ticket 24's "harmless
  under a real process-signal shutdown" note, not the graceful-drain reading. Reverified: exits
  in ~100ms now, even with the grid task permanently stuck retrying.
- **An infinite Digital/Analog thrash loop** — a *fresh* `EvdevCaptureSource::all` attempt's own
  three nodes converge to "all connected" one at a time, so its first presence messages are
  typically `false` then `true`, indistinguishable from a genuine reconnect by the original
  `last_connected == Some(false)` check alone. Every fresh fallback immediately mistook its own
  startup convergence for "device reconnected," swapped straight back to Analog, hit the same
  grace timeout again, and repeated — a de facto background-timer, exactly what ticket 18 §6
  rules out. Fixed with an `ever_connected` gate: the reconnect check now requires this attempt
  to have reached a genuine fully-connected state at least once already. Reverified: a fresh
  fallback with the udev rule missing now logs exactly one swap attempt and settles in `digital`.

**udev rule**: `packaging/60-acheron-tartarus-pro.rules`, matching `idVendor=="1532"`,
`idProduct=="0244"`, `MODE="0660"`/`GROUP="plugdev"` (covers both hidraw interfaces via the
vendor/product match rather than interface number). `install.sh` copies it to
`/etc/udev/rules.d/` and reloads/triggers udev via `sudo`, catching failure and printing manual
recovery instructions rather than aborting the rest of the install — the privileged step the
map's Destination already accounted for. `packaging/test_install.sh` stubs `sudo` (same pattern
as its existing `cargo`/`systemctl` stubs) so the automated coverage never touches the real
system. **Not yet run for real on this machine** — `install.sh`'s udev/systemd steps need the
user's own `sudo`; this session verified the built binary standalone instead.

**Live HITL verification, all against the real device**, using `gdbus`/D-Bus calls plus
`python-evdev` watchers on both the virtual output device and the grid's raw `hidraw`/evdev
nodes (the user driving physical key presses, the agent driving the daemon/observing):
- Fresh start lands in `analog`, `device_connected: true`; all of grid (bound and unbound-passthrough
  keys, correct Hold-to-repeat), thumbstick, wheel, and a grid-bound mouse-button Binding
  confirmed live via the virtual output device.
- `SetForceDigital(true)`/`(false)` swap live in both directions with zero leaked/duplicate fds
  (checked `/proc/<pid>/fd` after each swap) — the EVIOCGRAB fix confirmed working, not just
  theorized.
- `kill -9` while in Analog, then restart: the fresh process re-locks/re-unlocks cleanly, lands
  back in `analog` — matches ticket 16's finding.
- A real power-cycle (unplug/replug) reverted hidraw permissions to root-only (no persistent
  udev rule installed on this machine yet) — `device_connected` correctly stayed `false` since
  the grid's own absence-retry bucket treats `PermissionDenied` as absence; restoring permissions
  let the grid task self-heal back to `analog` *without a daemon restart*, purely via its own
  retry-and-reopen loop (ticket 18 §2) — no supervisor swap involved, confirming Analog's
  self-healing needs no help from this ticket's code.
- Fresh start with hidraw permission denied: settles in `digital` after the grace window (one
  swap attempt, not a thrash), grid/thumbstick/wheel/Mode-key-click all confirmed working via
  evdev passthrough.
- Clean `SIGTERM`: relocks and exits in ~100ms.

**One genuine scare, traced to test-sequencing, not a code bug**: mid-session the grid produced
*zero* output — confirmed even bypassing Acheron entirely (raw kernel evdev on the grid's If01
node) — because a relock attempt had failed (hidraw permission revoked for the fallback test at
exactly the wrong moment) and the device was left stuck in driver mode, silencing its own normal
keycodes with no daemon running to resume the analog stream either. `analog::relock()` (a fresh
standalone call, no running daemon needed) recovered it instantly once permissions were restored,
and the subsequent full verification pass confirmed the device's own mode lifecycle — not this
ticket's code — was the entire story. Worth recording since it's exactly ticket 16/the map's
documented "unclean death leaves the user with dead grid keys" failure mode, just triggered by a
relock failing rather than a crash, and confirms `relock()` is a real, working recovery path a
future troubleshooting doc (the "Not yet specified" release-documentation fog) should mention.

**Residual limitation, accepted rather than engineered around**: the grace-period fallback only
arms for a *fresh* Analog attempt (startup, or after an explicit swap). If Analog is already
running and its grid task alone loses `hidraw` access mid-session (e.g. a udev rule removed,
then a replug, while the daemon keeps running throughout) without the process restarting, the
grid stays silently dead until something restarts the daemon or fixes access and the grid task's
own retry loop reopens on its own — the supervisor never notices to fall back to full Digital.
In the shipped/installed case this can't actually arise (the persistent udev rule reapplies on
every replug automatically), and a Daemon restart trivially recovers via the startup grace
period either way, so this wasn't hardened against — flagged here rather than silently accepted.

No `capture/analog.rs`/`evdev_source.rs` behavior changed for existing, already-verified paths
(ticket 22/16's ten decisions and thresholding/repeat logic are untouched); everything above is
additive (shutdown cooperation, the supervisor, the udev rule/install step) or a fix to
integration-only code this ticket itself introduced. All 170 daemon tests pass;
`packaging/test_install.sh` passes with `sudo` stubbed.
