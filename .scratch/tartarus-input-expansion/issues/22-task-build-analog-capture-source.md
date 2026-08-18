Type: task
Status: resolved
Blocked by: 21

## Question

Build the analog `CaptureSource` itself — the hidraw grid task, its threshold/repeat logic,
and the plumbing that feeds it live actuation points. HITL: the discovery/unlock lifecycle and
the threshold/repeat behavior need verification against the real, connected Tartarus Pro (a
human pressing grid keys at varying depth), though most of the code — especially the pure
functions this ticket is designed around — can be written and unit-tested without it.

Read [ticket 18](./18-rework-capture-path-for-analog.md)'s `## Answer` in full first — it's
the spec this ticket and [ticket 23](./23-task-wire-analog-supervisor-and-install.md)
implement between them, worked out via a full grilling session against the actual code,
[ticket 12](./12-research-linux-analog-grid-key-protocol.md)'s protocol research, and
tickets [13](./13-task-standalone-analog-capture-prototype.md)/
[16](./16-task-analog-mode-hardware-facts.md)'s hardware findings. Don't re-derive any of
those decisions — implement them. `prototype/13-analog-grid-capture/prototype.py` is the
working reference implementation for the protocol bytes (unlock/relock buffers, CRC, the
`HIDIOCSFEATURE(91)` ioctl number, sysfs discovery) — port its constants, don't recompute them.

This ticket assumes [ticket 21](./21-task-apply-analog-data-model-to-code.md) has landed
(`PhysicalEvent.depth`, `ActuationPoint`, `Profile.default_actuation`/`actuation_overrides`
all exist in code). It does **not** wire the result into `main.rs` or make `force_digital`/
`capture_mode` live — that's ticket 23, deliberately kept separate so this ticket can focus
entirely on the capture module and be verified in isolation (a standalone test binary or
`#[tokio::test]`s against the real device, not a full Daemon run).

## Do this

**Generalize `evdev_source.rs`**: its node loop currently hardcodes `Node::ALL`. Change
`capture_node_blocking`'s caller (`EvdevCaptureSource::run`) to accept an explicit node list,
so the same per-node open/grab/relay/retry logic (including `is_device_absent`) can be reused
for a Main+If02-only subset without duplicating it.

**New module, `daemon/src/capture/analog.rs`** (or similar — name it for what it does, not
"driver mode"; CONTEXT.md's Capture-mode entry explicitly avoids that as a term):

- `AnalogCaptureSource`: composes the generalized evdev loop over `[Node::Main, Node::If02]`
  with one more `JoinSet` task — the grid task — all feeding the same `tx`/`connection_tx`,
  per ticket 18 §1.
- The grid task: discovers interface 1 (analog stream) and interface 2 (control) hidraw nodes
  by walking `/sys/class/hidraw` (vendor `1532`/product `0244`/`bInterfaceNumber`), per ticket
  18 §2. `ENOENT`/`EACCES` on open join the absence-retry bucket (`POLL_INTERVAL`, silent,
  non-fatal). On successful open: read `Device::get_auto_repeat()` off the If01 evdev node
  (ticket 18 §4) before sending the unlock, cache `(delay_ms, period_ms)`; send the unlock via
  `HIDIOCSFEATURE` on the interface-2 fd; wait up to 500ms for report `0x06` (ticket 18 §3) —
  timeout means the fd is poisoned, close it, don't resend until the next reopen.
- Depth→`EventState` hysteresis and the repeat-timer scheduling are **pure functions/a small
  state machine**, not entangled with the I/O loop (ticket 18 §9) — e.g. something like
  `fn observe(prev: KeyState, depth: u8, point: ActuationPoint) -> (KeyState, Option<EventState>)`
  plus a repeat-timer type that, given `(delay_ms, period_ms)` and a hold start time, decides
  when the next synthesized `Repeat` is due. Write these first and unit-test them exhaustively
  (crossing up through actuation, crossing down through release, staying in the hysteresis
  band produces no transition, repeated Down-band dwelling produces repeats at the right
  cadence) before wiring them to real I/O.
- The grid task holds a `tokio::sync::watch::Receiver<HashMap<Input, ActuationPoint>>` and
  reads `.borrow()` on every report to threshold against current values (ticket 18 §5). The
  **publish** side (dispatch constructing and pushing snapshots into the paired `Sender`) is
  this ticket's job too, even though dispatch is otherwise ticket 23's territory — add it now
  in `dispatch.rs`'s existing `handle_command` arms for `SetActuationPoint`/
  `SetDefaultActuation`/`ResetActuationPoints`/`SwitchProfile`, since the watch channel and its
  publisher are one seam and splitting them across tickets would leave this ticket unable to
  test against real actuation data.
- Byte 1..20 of report `0x06` maps to `Input::Grid` in the same row-major order as
  `daemon/src/input.rs`'s `GRID_KEYS` table (ticket 16 confirmed `byte n == keycap n` per-key,
  out of reading order — this is now a fact, not a hypothesis to re-verify).

**`capture/fake.rs`**: extend `PhysicalEvent` scripting to carry `depth: Some(_)` for grid
Inputs where a test wants it — no raw-byte/report simulation needed, since report parsing and
the threshold state machine are separately, already unit-tested pure functions.

**Verification against the real device** (HITL — the device is a Razer Tartarus Pro,
`1532:0244`, already connected to this machine as of this ticket's charting session): confirm
the unlock succeeds and report `0x06` arrives, confirm all 20 keys threshold correctly at the
default 128/112 actuation/release points, confirm Hold-to-repeat synthesizes at a rate that
looks right against the cached kernel delay/period, confirm a permission-denied `hidraw` open
(temporarily `chmod` the node, or test before the udev rule exists) degrades silently rather
than erroring, and confirm Main/If02 events (Mode key, thumbstick, wheel) keep flowing
unaffected while the grid task runs alongside them.

## Answer

Landed per ticket 18's `## Answer`, no `main.rs` wiring (ticket 23's job, confirmed correctly
out of scope by code review). Daemon test count went from 161 to 163 (+21 net: +26 new in
`capture::analog`, +5 elsewhere, -10 filtered-out placeholder wasn't a real count — see per-file
breakdown below); `cargo build`/`cargo test`/`cargo clippy --all-targets`/`cargo fmt` all clean.

### `daemon/src/capture/evdev_source.rs`

Generalized per ticket 18 §1: `EvdevCaptureSource` now holds `nodes: &'static [Node]` (a
`pub const ALL` constant preserves the old all-three-nodes behavior for `main.rs`/Digital
Capture mode). The per-node open/grab/relay/retry loop split into `spawn_nodes` (spawns into a
caller-supplied `JoinSet`, at a caller-chosen base index into a caller-supplied presence `Vec`)
and `join_first` (awaits the first task to finish), both `pub(super)` so `capture::analog`
reuses them unchanged for `[Node::Main, Node::If02]` rather than duplicating the open/grab/
relay/`is_device_absent` logic. `is_device_absent` and `POLL_INTERVAL` also went `pub(super)`
(a code-review fix — see below) so the grid task's own absence check builds on them instead of
re-declaring the same `ENODEV` constant and retry interval a second time.

### `daemon/src/capture/analog.rs` (new)

The hidraw grid task, composed into `AnalogCaptureSource` as "one more `JoinSet` task" per
ticket 18 §1, exactly as specified:

- **Protocol bytes** ported byte-for-byte from `prototype/13-analog-grid-capture/prototype.py`
  — `hidiocsfeature()`'s `_IOC` encoding, `build_razer_cmd`'s CRC-XOR-over-`buf[3..89]`
  construction, `transaction_id 0x01`, the unlock/relock command bytes. A `hidiocsfeature_91_
  matches_the_documented_ioctl_number` test plus a handful of others mirror `prototype.py`'s own
  `selftest` checks (CRC excludes `transaction_id`, CRC range's far end, unlock/relock differ
  only in `arguments[0]`/CRC).
- **Discovery** walks `/sys/class/hidraw` on every (re)open (ticket 18 §2), matching vendor
  `1532`/product `0244`/`bInterfaceNumber`; made testable by taking the sysfs root as a
  parameter, with unit tests building a synthetic sysfs tree under `tempfile::tempdir()` (no
  real device needed to exercise the matching logic itself).
- **Lifecycle**: open both hidraw interfaces, cache `Device::get_auto_repeat()` off the If01
  evdev node before unlocking (ticket 18 §4), send the unlock, wait up to 500ms for report
  `0x06` (ticket 18 §3) via a real `poll()`+`read()` on the raw fd. Every absence condition —
  `ENOENT`/`EACCES` on open, `ENODEV`/`EIO` mid-stream, an unconfirmed unlock — joins the same
  silent `POLL_INTERVAL`-retry bucket (ticket 18 §7), now via one shared `retry_after_absence`
  closure instead of seven copies of the same two lines (a code-review simplification).
- **Pure functions, written and tested first** (ticket 18 §9): `observe(prev: KeyState, depth,
  point) -> (KeyState, Option<EventState>)` for the Depth→`EventState` hysteresis, and
  `RepeatSchedule::repeat_due(held_for, fired)` for synthesized Hold-to-repeat timing — both
  exhaustively unit-tested (crossing thresholds, hysteresis-band dwelling, repeat cadence over a
  synthetic timeline) with no channels/tokio/hardware involved, mirroring how `dispatch::fire`/
  `executor::compile` are already tested.
- **The watch channel**: the grid task holds a `watch::Receiver<HashMap<Input, ActuationPoint>>`
  and re-clones its `.borrow()` only when `has_changed()` (not on every single report — a
  code-review efficiency fix), threshold-checking each of the 20 depth bytes against it every
  report. `byte n` maps to `Input::Grid` via the same row-major layout `input.rs`'s `GRID_KEYS`
  uses (ticket 16's confirmed identity mapping).
- **`dispatch.rs`'s publish side** (this ticket's job too, per the spec): added to
  `SetActuationPoint`/`ClearActuationPoint`/`SetDefaultActuation`/`ResetActuationPoints`/
  `SwitchProfile`'s existing `handle_command` arms via a new `publish_actuation_snapshot`
  helper and `Profile::resolved_actuation_points()` (all 20 Grid keys, override-or-default
  resolved). Note: the ticket's prose names only four of these five arms —
  `ClearActuationPoint` is the same `actuation_overrides` mutation `SetActuationPoint` is, so
  leaving it unpublished would let the watch channel go stale after a clear; included it as an
  evident omission rather than a deviation from the decision itself.

### `daemon/src/capture/fake.rs`

No functional change needed — `PhysicalEvent.depth: Option<u8>` already exists from ticket 21,
so `FakeCaptureSource` already carries `depth: Some(_)` unchanged. Added
`scripts_analog_depth_on_grid_inputs_without_any_raw_byte_simulation` to make that explicit and
close the ticket's checklist item with a real test rather than an assertion in prose.

### `daemon/examples/analog_probe.rs` (new)

A manual HITL verification tool, not part of the daemon or wired into `main.rs` — runs the real
`AnalogCaptureSource` against the connected device and prints every `PhysicalEvent`/connection
transition for a human to eyeball against the ticket's own verification checklist (unlock
succeeds, all 20 keys threshold correctly, repeat cadence looks right, Main/If02 keep flowing,
permission-denied degrades silently). The user chose to run HITL verification themselves rather
than granting this session temporary hidraw access — the device's `hidraw` nodes have no udev
rule yet (ticket 23), so this needs `sudo` or a temporary `chmod` either way, plus a real
`set_device_mode` send carries the documented (if low-probability) USB-reset risk ticket 12 §5
flagged. `cargo run --example analog_probe [duration_seconds]`; prints a reminder to re-lock via
the already-verified `prototype/13-analog-grid-capture/prototype.py relock` afterward.

### Code review findings, real ones fixed

`/code-review` ran 8 finder angles across both axes. Two were genuine correctness bugs, both
fixed and covered by new tests:

- **`reject_release_above_actuation` accepted `release == actuation`.** Harmless while nothing
  consumed `ActuationPoint`s (ticket 21's scope), but `capture::analog::observe` now does: a key
  held at a perfectly steady Depth at exactly that value would cross both thresholds on every
  report, chattering Down/Up forever. Tightened `<=` to `<`; added
  `set_actuation_point_rejects_a_release_point_equal_to_actuation`.
- **`poll_readable` only checked `POLLIN`, not `POLLHUP`/`POLLERR`.** Once a peer closes and the
  buffer drains, `poll()` can report `POLLHUP` alone — treating that as "not readable" span the
  relay loop in a tight, immediately-returning busy-loop instead of ever calling `read()` to
  observe the EOF. Caught by this ticket's own new regression test
  (`a_dropped_connection_force_releases_every_key_still_tracked_as_held`), which hung for a
  full `RepeatSchedule` delay before the fix — a real bug the test found, not just a test-setup
  artifact. Fixed by also treating `POLLHUP`/`POLLERR` as readable, since a subsequent `read()`
  on either correctly returns `0`/an `Err`, which the caller already handles.

That same new test also covers a third finding — **held-key state resets to all-`Up` on every
`hidraw` reopen with no compensating release**, so a transient dropout while a key is physically
held would otherwise produce a stray, unpaired Down on reconnect (double-firing
FireOnce/HoldToRepeat, or silently stopping a running Toggle). Fixed: `relay_grid_blocking` now
force-releases every key it still has tracked as held before propagating any `Err` exit.

Findings considered and **not** acted on, with reasons:

- **`GetConfig()`'s wire dict never serializes `force_digital`/`default_actuation`/
  `actuation_overrides`.** Pre-existing, deliberate scope decision from ticket 21's own Answer
  ("Deliberately out of scope... whichever ticket wires the GUI's actuation-point editor will
  need to add that") — not something this ticket touches or was asked to fix.
- **`GetState()`'s positional D-Bus tuple keeps growing instead of switching to a keyed dict.**
  Pre-existing architecture from before ticket 21; this ticket didn't add anything to that
  tuple. A real observation, but a wire-protocol redesign is its own decision, not a ticket-22
  drive-by.
- **Five `dispatch.rs` command arms (`Set`/`ClearActuationPoint`, `SetDefaultActuation`,
  `ResetActuationPoints`, `SetForceDigital`) hand-roll the same persist/rollback/publish
  sequence.** That shape pre-dates this ticket (`SetBinding`/`CreateProfile`/etc. already
  follow it) — this ticket only added a `publish` step to arms that already existed. A generic
  `persist_or_rollback` helper is a real simplification opportunity, but restructuring ~9
  already-tested arms across the whole file is a `/simplify` pass, not this ticket's job.
- **A short/truncated report `0x06` is silently dropped, indistinguishable from "no report
  yet."** Consistent with how `evdev_source::normalize` and the whole absence-retry path already
  degrade silently with no logging framework in this codebase — not a new pattern introduced
  here, and ticket 13/16 already confirmed 24 bytes on the real hardware.
- **map.md's Decisions-so-far never got a bullet for ticket 21.** Real, and fixed — see below —
  but it's a documentation-process gap in a prior session's commit, not this ticket's code.

### HITL verification — not done this session

The device (`1532:0244`) is connected, but `/dev/hidraw0-2` are `root:root 0600` (no udev rule
yet). Asked the user how to handle the real-hardware step (grant temporary access and I run it,
or they run it themselves); they chose to verify it themselves, so `daemon/examples/
analog_probe.rs` above is what they'll use. None of the ticket's HITL checklist (unlock
succeeds, 20-key threshold accuracy, repeat cadence, Main/If02 unaffected, permission-denied
degrades silently) has been confirmed against the real device by this session.

Follow-up: [ticket 24](./24-task-verify-analog-capture-source-on-hardware.md) picks this
checklist back up as a cooperative session with the user, rather than leaving it to
`analog_probe` run solo.
