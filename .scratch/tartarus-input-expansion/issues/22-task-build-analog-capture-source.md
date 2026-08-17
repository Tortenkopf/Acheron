Type: task
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

_(unresolved)_
