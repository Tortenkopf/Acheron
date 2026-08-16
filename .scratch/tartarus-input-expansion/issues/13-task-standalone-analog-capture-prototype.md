Type: task
Status: resolved
Blocked by: 12

## Question

Build a standalone Linux proof-of-concept — independent of Acheron's codebase, living under `prototype/analog-grid-capture/` rather than `daemon/`/`gui/` — that attempts to actually read live per-grid-key analog depth values off the real, connected Tartarus Pro via `hidraw`, using the concrete translation [the research ticket](./12-research-linux-analog-grid-key-protocol.md) produces.

This is a feasibility test, not a build task: the question it answers is "does this work at all on Linux, on our actual hardware," not "how should this integrate into Acheron." HITL — needs the physical device and a human watching results in real time (and the caution flagged in ticket 12 about a possible firmware reset/reconnect side effect from the unlock command).

Settle at least:

- Does sending the documented unlock (HID Feature Report) actually put the device into streaming mode, observably, on this hardware?
- Do the resulting Report `0x06` reads actually carry plausible per-key depth values that change as a grid key is pressed harder/softer — not just on/off?
- Any deviation from the documented byte layout/report IDs found on this specific unit/firmware revision.
- Whether the reported firmware-reset risk is reproduced.

## Answer

**It works, completely, on the first attempt — and the signal is better than the plan
predicted.** Ran against the real unit (serial `PM2443F36300141`, firmware `v1.2`) on
2026-08-16. Prototype: [`prototype/13-analog-grid-capture/prototype.py`](../../../prototype/13-analog-grid-capture/prototype.py).
Raw evidence: [`assets/13-unlocked.jsonl`](../assets/13-unlocked.jsonl) (6700 reports).

Every byte of [ticket 12's plan](./12-research-linux-analog-grid-key-protocol.md) held.
Nothing had to be re-derived, corrected, or worked around.

### 1. Does the unlock put the device into streaming mode? Yes, observably.

One `HIDIOCSFEATURE(91)` = `0xC05B4806` on the interface-2 `hidraw` node, with the exact
91-byte buffer from §2 (`transaction_id 0x01`, class `0x00`, cmd `0x04`, arg `0x03`, CRC
`0x05`). The all-zero standby report `0x06` arrived **3 ms later**, exactly as
`open-tartarus-driver` describes. One-shot: no heartbeat, no polling relationship.

### 2. Is it real analog, not on/off? Unambiguously.

6700 reports over 45 s, **all 20 keycaps** exercised, every one spanning `0x00`–`0xFF`
(one topped out at 246). **256 distinct depth values** observed across the device — the
full 8-bit range. A single press traces a smooth monotonic ramp
(`5 13 21 29 37 46 55 64 72 81 89 97 105 112 120 127 133 …`), not a step.

**New finding, not in the plan: the stream is event-driven, not polled.** Zero of the 6700
reports repeat the previous payload; the device goes completely silent between presses (62
gaps >100 ms, longest 7.8 s) and reports each change ~1 ms apart while a key is actually
moving. So the Daemon's cost is proportional to travel, not a constant ~1 kHz drain —
materially better news for an always-on daemon than a polled stream would have been.

### 3. Deviations from the documented layout? None.

All 6700 reports were exactly 24 bytes with report ID `0x06`; depths at bytes 1–20; trailing
bytes 21–23 zero in **every** report. First-touch order across the capture is perfectly
monotonic byte 01 → byte 20 (~1–4 s apart, one key at a time), consistent with the identity
mapping `byte n = keycap n` in reading order, keycap 20 being the thumb key
(`grid_r4c5`). Worth re-confirming per-key when the real feature is built, since it rests on
the keys having been pressed in layout order.

### 4. Is the firmware reset reproduced? No — not by either mode.

The device **never left the bus**. Same `hidraw` nodes (`hidraw1/2/3`), same HID IDs
(`000B/000C/000D`), same evdev nodes, same serial and firmware throughout. This held for
mode `0x03` *and* for the mode `0x00` re-lock that PR #2710's kernel-probe crash implicated.
`device_mode` read back `00 00` afterwards, and the re-lock genuinely worked (ordinary
keyboard reports resumed immediately).

One unit, one firmware, one attempt — so this doesn't refute PR #2710, but it does support
§5's `transaction_id` hypothesis: `0x01` behaved perfectly where OpenRazer's `0x1F`/`0xFF`
were reported to reset. **Prefer `0x01`, and keep avoiding the sysfs shortcut.**

### 5. §6 — driver mode *does* silence the grid keys. Confirmed, via a better channel.

The report-ID census on interface 1 answers this directly, and no evdev grab can distort it:

| Phase | Report `0x01` (ordinary keyboard) | Report `0x06` (analog) |
|---|---|---|
| Normal mode, keys pressed | **54** | 0 |
| Driver mode, keys pressed | **0** | 6700 |
| After re-lock, keys pressed | **38** | 1 (stream's final standby) |

The keyboard report doesn't merely get supplemented — it **stops**. Since evdev keycodes are
derived from those HID input reports, the grid keys necessarily go dark on
`/dev/input/event8` while driver mode is active.

So the consequential branch is the real one: **analog is a device-wide mode switch sitting
underneath Acheron's entire evdev capture path, not an additive second stream.** While it is
on, the Daemon would have to synthesise all 20 discrete keys from depth thresholds itself,
and every existing feature (Layers, Chords, Steppers, Trigger modes) would have to keep
working on top of that thresholded stream.

**Caveat on the direct observation.** The prototype also watches the three evdev nodes in-process
to see this first-hand, but `acheron-daemon` was running throughout (7 h uptime) and holds an
exclusive `EVIOCGRAB` on all three nodes (`daemon/src/capture/evdev_source.rs:111`), so it
starved those reads — the evdev counter read 0 in the *baseline* too, and is therefore not
evidence by itself. The census above is the evidence. A confirming run with the Daemon
stopped would make it first-hand; it is a formality given the census, not an open question.

### Consequences for v1.0

- **Feasibility is settled: the signal is fully reachable from Linux**, at full 8-bit
  resolution, with an event-driven stream and no reset. The "may be dropped if infeasible"
  caveat on the map's analog strand is discharged.
- **But the cost estimate went up, not down.** §6's expensive branch is the true one. Analog
  is not additive; it re-plumbs the capture path everything else stands on.
- The udev-rule/privileged-install consequence from §4.3 stands unchanged (`/dev/hidraw*` is
  root-only; the prototype needed `sudo`).

