# Research: how to drive the three side "Status LEDs" on the Razer Tartarus Pro

Status: **standalone research, no effort map yet.** Captured 2026-08-30 so it's on hand once a
post-release-development map exists. No ticket feeds this yet.

Scope note: these are the **three discrete single-colour LEDs on the side of the device** (Razer
calls them the *keymap indicator*; Synapse uses them as a profile-active indicator). This is **not**
the per-key multicolour "Chroma" backlight, which Acheron already ignores as out of scope. Lighting
in general is out of scope for v1.0 — this is filed as a "someday, if we ever want it" curiosity,
notably because the common wisdom is that no open reimplementation (OpenRazer,
ultramonaka/open-tartarus-driver) has cracked it.

## Bottom line

The common wisdom is **wrong** — it's solved, twice, just not in a place people look:

- The classic single-LED command OpenRazer uses for the Orbweaver / original Tartarus / Tartarus V2
  (`command_class 0x03`, LED IDs `0x0C/0x0D/0x0E`) **does nothing on a Tartarus Pro**. The device
  ACKs it and ignores it. This is the path everyone tried and gave up on.
- The Pro instead exposes the three LEDs through its **extended-matrix effect command** — the same
  `command_class 0x0F` family as the per-key backlight — via a **dedicated LED ID `0x0B`**
  ("side stripe" LED), using a **static effect** whose R/G/B argument bytes act as three
  independent on/off (really brightness) channels, one per physical LED (red/orange, green, blue).
  All three light independently and simultaneously.
- **OpenRazer PR #2336** (`plxty`, Feb–May 2025) implements exactly this as a real driver patch
  (`SIDE_STRIPE_LED 0x0B`, `profile_led_red/green/blue` sysfs). Maintainer `z3ntu` reviewed it and
  the LED behaviour was confirmed on real hardware. The PR was **closed unmerged** in favour of a
  reduced-scope Tartarus Pro PR.
- **CommandPost** (a macOS automation app) ships an independent implementation of the same command
  in `HSRazerTartarusProDevice.m` (`:orangeStatusLight()` / `:greenStatusLight()` /
  `:blueStatusLight()`).
- The **merged** OpenRazer Tartarus Pro support (PR #2710) **deliberately omits** the status LEDs
  ("Currently missing is support for the profile LEDs, this will be added later"). So OpenRazer's
  lack of support is a scoping decision, not an unsolved problem.
- ultramonaka/open-tartarus-driver's `research.md` lists the status LEDs as an open question — but
  it only ever tried the `0x03` path and never tried LED ID `0x0B`.

## 1. Device identity (authoritative — OpenRazer `driver/razerkbd_driver.h`)

| Device                | PID      | transaction_id | report/response wIndex |
|-----------------------|----------|----------------|------------------------|
| Razer Tartarus (orig) | `0x0201` | `0xFF`         | `0x02`                 |
| Razer Tartarus Chroma | `0x0208` | `0xFF`         | `0x02`                 |
| Razer Tartarus V2     | `0x022B` | `0x1F`         | `0x02`                 |
| **Razer Tartarus Pro**| `0x0244` | `0x1F`         | `0x02`                 |

VID `0x1532`. Model no. RZ07-03110 / RZ03-03050. (Cross-check against Acheron's own device tables —
Acheron already binds this PID.)

## 2. The Razer 90-byte report on the wire (all Tartarus models)

`struct razer_report` (`driver/razercommon.h`), sent as a HID **feature report** over a USB control
transfer: `bmRequestType 0x21` (SET) / `0xA1` (GET), `bRequest 0x09` / `0x01`, `wValue 0x0300`,
`wIndex 0x02` (Tartarus Pro).

| Byte      | Field                    | Tartarus Pro value        |
|-----------|--------------------------|---------------------------|
| `[0]`     | status                   | `0x00`                    |
| `[1]`     | transaction_id           | `0x1F`                    |
| `[2:4]`   | remaining_packets (BE16) | `0x0000`                  |
| `[4]`     | protocol_type            | `0x00`                    |
| `[5]`     | data_size                | # of argument bytes       |
| `[6]`     | command_class            | `0x0F`                    |
| `[7]`     | command_id               | `0x02` set / `0x82` get   |
| `[8..87]` | arguments[80]            | see §3                    |
| `[88]`    | crc                      | XOR of bytes `[2]..[87]`  |
| `[89]`    | reserved                 | `0x00`                    |

CRC: `for (i = 2; i < 88; i++) crc ^= report[i];` (`razer_calculate_crc`, `driver/razercommon.c`).

## 3. The status-LED command — exact bytes (LED ID `0x0B`)

`command_class 0x0F`, `command_id 0x02` (write) / `0x82` (read), `data_size 0x09`,
`transaction_id 0x1F`:

| arg | value                    | meaning                                             |
|-----|--------------------------|-----------------------------------------------------|
| 0   | `0x01`                   | VARSTORE (persists in onboard memory)               |
| 1   | `0x0B`                   | LED ID — side status LEDs                            |
| 2   | `0x01`                   | effect = static                                     |
| 3   | `0x00`                   | —                                                   |
| 4   | `0x01` or `0x00`         | CommandPost sends `0x01`; OpenRazer helper `0x00`   |
| 5   | `0x01`                   | —                                                   |
| 6   | `0x00` / `0xFF`          | red/orange LED (0 = off, non-zero = on/brightness)  |
| 7   | `0x00` / `0xFF`          | green LED                                           |
| 8   | `0x00` / `0xFF`          | blue LED                                            |

Read-back (`command_id 0x82`, same args): current channel values come back in `arg6/arg7/arg8`
(`0xFF` = on for CommandPost; OpenRazer's sysfs clamps writes to `0x01`).

**One packet drives all three LEDs.** To change one without disturbing the others you must either
GET first and copy back the other two channels, or keep the RGB triple in your own state. Both
reference implementations keep their own cached `led_state` / dictionary and re-send the whole
static frame on every change.

Turning them off: same packet, channel byte(s) = `0x00`. An `effect_none` (effect id `0x00`,
`data_size 0x06`) on LED `0x0B` is plausible but untested.

Full example frame — light the **green** LED only:

```
[0]  00            status
[1]  1F            transaction_id
[2]  00 00         remaining_packets
[4]  00            protocol_type
[5]  09            data_size
[6]  0F            command_class
[7]  02            command_id   (82 to read back)
[8]  01            arg0  VARSTORE
[9]  0B            arg1  LED ID (side status LEDs)
[10] 01            arg2  effect = static
[11] 00            arg3
[12] 01            arg4  (CommandPost value; OpenRazer uses 00)
[13] 01            arg5
[14] 00            arg6  red/orange   (00 off / FF on)
[15] FF            arg7  green
[16] 00            arg8  blue
[17..87] 00        padding
[88] <crc>         XOR of [2..87]
[89] 00            reserved
```

Enable driver/streaming mode first (`command_class 0x00, command_id 0x04, arg0 = 0x03`), the same
as for the backlight — **but** see the PR #2710 warning in §6.

## 4. The classic `0x03` path — for contrast (this is what does NOT work on the Pro)

`driver/razercommon.h`: `RED_PROFILE_LED 0x0C`, `GREEN_PROFILE_LED 0x0D`, `BLUE_PROFILE_LED 0x0E`.
`razer_chroma_standard_set_led_state(varstore, led_id, state)` = `get_razer_report(0x03, 0x00,
0x03)`, `arguments = [varstore, led_id, state]`; GET = `0x03, 0x80, 0x03`.

OpenRazer master exposes `profile_led_red/green/blue` for Nostromo, Orbweaver (+Chroma), Tartarus,
Tartarus Chroma, and Tartarus V2 this way (`transaction_id 0xFF` for the older ones, red/green
channel swap on the Chroma variants). **Tartarus Pro is in none of those switch statements** and
gets no `profile_led_*` sysfs files. The open-tartarus-driver author tried this path on real Pro
hardware with both `0xFF` and `0x1F` transaction IDs: device ACKed, nothing lit.

## 5. Hardware-automatic vs host-driven

- **Host-addressable: definitively yes.** `plxty` set the three LEDs independently; `z3ntu`
  summarised it and `plxty` confirmed — "depending on the R, G and B values in this call the 3 LEDs
  just change brightness … the colour is fixed for them (red, green, blue)". CommandPost ships it.
- **Persistence:** the packet uses `VARSTORE (0x01)`, so state is written to onboard memory and
  survives reconnect.
- **Firmware's own use:** the Razer Tartarus Pro manual describes a *keymap indicator* that shows
  which of up to 8 key-assignment sets is active, "each represented by a colour". 8 states from 3
  binary LEDs ⇒ the firmware almost certainly drives these same LEDs as a **3-bit binary code** of
  the active on-device keymap, on keymap-switch (which is handled onboard).
- **Synapse's role:** a PR #2336 commenter notes Synapse writes these explicitly on each profile
  switch and the colours aren't customisable there.

**Not settled by any source:** whether an on-device keymap switch makes the firmware *re-assert*
its own value and clobber a host-set state. If it does, any host driver that wants to own these
LEDs must re-assert after each keymap change (hook the keymap-change event, or poll).

No source implements Razer onboard-profile commands (`command_class 0x05`) for the Tartarus Pro;
in OpenRazer that class is mouse-only and the keyboard driver has no `set_active_profile`.

## 6. Distinctions confirmed

- **Not part of the per-key matrix frame.** The Pro backlight is a 1×21 linear matrix uploaded via
  `command_class 0x0F, command_id 0x03`. The status LEDs are `command_id 0x02` (LED-effect),
  effect = static, LED ID `0x0B`. Same class and `transaction_id`, different `command_id` and LED
  ID. Backlight effect commands use LED ID `0x00` / `BACKLIGHT_LED 0x05`; the status LEDs are their
  own LED ID.
- **Driver-mode caution (PR #2710):** driver mode (`0x00 0x04`, mode `0x03`) can throw *some*
  Tartarus Pro units into a reset loop, so OpenRazer disabled driver mode for the Pro entirely.
  The open-tartarus-driver author has never reproduced this and uses mode `0x03` fine — likely a
  firmware-revision dependence. Acheron should test carefully on its own unit before relying on it.

## 7. Open questions / experiments to run if this is ever picked up

1. **Does firmware overwrite host-set LEDs on an on-device keymap switch?** Set all three via §3,
   press the keypad's keymap-switch combo, then poll (`0x0F/0x82`, LED `0x0B`) and watch visually.
   If they snap to a binary-coded value, a host driver must re-assert per switch.
2. **`arg3`/`arg4` semantics** — OpenRazer sends `00 00`, CommandPost sends `00 01`. Try
   CommandPost's values first (shipped, keypad-specific).
3. Other effect ids on LED `0x0B` (`0x00` none, `0x02` breathing) — untested.
4. **A USB capture of Synapse switching profiles on a real Tartarus Pro** would settle 1–2
   definitively. No such public capture was found. open-tartarus-driver's `research.md` is the only
   public Tartarus Pro USB-capture write-up and it never tried LED `0x0B`.

## 8. Sources

Primary (source code / patches):

- OpenRazer PR #2336 — the status-LED implementation + maintainer discussion:
  https://github.com/openrazer/openrazer/pull/2336
  (raw diff: https://patch-diff.githubusercontent.com/raw/openrazer/openrazer/pull/2336.diff)
- OpenRazer PR #2710 — merged, reduced-scope Tartarus Pro support; profile LEDs deferred:
  https://github.com/openrazer/openrazer/pull/2710
- OpenRazer PR #1577 — earliest attempt, "Profile LEDs are now set together as one RGB led (0xb)":
  https://github.com/openrazer/openrazer/pull/1577
- OpenRazer master driver: `driver/razerkbd_driver.{c,h}`, `driver/razerchromacommon.{c,h}`,
  `driver/razercommon.{c,h}` — https://github.com/openrazer/openrazer/tree/master/driver
- CommandPost Tartarus Pro status LEDs (shipped, independently derived):
  https://github.com/CommandPost/CommandPost-App/blob/master/extensions/razer/HSRazerTartarusProDevice.m
- CommandPost/Hammerspoon Tartarus V2 (classic `0x03` path with `0x1F`):
  https://github.com/Hammerspoon/hammerspoon/blob/master/extensions/razer/HSRazerTartarusV2Device.m
- open-tartarus-driver protocol write-up (analog + backlight solved, status LEDs left open):
  https://github.com/ultramonaka/open-tartarus-driver/blob/main/research.md

Secondary / leads:

- OpenRGB new-device issue for Tartarus Pro (open, needs captures):
  https://gitlab.com/CalcProgrammer1/OpenRGB/-/issues/2317 ; Razer controller code under
  `Controllers/RazerController/`
- OpenRazer tracking issues: #1039, #1177, #2475, #2514 (all Tartarus Pro)
- Razer Tartarus Pro manual (keymap indicator = 8 colours across 3 LEDs):
  https://manuals.plus/razer/razer-tartarus-pro-manual-and-faq
