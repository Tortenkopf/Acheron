Type: grilling
Blocked by: 17
Status: resolved

## Question

Build the **analog `CaptureSource`** and the device-mode lifecycle around it, so the Daemon
captures the 20 grid keys from the analog stream and synthesises their press/release events
from depth thresholds. Non-blocking for v1.0 (see the map's Destination — the *model* is in
the required floor, this build is not).

No second prototype: [ticket 13](./13-task-standalone-analog-capture-prototype.md) already
answered "does this work," and what remains is integration behind an existing seam. Build
against the real device incrementally, per the map's execution discipline.

The seam is already there — `daemon/src/capture/mod.rs` defines `CaptureSource` as a trait
producing events into a channel, with `evdev_source` and `fake` as its implementations, so
this is a third implementation rather than surgery on `dispatch.rs`. The protocol is fully
specified in [the research](./12-research-linux-analog-grid-key-protocol.md) and confirmed
byte-for-byte by [the prototype](./13-task-standalone-analog-capture-prototype.md);
`prototype/13-analog-grid-capture/prototype.py` is the working reference implementation to
port.

Settle at least:

- **The hybrid source.** Driver mode silences only the grid keys (pending [ticket
  16](./16-task-analog-mode-hardware-facts.md)'s confirmation), so the Mode key, thumbstick
  and wheel still arrive over evdev. The analog source is therefore almost certainly
  *both* channels at once — `hidraw` for the grid, evdev for the rest — not a replacement
  for `evdev_source`. Decide whether that's one composite `CaptureSource` or two sources
  merged into the one channel.
- **Threshold → `EventState`.** Turning a depth ramp into `Down`/`Up` using the actuation
  and release points [ticket 17](./17-decide-analog-data-model.md) settles. Note there is
  no longer any device-generated `Repeat`: today's Hold-to-repeat rides the device's own
  evdev autorepeat (`dispatch.rs` fires on every `Repeat`), and driver mode emits no such
  thing. **Hold-to-repeat on a grid key will break unless the analog source synthesises
  the repeat itself** — including matching the kernel's delay/rate closely enough that
  existing Bindings behave the same. This is the sharpest regression risk in the ticket.
- **Mode lifecycle.** Unlock on start, re-lock to mode `0x00` on clean shutdown, and the
  automatic degradation path from the map's Notes: fall back to `evdev_source` if the udev
  rule is missing, the `hidraw` open fails, or the unlock is rejected. Plus the user-facing
  force-digital override. Use `transaction_id 0x01` and keep avoiding the sysfs shortcut,
  per ticket 12 §5.
- **`hidraw` node discovery.** Node numbers are not stable across boots or reconnects — the
  prototype walks `/sys/class/hidraw` to find interface 2 rather than hardcoding a path.
  Port that, and decide how it interacts with the existing hotplug/reconnect handling that
  `evdev_source` already does via `connection_tx`.
- **The udev rule**, and its consequence for `install.sh` — this is the privileged install
  step the map's Destination now acknowledges. Decide the rule's shape (which device, which
  group) and whether `install.sh` prompts, requires `sudo`, or degrades to digital and
  tells the user how to enable analog later.
- **Tests.** `fake.rs` is the scripted stand-in the existing 72 Daemon tests use; decide
  what it grows so thresholding and synthesised repeat are testable without hardware.

## Answer

Settled across ten decisions, grilled 2026-08-17 with the real device connected but nothing
built yet — this ticket's Question was a design decision that got conflated with "build it" in
the same breath when it was written; the build itself is too large for one session and is
split into three child task tickets below.

### 1. Composite shape

`evdev_source`'s per-node loop generalizes to take an explicit node subset. A new
`AnalogCaptureSource` reuses it unchanged for `Node::Main` (Mode key, thumbstick) and
`Node::If02` (wheel), and adds one more task to the same `JoinSet`/`presence`/`connection_tx`
bookkeeping: a hidraw-based grid task covering the 20 grid keys. Not a second parallel
`CaptureSource` — structurally "one more node."

### 2. `hidraw` discovery and reconnect

The grid task walks `/sys/class/hidraw` (vendor `1532`/product `0244`/`bInterfaceNumber`
match, per the prototype's `discover()`) on every (re)open — node numbers aren't stable.
Open failures (`ENOENT`/`EACCES`) join `evdev_source`'s existing device-absence bucket:
cheap, silent, retried every `POLL_INTERVAL` forever, never fatal, no bytes reach the
firmware. On every successful reopen the unlock is resent unconditionally (ticket 16: a
power cycle reverts the device to digital).

### 3. Unlock confirmation, timeout, resend cadence

After a successful `HIDIOCSFEATURE` unlock ioctl, wait up to 500ms for report `0x06`
(prototype observed ~3ms). No report in time means the fd is poisoned: close it, fall back to
digital for this attempt, and only resend unlock after the *next* full reopen cycle — never on
a tight timer. This bounds unlock sends to once per `POLL_INTERVAL` (2s) even under sustained
rejection, respecting ticket 12 §5's reset-risk caution around repeated `set_device_mode`.

### 4. Repeat timing

Read `Device::get_auto_repeat()` (`EVIOCGREP`, exposed by the `evdev` crate) off the If01
evdev node before each unlock — while it's still live in digital mode — and cache
`(delay_ms, period_ms)` for the grid task's synthesized-repeat timers. Matches the user's real
kernel autorepeat settings rather than hardcoding the 250ms/33ms default, so Hold-to-repeat on
a grid key behaves identically to the other 8 Inputs riding real kernel autorepeat.

### 5. Threshold access to `Config` without breaking single ownership

Ticket 17: "how the analog source synthesizes Down/Repeat/Up from depth thresholds... is
ticket 18's job" — so thresholding happens in the capture layer, which needs per-key
`ActuationPoint`s that live in `Config` (owned exclusively by the dispatch task, issue 07).
Dispatch publishes a derived, read-only snapshot (`HashMap<Input, ActuationPoint>` for the
active Profile, defaults resolved) into a `tokio::sync::watch` channel on every mutation that
touches it (`SetActuationPoint`/`SetDefaultActuation`/`ResetActuationPoints`/`SwitchProfile`).
The grid task holds a `watch::Receiver` and reads the latest snapshot on every threshold check
— a snapshot publish, not a second copy dispatch itself reads back, so `Config` stays
single-owner.

### 6. Live source swap

`SetForceDigital` and `CaptureModeChanged` (ticket 17) only mean something if flipping
`force_digital` while running actually swaps the live source. `main.rs` grows a small
supervisor loop owning *which* `CaptureSource` currently runs, able to relock/stop one
`JoinSet` and start the other without tearing down dispatch, injector, or the D-Bus server.
Retry-to-upgrade (digital → analog) happens only at three moments — Daemon startup, a genuine
device reconnect, and an explicit `SetForceDigital(false)` — never a background timer, since a
freshly-installed udev rule only takes effect on replug anyway.

### 7. Fatality taxonomy

Unchanged in spirit from today (issue 07/ticket 20's "any capture failure is fatal, absence is
the one exception"): dispatch or injector exiting is still fatal; a capture-side swap or
fallback is not; the grid task's hidraw opens join the existing absence bucket rather than
introducing a new fatal case.

### 8. udev rule and `install.sh`

Rule matches `idVendor==1532`, `idProduct==0244`, grants the `plugdev` group `MODE="0660"` on
both hidraw interfaces (interface 1 read-only for the stream, interface 2 read-write for the
unlock/relock ioctl). `install.sh` attempts a `sudo` copy + `udevadm control --reload-rules
--trigger`, catches failure, and prints manual instructions rather than aborting the install.

### 9. Test seam

Depth→`EventState` hysteresis and repeat-timer scheduling are pure, hardware-free
functions/state machines, unit-tested directly with no channels/tokio/hardware involved —
mirrors how `dispatch::fire`/`executor::compile` are already pure and separately tested.
`fake.rs` and the 72 existing Daemon tests only need `depth: Option<u8>` added to every
`PhysicalEvent` construction site (ticket 17's widened shape) — no raw-byte simulation needed,
since byte→depth parsing and the threshold/repeat state machine are separate pure functions.

### What this spawns

The build is too large for one session and splits into three sequential task tickets, each
scoped to fit a fresh agent's context budget:

- [Apply the analog data model to code](./21-task-apply-analog-data-model-to-code.md) — ticket
  17's decided shapes (`ActuationPoint`, `Config.force_digital`, `PhysicalEvent.depth`, five
  D-Bus methods) were never actually written into `config.rs`/`command.rs`/`dbus`; this ticket
  does that mechanically, verified by the existing test suite alone, no hardware needed.
- [Build the analog CaptureSource](./22-task-build-analog-capture-source.md) — the hidraw grid
  task itself: discovery, unlock/relock lifecycle, the watch-channel actuation snapshot, the
  pure threshold/repeat-timer functions and their unit tests, `fake.rs`'s `depth` extension.
  Needs the real device for verification.
- [Wire live source-swap, udev rule, and install.sh](./23-task-wire-analog-supervisor-and-install.md)
  — the `main.rs` supervisor restructuring, the udev rule file, `install.sh`'s privileged step,
  and end-to-end verification (power-cycle, reconnect, `force_digital` toggling, unclean-death
  recovery) against the real device.

No second prototype ticket: every open question above was an architecture/integration
decision resolvable by grilling against the existing code and ticket 12/13/16's already-proven
protocol facts, not a "how should it look/behave" question.

## Comments

**[Ticket 16](./16-task-analog-mode-hardware-facts.md) settled the facts this ticket was
waiting on** — three of its bullets can be tightened before the grilling starts:

- **The hybrid source is confirmed, not "almost certainly".** Driver mode silences the 20
  grid keys and nothing else; the Mode key, thumbstick ×4 and wheel ×3 keep emitting evdev
  normally while the analog stream runs.
- **The Hold-to-repeat regression is narrower than written here.** The device's own evdev
  autorepeat *does* still fire in driver mode — for the Mode key and thumbstick. It is lost
  for the 20 grid keys only, so the analog source must synthesise repeat for the grid while
  the other 8 Inputs keep riding the kernel's autorepeat unchanged. Two repeat sources have
  to coexist and behave identically, rather than one replacing the other wholesale.
- **Mode lifecycle: suspend needs no handling; re-enumeration does.** Driver mode survives
  suspend/resume with the `hidraw` fd still open and the stream simply resuming, but a power
  cycle restores digital mode. So the re-unlock hook belongs on the reconnect path
  (`connection_tx`) and nowhere else. An unclean death leaves the device in driver mode with
  20 dead grid keys until something re-locks it, and a re-lock from a fresh process that
  never sent the unlock does work — so a Daemon restart can recover the state, provided it
  re-locks or re-unlocks on start rather than assuming.

Also settled: `byte n = keycap n` is confirmed per-key out of order, so the port can rely on
the identity mapping.
