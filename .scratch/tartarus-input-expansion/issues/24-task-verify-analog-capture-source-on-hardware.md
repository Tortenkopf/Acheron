Type: task
Blocked by: 22

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

_(unresolved)_
