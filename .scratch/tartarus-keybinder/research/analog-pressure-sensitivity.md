# Research: Tartarus Pro analog/pressure-sensitive input

Ticket: [05-research-analog-pressure-sensitivity](../issues/05-research-analog-pressure-sensitivity.md)

> **Partly superseded (2026-08-16).** The protocol facts below still stand, but the Linux
> feasibility framing ("unverified on Linux", "nobody has done this", "extrapolation from a
> Windows-only reference") has been overtaken by
> [Linux `hidraw` implementation plan for the Tartarus Pro analog grid-key signal](../../tartarus-input-expansion/research/linux-analog-grid-key-protocol.md),
> which corroborates the protocol against our own hardware's HID descriptors, pins down the
> exact byte buffer and ioctls, finds Linux prior art on a sibling Razer analog device, and
> sharpens the firmware-reset risk into something first-party and testable. Read that file
> first; this one for background.

## Bottom line

Yes — the Tartarus Pro's 20 grid keys have a **genuine per-key analog signal**, not a
Synapse-side inference from timing/repeat frequency. It comes from real optical hardware
(Razer's "Analog Optical" switches, an IR-light-through-stem design), and the analog
depth values are sent over USB HID on a **separate, undocumented HID report** that OpenRazer
does not read and that is *not* visible on the three standard evdev nodes the daemon already
found. A community project (`open-tartarus-driver`) has reverse-engineered this report and
documented the protocol in detail — but **that project is Windows-only** (a Rust driver
combining user-mode HID with the third-party Interception kernel driver for D-pad/wheel
remapping), not a Linux tool. Nobody has verified this protocol against a real device from
Linux. The report structure itself is a standard HID Feature Report, which is platform-agnostic
in principle, so a Linux `hidraw` implementation *should* work the same way — but this is
extrapolation from a Windows-only reference, not a confirmed Linux result.

**Correction (post-resolution)**: this file originally stated `open-tartarus-driver` "reads the
analog data directly from Linux userspace via hidraw, bypassing both Synapse and OpenRazer
entirely." That was wrong — verified against the actual repo (README + `research.md`) after the
user questioned it. The project targets Windows 10/11 only. The protocol details below (report
IDs, byte layout, unlock command) are unaffected by this correction — they're still what that
project documented — but "accessible from Linux today" should be read as "the protocol should
be portable to Linux in principle," not "someone has done it."

## Findings

### 1. Razer's own claims for this specific device

Razer's product page for the Tartarus Pro states the device uses **Razer Analog Optical
Switches**, and explicitly ties this to depth sensing (not just marketing language reused from
the analog keyboards):

> "The Razer Tartarus Pro has Analog Optical Switches, which measure how far down you press.
> Razer Synapse then translates these measurements into analog input for games."
>
> "Supporting a range of actuation from 1.5 to 3.6 mm, customize the switches to be as
> sensitive as you want..."

— [Razer Tartarus Pro product page](https://www.razer.com/gaming-keypads/razer-tartarus-pro)
(supports: Razer itself claims real analog/depth sensing for this exact device, including
half-press/full-press dual-bind and thumbstick-like analog movement from the grid keys).

Razer's technology page for the switch itself confirms the underlying physical mechanism and
explicitly lists the Tartarus Pro among the (Gen-1) products that ship with it — alongside the
Huntsman V2 Analog / Huntsman Mini Analog keyboards the user was likely half-remembering:

> "With the ability to measure the exact amount of light that goes through the switch stem,
> Razer™ Analog Optical Switches Gen-2 can detect how far a key is pressed instead of being
> restricted to a traditional binary input."

Gen-1 list includes: Huntsman V2 Analog, Huntsman Mini Analog, **and Razer Tartarus Pro**.

— [Razer Analog Optical Switch tech page](https://www.razer.com/technology/razer-analog-optical-switch)
(supports: physical switch technology and confirmation the Tartarus Pro specifically uses it,
distinguishing it from — but confirming it's the same underlying tech family as — the Huntsman
analog keyboards).

**Conclusion on switch type**: analog optical, not mechanical and not Hall-effect. Physically
capable of continuous depth output (an IR beam through the switch stem, read by a sensor) —
this is not something Synapse could infer from a plain digital switch, so the "is it real or
faked by Synapse" question is resolved: it's real, hardware-level analog sensing.

### 2. OpenRazer project — driver and issue tracker

OpenRazer added basic Tartarus Pro support (matching the map's existing finding: lighting +
keymap only, no macro/remap DBus surface). Across the two PRs that implemented this
(`openrazer/openrazer` [#1577](https://github.com/openrazer/openrazer/pull/1577) and
[#2336](https://github.com/openrazer/openrazer/pull/2336), the latter eventually superseded by
a simplified merge), and the tracking issues
([#1039](https://github.com/openrazer/openrazer/issues/1039),
[#1177](https://github.com/openrazer/openrazer/issues/1177)), there is **no mention anywhere of
analog input, pressure sensitivity, or a depth-carrying HID report**. PR #2336's discussion
covers RGB effects, key-matrix layout translation, and macros only.

Community summaries note OpenRazer support was modeled on the (non-analog) Tartarus V2
codebase, and requests for "partial key press" support were declined/never implemented, i.e.
OpenRazer maintainers were aware analog existed but chose not to build it.

— [PR #1577](https://github.com/openrazer/openrazer/pull/1577),
[PR #2336](https://github.com/openrazer/openrazer/pull/2336),
[Issue #1039](https://github.com/openrazer/openrazer/issues/1039)
(supports: OpenRazer's driver — including `hardware/keyboards.py` — never parses or exposes
analog data for this device; corroborates the ADR-0002 finding that OpenRazer's Tartarus Pro
surface is lighting/keymap-only).

### 3. Community reverse-engineering of the raw protocol

A from-scratch community driver, **`ultramonaka/open-tartarus-driver`**
(https://github.com/ultramonaka/open-tartarus-driver), is a **Windows-only** (Windows 10/11)
Rust project — "a from-scratch Rust driver for the Razer Tartarus Pro... that runs on Windows
without Razer Synapse at all." Analog key handling is done in Windows user-mode via HID; D-pad/
wheel *remapping* additionally uses the third-party Interception kernel driver, needed to work
around Windows-specific input message-ordering constraints — that kernel-mode piece is about
suppressing/reinjecting Windows input events, not about reading the analog signal itself. Its
`research.md` documents the reverse-engineered protocol (found by capturing USB traffic around
Synapse's own startup):

- Device: VID `0x1532` / PID `0x0244` (matches the ID already recorded in the map).
- **Analog data stream**: HID Interface 1, endpoint `0x82`, Report ID `0x06`. `byte[1]..byte[20]`
  are per-key depth values, range `0x00`–`0xFF`, mapped 1:1 to the 20 keycap numbers (confirmed
  by pressing keys one at a time and watching which byte moved).
- **This stream is off by default.** It has to be unlocked by sending a **HID Feature Report**
  (a control-transfer report type, not a regular input/output report) to Interface 2 (endpoint
  `0x83`): 90 bytes, `command_id = 0x04` ("set device mode") at offset 7, mode argument `0x03`
  ("streaming/driver mode"; `0x00` reverts to normal) at offset 8, CRC = XOR of bytes `[2..88]`
  at offset 88. The doc doesn't name the exact Windows API used to send it (presumably
  `HidD_SetFeature` or equivalent) — on Linux the equivalent would be the `hidraw` `HIDIOCSFEATURE`
  ioctl (or a Rust `hidraw`/`hidapi` crate's `send_feature_report`) against the Interface-2 node.
  It's a **one-time toggle, not a polling relationship**: a few ms after sending it, Interface 1
  starts emitting a "standby" report (all-zero depths), then real analog reports on every
  keypress — and it keeps streaming even if the sending process exits.
- **Reported risk**: `research.md` notes a firmware reset/reconnect loop observed on some units
  after sending this command (cross-referencing OpenRazer issue/PR #2710), not reproduced by
  that project's own testing — possibly a firmware-revision-specific quirk. Worth verifying
  cautiously (not by default at every daemon startup) if this is ever picked up.

— [`open-tartarus-driver` README](https://github.com/ultramonaka/open-tartarus-driver) and
`research.md` (supports: a genuine, undocumented analog HID report exists on the wire,
independent of OpenRazer/Synapse; the report/command structure is standard cross-platform HID,
so it should be portable to Linux `hidraw` in principle — but this project itself never runs on
Linux, so that portability is unverified).

Caveat: this is a single community reverse-engineering project, not vendor documentation or a
peer-reviewed spec, and it targets Windows only. The byte offsets/command bytes above are as
documented by that project and not independently re-verified against this repo's own USB
capture, nor against Linux `hidraw` at all — treat as credible but doubly unverified (unverified
protocol details, and unverified on this OS) until confirmed with our own capture if this is
ever picked up.

### 4. Switch type recap

Analog **optical**, not mechanical, not Hall-effect (Hall-effect is a different, magnet-based
analog approach used by some competitors). Confirmed by Razer's own tech page (finding #1). This
matches physically with why the three evdev nodes already captured show nothing but discrete
keycodes: evdev only ever carries the digitized up/down HID usage the firmware chooses to also
emit on the standard keyboard interface — the analog depth lives on a separate report the kernel's
generic HID/input driver doesn't map to any evdev event at all, so it's invisible to `evtest`/
`libinput` no matter how carefully you look at the three already-enumerated nodes.

## Why this changes (or doesn't change) the map

The signal is real and *should* be reachable from Linux in principle, but:

- The only documented reference implementation is Windows-only; nobody has verified this
  protocol against a real device from Linux `hidraw`. "Reachable from Linux" is currently a
  reasonable inference from a platform-agnostic HID report structure, not a demonstrated fact.
- It requires bypassing the device's default protocol with an **undocumented, unverified
  vendor command** (not something Razer or OpenRazer publish or support), with at least one
  report of a firmware reset/reconnect side effect on some units.
- It requires raw `hidraw` access and hand-rolled report parsing — a materially different and
  riskier capture path than the evdev+uinput approach ADR-0002 already committed to.
- It has zero bearing on the MVP's actual use case (keybinding/macro remap for a grid of
  discrete game-action keys) — analog-as-thumbstick emulation is a distinct feature with its
  own UI/config surface (dual-zone bindings, actuation curves, etc.), not a small extension of
  discrete Bindings.

Given that, this is being recorded as **out of scope** rather than a "not yet specified"
opportunity: it's not that the cost is unknown, it's that the cost (raw HID reverse-engineering
of an unverified vendor command, a second capture pipeline alongside evdev, and a new Binding
data-model concept for continuous input) clearly outweighs the MVP's discrete-remap goal. If
priorities change later, `open-tartarus-driver`'s `research.md` is a concrete, reusable starting
point for a future analog-aware effort — worth a footnote so nobody has to rediscover this.

## Sources

- [Razer Tartarus Pro product page](https://www.razer.com/gaming-keypads/razer-tartarus-pro) — Razer's own analog/depth-sensing claim for this device.
- [Razer Analog Optical Switch tech page](https://www.razer.com/technology/razer-analog-optical-switch) — switch physics, confirms Tartarus Pro uses Gen-1 analog optical switches.
- [OpenRazer PR #1577](https://github.com/openrazer/openrazer/pull/1577) — Tartarus Pro driver addition, no analog handling.
- [OpenRazer PR #2336](https://github.com/openrazer/openrazer/pull/2336) — Tartarus Pro driver addition (superseding), no analog handling; discussion confined to lighting/macros/matrix.
- [OpenRazer Issue #1039](https://github.com/openrazer/openrazer/issues/1039) — original Tartarus Pro support request, no analog mention.
- [`ultramonaka/open-tartarus-driver`](https://github.com/ultramonaka/open-tartarus-driver) (README + `research.md`) — community reverse-engineered raw-HID protocol for the analog depth stream; a working **Windows-only** driver independent of Synapse/OpenRazer, not a Linux implementation.
