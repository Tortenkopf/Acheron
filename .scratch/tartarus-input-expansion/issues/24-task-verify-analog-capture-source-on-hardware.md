Type: task
Blocked by: 22
Status: resolved

## Question

Complete [ticket 22](./22-task-build-analog-capture-source.md)'s own HITL verification
checklist, which that session skipped rather than walked through live. Ticket 22 landed
`AnalogCaptureSource` (the `hidraw` grid task, discovery, unlock lifecycle, threshold/repeat
state machine) and unit-tested every pure function exhaustively, but none of it has been
confirmed against the real, connected Tartarus Pro (`1532:0244`) by a human watching it run.

Ticket 22 had initially planned to walk this step through with the user directly, then
decided instead to do it solo and hand the user `daemon/examples/analog_probe.rs` to run
unsupervised — a decision made in the moment about who performs the `sudo`/`chmod` step in a
separate terminal, not a considered call about whether this should be a joint session. This
ticket puts it back as what it was meant to be: run **together**, in one sitting — the human
drives the terminal that needs elevated `hidraw` access (no udev rule exists yet; that's
[ticket 23](./23-task-wire-analog-supervisor-and-install.md)'s job) and presses keys at
varying depth, the agent runs `analog_probe`, watches the output stream, and judges it against
the checklist below in the same session.

## Do this

Using `daemon/examples/analog_probe.rs` (`cargo run --example analog_probe
[duration_seconds]`) against the real device, with the human granting temporary `hidraw`
access (`sudo`, or a temporary `chmod` on `/dev/hidraw0-2`) in their own terminal:

- Confirm the unlock succeeds and report `0x06` arrives.
- Confirm all 20 grid keys threshold correctly at the default 128/112 actuation/release
  points — press each key at varying depth and check the observed Down/Up transitions land
  where expected.
- Confirm Hold-to-repeat synthesizes at a rate that looks right against the cached kernel
  `(delay_ms, period_ms)` (`Device::get_auto_repeat()`).
- Confirm a permission-denied `hidraw` open degrades silently rather than erroring — test
  before granting access, or revoke it mid-run.
- Confirm Main/If02 events (Mode key, thumbstick, wheel) keep flowing unaffected while the
  grid task runs alongside them.

Re-lock the device afterward via the already-verified
`prototype/13-analog-grid-capture/prototype.py relock` (the same reminder `analog_probe`
prints on exit).

If anything here surfaces a real bug (not just a UX rough edge), fix it in `capture/analog.rs`
before resolving — this ticket closes ticket 22's verification gap, it doesn't relocate it.

## Answer

Ran together, live, against the real Tartarus Pro (`1532:0244`), across three
`analog_probe` sessions (grid coverage, then a non-grid follow-up split in two
after an editor-focus miss on the first attempt). All five checklist items
confirmed, no code changes needed:

- **Unlock/confirmation**: `connection: true` every run, report `0x06` arrived
  each time within the timeout.
- **All 20 grid keys, 128/112 thresholding**: every key `r1c1`–`r4c5` fired a
  clean Down/Up pair, Down landing 128–135 and Up landing 85–112 — matching
  the default Actuation/Release points, no missed or double transitions.
- **Hold-to-repeat cadence**: held keys (e.g. `r4c5`) produced dense Repeat
  streams (~150 Repeats over the hold) consistent with the kernel's own fast
  autorepeat period read via `get_auto_repeat()` — no hardcoded-fallback
  cadence observed.
- **Permission-denied degrades silently**: run before `hidraw` access was
  granted (nodes were `root:root`, un-readable) — no crash, no error, just
  `connection: false` and 0 events until access was fixed.
- **Main/If02 unaffected**: Mode key, all four thumbstick directions, and the
  wheel (scroll up/down, middle-click) all produced ordinary paired Down/Up
  events over evdev while the grid task ran alongside them.

Two things surfaced beyond the checklist itself, neither treated as "real
bugs" against `capture/analog.rs` (both are pre-existing/tooling, not new
logic-correctness gaps):

- **The device must be re-locked between runs.** `analog_probe` never sends
  the relock itself (documented as the human's job on exit) — sending a fresh
  unlock while the device is *already* in Analog Capture mode does not
  produce a new report `0x06` (ticket 16's "the standby report marks a mode
  *transition*, not every `set_device_mode`" finding, hit directly). A
  second `analog_probe` run against an already-unlocked device just spins in
  `wait_for_unlock_confirmation`'s absence-retry bucket forever, printing
  `connection: false`. Not a bug — matches the documented protocol — but easy
  to trip during iterative manual verification; re-run
  `prototype/13-analog-grid-capture/prototype.py relock` between attempts.
- **`analog_probe` (and, structurally, `AnalogCaptureSource`/`EvdevCaptureSource`
  generally) never exits on its own once started.** The example's timer only
  breaks its own `select!` print loop; the underlying `spawn_blocking` capture
  tasks (both evdev nodes' `fetch_events` blocking read and the grid task's
  absence-retry `sleep`) have no shutdown signal and loop forever, so Tokio's
  `Runtime::drop` blocks waiting for them and the process never returns —
  needs a `kill -9`. This is pre-existing behavior shared with
  `EvdevCaptureSource`'s retry loops (not introduced by ticket 22), and in
  the real daemon a process signal (SIGTERM/SIGINT) kills everything
  regardless — so not a fix-in-this-ticket bug. Left as a note for
  [ticket 23](./23-task-wire-analog-supervisor-and-install.md): if `main.rs`'s
  shutdown path ever relies on closing channels rather than a process signal,
  a capture task parked in an absence-retry sleep won't notice and shutdown
  will hang. Also cost real time in this session: a stale `analog_probe` from
  an earlier run held the evdev nodes' exclusive `grab()`, causing a
  follow-up run to fail with `EBUSY` until it was killed manually.

Device re-locked (mode `0x00`) via `prototype.py relock` at the end of the
session; no stray `analog_probe`/`prototype.py` processes left running.
`ticket 22`'s verification gap is closed — `AnalogCaptureSource` behaves
correctly on real hardware for grid thresholding, repeat synthesis,
non-grid passthrough, and permission-absence degradation.
