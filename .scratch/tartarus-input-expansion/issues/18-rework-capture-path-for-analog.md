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
