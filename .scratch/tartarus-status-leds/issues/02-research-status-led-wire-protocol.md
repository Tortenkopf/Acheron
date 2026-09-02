Type: research
Blocked by: —
Status: resolved

<!-- Resolved 2026-09-02 by a background /research agent. Full write-up:
     ../research/status-led-wire-protocol.md (new file; supersedes §3 of
     tartarus-pro-status-leds.md, which now carries a pointer note). -->


## Question

Sharpen [`research/tartarus-pro-status-leds.md`](../research/tartarus-pro-status-leds.md) from
"this is how it's done twice elsewhere" into an **implementation-ready wire specification** for
the Status-LED frame — the remaining byte-level ambiguities the spec (ticket 05) will need
pinned, and that [ticket 01's prototype](./01-prototype-status-led-controllability.md) should
try first. Runs in parallel with ticket 01; both feed the spec.

Read the actual source (not just prose) of the two independent implementations and settle:

- **`arg3` / `arg4` semantics.** OpenRazer PR #2336's helper sends `00 00`; CommandPost's
  `HSRazerTartarusProDevice.m` sends `00 01`. What do these bytes mean in the `0x0F/0x02`
  LED-effect command family (cross-reference `razerchromacommon.c`'s other `0x0F/0x02`
  callers)? Which should Acheron send?
- **The "off" frame.** Is a static effect with all channel bytes `0x00` the right way to turn
  the LEDs off, or is `effect_none` (`effect` id `0x00`, `data_size 0x06`) on LED `0x0B` the
  correct call? Does either reference implementation ever send `effect_none` here?
- **Read-back frame shape.** Exact `command_id 0x82` request bytes and where the three channel
  values land in the response (research §3 says args 6/7/8; confirm against source).
- **`data_size` / argument count.** Research §3 says `data_size 0x09` (9 argument bytes).
  Confirm against both implementations and against the `struct razer_report` layout.
- **Driver-mode dependency.** Do either of the reference implementations send the LED frame
  *without* first enabling driver/streaming mode (`command_class 0x00, command_id 0x04,
  arg 0x03`)? (Acheron's daemon already enters it, but the spec should record whether the LED
  frame strictly needs it.)
- **VARSTORE vs NOSTORE.** Research §3 arg0 = `0x01` (VARSTORE, persists to onboard memory).
  Is there a NOSTORE (`0x00`) variant, and does either implementation use it? Weigh whether
  Acheron wants persistence (a flash of stale state on next boot before the daemon asserts) or
  not.

Primary sources (from research §8): OpenRazer PR #2336 diff, PR #1577, `razerchromacommon.{c,h}`
/ `razercommon.{c,h}` on `master`, CommandPost `HSRazerTartarusProDevice.m`.

Does not touch Acheron's code. Output: append to / supersede
[`research/tartarus-pro-status-leds.md`](../research/tartarus-pro-status-leds.md) (writer's
judgment), then an `## Answer` with the settled frame.

## Answer

**Implementation-ready.** Full write-up with primary-source citations (PR #2336 raw diff +
its z3ntu↔plxty review thread, OpenRazer `master` `razerchromacommon.{c,h}` /
`razerkbd_driver.c` / daemon `keyboards.py`, PR #1577 diff, CommandPost
`HSRazerTartarusProDevice.m` / `HSRazerDevice.m`):
[`../research/status-led-wire-protocol.md`](../research/status-led-wire-protocol.md). §3 of
the grounding file is superseded by it (pointer note added there); §8 of the write-up carries
byte-by-byte tables for the write / off / read-back frames with worked CRCs.

The frame is a standard extended-matrix static-effect command — exactly
`razer_chroma_extended_matrix_effect_static(VARSTORE, 0x0B, rgb)`, `transaction_id 0x1F`,
`data_size 0x09`, and expressible through Acheron's existing
`analog.rs::build_razer_cmd(0x1F, 0x0F, 0x02, &[0x01,0x0B,0x01,0x00,0x00,0x01, r,g,b])` with
**no helper changes**. Arg6/7/8 = red/green/blue, one fixed-colour LED each; z3ntu and plxty
confirmed the on-hardware behaviour in review (`00 FF 00` ⇒ green only).

The six questions:

1. **`arg3`/`arg4`.** Effect-specific sub-param slots (direction / speed) that the *static*
   effect ignores — Razer's own captured Synapse frame is `01 05 01 00 00 01 ff 00 00`.
   **Send `arg3=0x00, arg4=0x00, arg5=0x01`** (arg5 is the colour-count byte, always `0x01`
   here). CommandPost's `arg4=0x01` lands in the ignored speed slot — keep only as a fallback
   if a real unit doesn't react to `arg4=0x00`.
2. **Off frame.** Static effect with the channel byte(s) `0x00` (all-off args
   `01 0B 01 00 00 01 00 00 00`, crc `0x0E`). **Neither reference impl ever sends
   `effect_none` on `0x0B`** — its effect on a fixed-colour indicator LED is undefined. Don't
   use it.
3. **Read-back (`0x82`).** Request is byte-identical to the write frame bar `command_id` and
   zeroed RGB; channels return in `arguments[6/7/8]` (Acheron resp buffer `[15]/[16]/[17]`).
   z3ntu rejected the GET call as unreliable *across devices* ("we can never reliably get back
   what we've set earlier") — the specific thing that kept #2336 from merging. **Acheron owns
   an authoritative RGB triple in daemon state and re-sends the whole frame per change** (both
   reference impls do this; and the daemon must write unconditionally on every connect anyway).
   *Hardware update — ticket 01: `0x82` was in fact reliable on fw v1.2, including cold reads
   after a replug, so a startup seed is trustworthy on this unit.*
4. **`data_size`.** `0x09` — confirmed in OpenRazer, CommandPost, and against `struct
   razer_report` (CRC over bytes `[2..87]`).
5. **Driver mode: not required.** CommandPost sends no device-mode command at all; OpenRazer
   sets `DRIVER_MODE = False` for the Tartarus Pro *alone* and its `0x0F` lighting still
   works. **The LED frame is independent of Acheron's Capture mode** — send it on a
   short-lived Interface-2 fd like `read_device_info()` / `relock()`, regardless of whether
   the grid task has the device unlocked. (Grounding §3's "enable driver mode first" is
   **wrong**.)
6. **VARSTORE vs NOSTORE.** Both impls use VARSTORE (`arg0=0x01`); no NOSTORE variant of
   `0x0F/0x02` exists in the OpenRazer tree. Source analysis predicted VARSTORE would persist
   and flash stale state on boot — *hardware update — ticket 01 disproved this on fw v1.2:*
   **neither VARSTORE nor NOSTORE survives a USB re-enumeration** (firmware reclaims the LEDs
   to its orange-only power-on default), and a settled read-back always echoes `arg0=0x01`, so
   the byte is inert. **Send `arg0=0x00` for intent-clarity; no config knob.** The real
   requirement this surfaces: **the daemon must assert Status-LED state on startup *and* every
   device reconnect** (feeds [ticket 03](./03-daemon-architecture-for-status-leds.md)).

**Corrections to the grounding file's §3 prose** (arg *table* was correct): "enable driver
mode first" — wrong (Q5); clean GET-modify-SET read-back — unreliable (Q3); `effect_none` —
used by neither impl (Q2); PR #1577 "cited as an implementation" — its diff actually contains
**no `0x0B` code** (treats the Pro as a backlight-only Tartarus V2); **PR #2336 is the sole
real implementation of the `0x0B` path and it is unmerged**. The merged PR #2710 confirmed to
create no `profile_led_*` files for the Pro — status LEDs are unimplemented in any shipping
OpenRazer release.

### Hardware update — [ticket 01](./01-prototype-status-led-controllability.md) (fw v1.2, 2026-09-02)

The prototype exercised **every byte of the source-derived frame against the real device —
they agree**. The write frame (`arg4=0x00`), the off frame (static-zero; `effect_none` ACKs
but does nothing), the read-back slots, `data_size 0x09`, and "no driver mode needed" all
verified. Corrections to this write-up's *predictions* (source analysis stands, the
hardware-behaviour guesses in §3.3 / §6 / §9.3 / §9.6 did not):

- **Nothing persists.** Neither VARSTORE nor NOSTORE survives a re-enumeration; the firmware
  always returns to orange-only. `arg0` is inert (read-back echoes `0x01` regardless). → send
  `arg0=0x00`, drop the config knob, and **the daemon must re-assert on startup + every
  reconnect** (hard requirement, → ticket 03).
- **`0x82` read-back is reliable on this unit** (incl. cold-read seed case) — the "unreliable"
  caution is a cross-device one; keep the authoritative-triple design but a startup seed is
  trustworthy here.
- **No on-device keymap switch exists** on the Tartarus Pro (LED↔keymap link is Synapse-side
  only) → the "re-assert after on-device keymap change" hook is **not needed**.

The write-up (`status-led-wire-protocol.md` §3.3, §6, §7, §9) has been annotated inline with
these. **Ticket 01 unblocks tickets 03 and 04.**
