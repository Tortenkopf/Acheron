Type: grilling
Blocked by: 17

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

_(unresolved)_

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
