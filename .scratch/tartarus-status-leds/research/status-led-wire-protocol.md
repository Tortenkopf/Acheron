# Research: Tartarus Pro status-LED frame — implementation-ready wire protocol

Ticket: [02-research-status-led-wire-protocol](../issues/02-research-status-led-wire-protocol.md)

Sharpens **§3 ("The status-LED command — exact bytes")** of
[`tartarus-pro-status-leds.md`](./tartarus-pro-status-leds.md) from "this is how it's done
twice elsewhere" into a byte-by-byte frame spec. That file's §1–§2 and §4–§8 still stand;
its §3 is **superseded by this file** (a pointer note has been added there). Everything below
is read straight from primary source — the OpenRazer PR #2336 raw diff and its review
thread, OpenRazer `master` driver source, CommandPost `.m` source — with file/line or
PR-comment citations.

## Bottom line

- The frame is a **standard extended-matrix static-effect command** (`command_class 0x0F`,
  `command_id 0x02`, effect id `0x01`), aimed at a **dedicated LED id `0x0B`**
  (`SIDE_STRIPE_LED`), `transaction_id 0x1F`, `data_size 0x09`. Arg bytes 6/7/8 are the
  red / green / blue channels — one physical single-colour LED each, independently
  addressable. This is exactly what `razer_chroma_extended_matrix_effect_static(VARSTORE,
  0x0B, rgb)` emits, and the OpenRazer maintainer explicitly said so in review.
- **The two reference impls disagree on exactly one byte** — `arg4` (OpenRazer `0x00`,
  CommandPost `0x01`) — and that byte is the "speed" slot that the static effect ignores.
  Send `0x00` (matches Razer's own captured Synapse frames); CommandPost's `0x01` is a
  documented fallback to try if the LEDs don't respond.
- **The grounding file's §3 arg table is correct** (arg3 `0x00`, arg4 `0x00`/`0x01`, arg5
  `0x01`); two of its *prose* claims need correcting (details below in §7):
  (a) "enable driver/streaming mode first" is **wrong** — the frame needs no driver mode and
  OpenRazer drives this device's lighting with driver mode permanently disabled; (b) read-back
  is unreliable *across devices* per the OpenRazer maintainer (it's why #2336 stalled), so
  the design keeps an authoritative RGB triple in the daemon — **though on our fw v1.2
  [ticket 01](../issues/01-prototype-status-led-controllability.md) found `0x82` reliable in
  practice**. It also leaves two things open that this file settles: send `arg4 = 0x00` (not
  CommandPost's `0x01`), and `effect_none` is used by neither impl.
- **Hardware caveat (ticket 01, fw v1.2):** the source analysis of storage-mode behaviour in
  §6 was wrong — **nothing persists** across a USB re-enumeration (VARSTORE or NOSTORE), the
  firmware always reclaims the LEDs to its orange-only default, and `arg0` is inert. Net
  effect on the spec: send `arg0 = 0x00`, no config knob, and **the daemon must re-assert
  Status-LED state on startup and every reconnect** (not merely a boot-flash optimisation).

---

## 1. The write frame — `command_id 0x02`

### 1.1 What OpenRazer PR #2336 sends

`daemon/openrazer_daemon` → sysfs `profile_led_{red,green,blue}` → kernel
`razer_attr_write_profile_led_red/green/blue`, new `case USB_DEVICE_ID_RAZER_TARTARUS_PRO`
(PR #2336 diff, `driver/razerkbd_driver.c`, hunks at old lines 1544 / 1576 / 1607):

```c
case USB_DEVICE_ID_RAZER_TARTARUS_PRO:
    device->led_state.r = enabled;          // .g / .b for the other two
    request = razer_chroma_extended_matrix_effect_static(VARSTORE, SIDE_STRIPE_LED, &device->led_state);
    request.transaction_id.id = 0x1F;
    break;
```

- `SIDE_STRIPE_LED 0x0B` — new `#define` added next to `RED_PROFILE_LED` in
  `driver/razercommon.h` (PR #2336 diff). z3ntu, review comment on `razerchromacommon.c`
  (2024-12-09): *"…which apparently is called "Side Stripe LED", so make a new define in
  driver/razercommon.h around `RED_PROFILE_LED` for `#define SIDE_STRIPE_LED 0x0B`"*.
- `struct razer_rgb led_state` is a new per-device cached field
  (`driver/razerkbd_driver.h`, PR #2336 diff: *"For devices that have multiple LED states,
  like Tartarus"*). Each single-channel sysfs write updates one component of the cache and
  **re-sends the whole static frame**, because the other two channels can't be read back
  reliably (see §3).

`razer_chroma_extended_matrix_effect_static` (`driver/razerchromacommon.c:511`, unchanged
on `master`):

```c
struct razer_report razer_chroma_extended_matrix_effect_static(unsigned char variable_storage, unsigned char led_id, struct razer_rgb *rgb)
{
    struct razer_report report = razer_chroma_extended_matrix_effect_base(0x09, variable_storage, led_id, 0x01);
    report.arguments[5] = 0x01;
    report.arguments[6] = rgb->r;
    report.arguments[7] = rgb->g;
    report.arguments[8] = rgb->b;
    return report;
}
```

`razer_chroma_extended_matrix_effect_base` (`driver/razerchromacommon.c:481`):

```c
struct razer_report report = get_razer_report(0x0F, 0x02, arg_size);   // arg_size = 0x09
report.arguments[0] = variable_storage;   // 0x01 VARSTORE
report.arguments[1] = led_id;             // 0x0B
report.arguments[2] = effect_id;          // 0x01 static
// arguments[3], arguments[4] left 0x00
```

So OpenRazer's 9 arg bytes for "green on, red/blue off" are:

```
01 0B 01 00 00 01 00 FF 00
a0 a1 a2 a3 a4 a5 a6 a7 a8
```

### 1.2 What CommandPost `HSRazerTartarusProDevice.m` sends

`-setGreenStatusLight:` (lines 373–422; `-setOrangeStatusLight:` / `-setBlueStatusLight:`
identical bar which channel is the toggle):

```objc
NSDictionary *arguments = @{
    @0 : @0x01,  @1 : @0x0b,  @2 : @0x01,
    @3 : @0x00,  @4 : @0x01,  @5 : @0x01,
    @6 : orangeStatus,          // cached "orange" (=red) channel
    @7 : @(onOrOff),            // 0xff if active else 0x00
    @8 : blueStatus,            // cached blue channel
};
return [self sendRazerReportToDeviceWithTransactionID:0x1f commandClass:0x0f commandID:0x02 arguments:arguments];
```

`data_size` = `[arguments count]` = **9** (`HSRazerDevice.m:527`, `report.data_size = dataSize`).
Same 90-byte report, same CRC `for(i=2;i<88) crc ^= report[i]` (`HSRazerDevice.m:551`),
same `wIndex = self.index = 0x02` (`HSRazerTartarusProDevice.m:17`).

CommandPost also caches the other two channels in its own `HSRazerResult` state and
re-sends the whole triple every time — it calls `getGreenStatusLight` / `getBlueStatusLight`
first to repopulate them (lines 382–400), i.e. it *tries* read-back but still keeps a cache.

### 1.3 The one disagreeing byte, and what the arg slots mean

| arg | OpenRazer #2336 | CommandPost | slot meaning (from the `0x0F/0x02` family) |
|-----|-----------------|-------------|--------------------------------------------|
| a0  | `0x01` | `0x01` | `variable_storage` — VARSTORE |
| a1  | `0x0B` | `0x0b` | `led_id` — side stripe |
| a2  | `0x01` | `0x01` | `effect_id` — static |
| a3  | `0x00` | `0x00` | effect sub-param 1 — *unused for static* (is `direction` for wave/wheel, `colour-count` for breathing) |
| a4  | **`0x00`** | **`0x01`** | effect sub-param 2 — *unused for static* (is `speed` for wave `0x28`, starlight, reactive) |
| a5  | `0x01` | `0x01` | **colour count** — `0x01` for all single-colour effects (static, reactive, starlight-single, breathing-single); `0x02` for dual |
| a6  | `r` | `r` | red LED (`0x00` off … `0xFF` on / brightness) |
| a7  | `g` | `g` | green LED |
| a8  | `b` | `b` | blue LED |

Slot meanings are cross-referenced across every `0x0F/0x02` caller in
`razerchromacommon.c`: `_effect_wave` (a3=direction, a4=`0x28` speed),
`_effect_wheel` (a3=direction, a4=`0x28`), `_effect_starlight_single` (a4=speed, a5=`0x01`),
`_effect_reactive` (a4=speed, a5=`0x01`, a6–8=rgb), `_effect_breathing_single`
(a3=`0x01`, a5=`0x01`, a6–8=rgb), `_effect_breathing_dual` (a3=`0x02`, a5=`0x02`).
**a5 is consistently the colour count**; a3/a4 are effect-specific and, for the *static*
effect, carry no payload — the canonical Synapse capture in the `razer_chroma_extended_matrix_effect_static`
doc-comment (`razerchromacommon.c:507`) is `01 05 01 00 00 01 ff 00 00`, i.e. **a3=a4=0x00**.

**⇒ Acheron should send `a3=0x00, a4=0x00, a5=0x01`** — the OpenRazer/Synapse-capture form.
CommandPost's `a4=0x01` is almost certainly inert (it lands in the ignored speed slot) but
is a cheap documented fallback if a real unit doesn't react to `a4=0x00`.

### 1.4 Was it confirmed on hardware?

Partly, and enough. In the PR #2336 review thread, z3ntu asks (2025-02-19, comment on
`razerchromacommon.c`): *"depending on the R, G and B values in this call the 3 LEDs just
change brightness? So the color is fixed for them for red, green and blue … E.g. 00FF00
makes only green be on and red and blue LEDs be off here?"* — plxty (device owner)
replies (2025-02-20): *"Yes, exactly!"* and *"I'm using it as a kanata layer indicator,
with three LED is fine at the moment."* So the **write** frame with `transaction_id 0x1F`,
LED `0x0B`, VARSTORE, is confirmed working on a real Tartarus Pro. plxty's caveat *"I've
no Windows environment currently, therefore no promising of the correctness of the
protocol"* (z3ntu: *"If it works, it works :)"*) is about not having verified it against a
Synapse USB capture — not about it failing.

---

## 2. The "off" frame

**Neither reference implementation ever sends `effect_none` on LED `0x0B`.**

- OpenRazer: `razer_attr_write_matrix_effect_none` adds a Tartarus Pro case (PR #2336 diff,
  old line 1707) but it targets the **backlight** (`master` routes the Pro's
  `matrix_effect_none` through the shared Tartarus-V2 path on `BACKLIGHT_LED`), not `0x0B`.
  The side-stripe LEDs are only ever touched by the *static* frame.
- CommandPost: `-setBacklightToOff` = `-setBacklightToStaticColor:[NSColor blackColor]`
  (`HSRazerTartarusProDevice.m:107`). There is no "status light off" method at all —
  callers pass `active:NO`, which sends the static frame with that channel byte `0x00`.
- plxty toggles his kanata indicator channels on and off purely via the static frame.

`razer_chroma_extended_matrix_effect_none` does exist (`razerchromacommon.c:498`:
`_effect_base(0x06, varstore, led_id, 0x00)` — effect id `0x00`, `data_size 0x06`, args
`{varstore, led_id, 0x00, 0…}`). **Ticket 01 tried it on LED `0x0B`: it ACKs (`status 0x02`)
but does nothing to the LEDs** — the static-zero frame is the working off path.

**⇒ Acheron's "off" = the static frame with the relevant channel byte(s) `0x00`.**
All-off frame: args `00 0B 01 00 00 01 00 00 00`. Do not use `effect_none`.

---

## 3. The read-back frame — `command_id 0x82`

### 3.1 Request bytes

OpenRazer PR #2336 adds `razer_chroma_extended_matrix_get_effect_static`
(`driver/razerchromacommon.c`, new in the diff):

```c
static struct razer_report razer_chroma_extended_matrix_get_effect_base(unsigned char arg_size, unsigned char variable_storage, unsigned char led_id, unsigned char effect_id)
{
    struct razer_report report = get_razer_report(0x0F, 0x82, arg_size);
    report.arguments[0] = variable_storage;
    report.arguments[1] = led_id;
    report.arguments[2] = effect_id;
    return report;
}
struct razer_report razer_chroma_extended_matrix_get_effect_static(unsigned char variable_storage, unsigned char led_id)
{
    struct razer_report report = razer_chroma_extended_matrix_get_effect_base(0x09, variable_storage, led_id, 0x01);
    report.arguments[5] = 0x01;
    return report;
}
```

⇒ request args (`data_size 0x09`): `01 0B 01 00 00 01 00 00 00`, `transaction_id 0x1F`
(the read handlers set `request.transaction_id.id = 0x1F`). **Byte-identical to the write
frame except `command_id` (`0x82` vs `0x02`) and the RGB bytes zeroed.**

CommandPost's `-getOrangeStatusLight` / `-getGreenStatusLight` / `-getBlueStatusLight`
(lines 336–535) send the **same 9-arg shape** (`@0:0x01 @1:0x0b @2:0x01 @3:0x00 @4:0x01
@5:0x01 @6:0 @7:0 @8:0`, `commandID:0x82`, `transactionID:0x1f`). (The ASCII-art comment
above `-getOrangeStatusLight`, `06 0f 02 00 00 08 00 01 00`, is a stale copy-paste from
the brightness getter — the actual dict it builds is the 9-arg one. Ignore the comment.)

### 3.2 Where the channels land in the response

OpenRazer `razer_attr_read_profile_led_red/green/blue`, Tartarus Pro cases (PR #2336 diff):

```c
case USB_DEVICE_ID_RAZER_TARTARUS_PRO:
    request = razer_chroma_extended_matrix_get_effect_static(VARSTORE, SIDE_STRIPE_LED);
    request.transaction_id.id = 0x1F;
    red_index = 6;      // green_index = 7;  blue_index = 8;
    break;
...
return sprintf(buf, "%d\n", clamp_u8(response.arguments[red_index], 0, 1));
```

⇒ **R = `response.arguments[6]`, G = `arguments[7]`, B = `arguments[8]`** — the same slots
as the write frame. CommandPost reads the same (`[result argumentSix/Seven/Eight]`,
comparing `== 0xff`). OpenRazer clamps to `0..1`; CommandPost treats `0xFF` as "on".

In Acheron's response buffer (`analog.rs`, `RESP_ARGS = 9`): R = `buf[15]`, G = `buf[16]`,
B = `buf[17]`; class echo `buf[7] == 0x0F`, id echo `buf[8] == 0x82`, status `buf[1]`.

### 3.3 …but read-back is not trustworthy (across devices — though it held up on our unit)

z3ntu, PR #2336 review on the get helper (2024-12-09): *"we can probably skip this, across
devices we can never reliably get back what we've set earlier so no need to have that call
really."* plxty, 2025-02-17: *"the "Side Strip LED" problem hasn't fixed yet (will still
read values from device)"* and repeatedly proposed caching the RGB triple in the kernel
module instead. The unreliable read-back is the specific thing that kept #2336 from
merging. CommandPost keeps its own cached channel state regardless.

> **Hardware check ([ticket 01](../issues/01-prototype-status-led-controllability.md), fw
> v1.2, 2026-09-02):** on *our* unit `0x82` was reliable — ~20 immediate reads-after-write
> all matched, and (the case this caution is really about) **4 cold reads after a replug,
> with no host write since, all returned the true firmware state** (`ff 00 00`), not the
> stale value set before the replug. So the "seed daemon state at startup" use is safe here.
> The recommendation below still stands as the safe *cross-device* choice — and because the
> daemon must write unconditionally on every connect anyway (nothing persists), a startup GET
> buys little.

**⇒ Acheron must own an authoritative RGB triple in daemon state and re-send the full
static frame on every change** (exactly what both reference impls do). A single best-effort
`0x82` read at startup to *seed* that state is fine; a per-change GET-then-modify-then-SET
cycle is not safe to rely on.

---

## 4. `data_size` / argument count

`data_size = 0x09` (9 argument bytes) for both the write (`0x02`) and read (`0x82`) frames.

- OpenRazer: `_effect_base(0x09, …)` / `_get_effect_base(0x09, …)` — the `arg_size`
  argument is written to `report.data_size` by `get_razer_report`
  (`razercommon.c:129`, `new_report.data_size = data_size`).
- CommandPost: `report.data_size = (int)[arguments count]` = 9
  (`HSRazerDevice.m:527,534`); the doc-comments annotate `# Params 09`.
- `effect_none` would be `0x06`, not `0x09` (`razerchromacommon.c:500`) — another reason
  not to use it here.

`struct razer_report` layout (`razercommon.h`, header comment): `status[0]`,
`transaction_id[1]`, `remaining_packets[2..3]`, `protocol_type[4]`, `data_size[5]`,
`command_class[6]`, `command_id[7]`, `arguments[8..87]` (max 80), `crc[88]`, `reserved[89]`.
CRC = XOR of bytes `[2..87]` (`razer_calculate_crc`, `razercommon.c:111`, `for(i=2;i<88)`).

Acheron's `build_razer_cmd(0x1F, 0x0F, 0x02, &[9 bytes])` (`daemon/src/capture/analog.rs:159`)
produces this exactly: it sets `buf[6] = args.len() = 0x09`, `buf[7]=class`, `buf[8]=id`,
`buf[9..18]=args`, `buf[89]=XOR(buf[3..89])` — the same struct with the extra leading
report-number byte, so buffer index = struct index + 1 and the CRC fold `buf[3..89]`
covers struct bytes `[2..87]`. The frame is expressible through the existing helper with
no changes.

---

## 5. Driver-mode ("Capture mode") dependency

**Neither reference implementation enables driver mode for the LED frame, and OpenRazer
proves the `0x0F/0x02` class works with driver mode permanently off on this device.**

- **OpenRazer daemon `master`** (`daemon/openrazer_daemon/hardware/keyboards.py`,
  `class RazerTartarusPro`): `DRIVER_MODE = False`. The Pro is the **only** keyboard-family
  device with driver mode disabled. Its lighting methods (`set_static_effect`,
  `set_spectrum_effect`, …) all still function — they route through the same
  `command_class 0x0F` extended-matrix path.
- **OpenRazer kernel `master`** (`razerkbd_driver.c:5798`): the probe deliberately skips
  even the *normal*-mode command for this PID — *"Set device to regular mode, not driver
  mode … Tartarus Pro resets when it receives this command … `if
  (idProduct != USB_DEVICE_ID_RAZER_TARTARUS_PRO)`"*.
- **OpenRazer LED handlers** build the request and call `razer_send_payload` directly —
  there is no device-mode call anywhere in the `profile_led` / `matrix_effect_*` paths.
- **CommandPost** is a lighting-only integration; `HSRazerDevice.m` and
  `HSRazerTartarusProDevice.m` contain **no** `set_device_mode` / `0x00 0x04` command of
  any kind. It sends the status-LED frame on a freshly targeted control transfer, cold.

**⇒ The status-LED frame is independent of Acheron's Capture mode.** Send it on a
short-lived Interface-2 fd the way `read_device_info()` and `relock()` already do
(`analog.rs`), whether or not the grid task has the device unlocked. This **contradicts**
grounding §3's *"Enable driver/streaming mode first."*

---

## 6. VARSTORE (`0x01`) vs NOSTORE (`0x00`)

**Both reference impls use VARSTORE (`0x01`) exclusively for this frame.** There is no
NOSTORE variant of the `0x0F/0x02` LED-effect command anywhere in `razerchromacommon.c` —
the entire extended-matrix effect family passes `variable_storage` straight through and
every caller in the tree passes `VARSTORE`. (`NOSTORE 0x00` is real —
`razercommon.h:33` — and PR #1577 uses it, but only for `MACRO_LED` via the *classic*
`razer_chroma_standard_set_led_effect` command `0x03/0x00`, a different command family.)

Whether the Pro's firmware honours `arg0 = 0x00` on the `0x0F/0x02` frame was **untested by
any primary source** at the time of writing. `arg0` is copied verbatim into
`report.arguments[0]` (`_effect_base`), so a NOSTORE attempt is a one-byte change and safe
to try.

> **Hardware-verified ([ticket 01](../issues/01-prototype-status-led-controllability.md), fw
> v1.2, 2026-09-02) — this section's premise was wrong.** On our unit **neither `arg0 = 0x01`
> nor `arg0 = 0x00` persists across a USB re-enumeration.** Wrote green with VARSTORE,
> replugged → device came back **orange-only** (firmware keymap-indicator power-on default);
> identical result with NOSTORE. The persistent orange LED is *not* a stored host write from
> Synapse — it is simply the firmware default. Further: a settled `0x82` read-back **always
> echoes `arg0 = 0x01`** regardless of what was sent, so the firmware appears to ignore the
> distinction and treat every write as VARSTORE.
>
> Consequences:
> - The "stale *host* RGB flashes on boot" concern **does not apply** — the firmware always
>   reclaims the LEDs to orange-only on enumeration, so there is a brief orange-only window on
>   every connect no matter the storage mode, closed only by the daemon asserting.
> - **The daemon must assert Status-LED state on startup *and* on every device reconnect** —
>   a hard requirement (feeds [ticket 03](../issues/03-daemon-architecture-for-status-leds.md)),
>   not a boot-flash mitigation.
> - `arg0` choice is **cosmetic on this unit.** Send `arg0 = 0x00` for intent-clarity (we do
>   not want persistence), but expect no observable difference and **do not assume it avoids
>   flash writes.** The config knob floated below is **probably not worth exposing** — there
>   is no user-visible behaviour to toggle.

**⇒ Recommendation (post-hardware): send `arg0 = 0x00` (NOSTORE) for intent-clarity; the
daemon owns the LEDs and re-asserts on every connect regardless.** The earlier
"try NOSTORE first, fall back to VARSTORE, expose a config knob" plan is superseded — on
fw v1.2 the byte is inert and nothing persists either way.

---

## 7. Corrections & clarifications for `tartarus-pro-status-leds.md` §3

| Grounding §3 says | This file's finding |
|---|---|
| "Enable driver/streaming mode first (`command_class 0x00, command_id 0x04, arg0 = 0x03`), the same as for the backlight" | **No.** Neither impl does. OpenRazer runs this device with `DRIVER_MODE = False` and its `0x0F` lighting still works. The LED frame is independent of Capture mode — send it on a standalone Interface-2 fd. (§5) |
| arg table "arg 4 = `0x01` or `0x00` … arg 5 = `0x01`" | Table is **correct** — not a contradiction. Clarifications: arg5 = `0x01` is the "colour count" byte (both impls, always); arg4 is the effect's ignored "speed" slot, so **send arg4 = `0x00`** (OpenRazer's value = Razer's own captured static frame). CommandPost's `arg4 = 0x01` is a documented fallback only. (§1.3) |
| Read-back: "current channel values come back in `arg6/arg7/arg8`" — presented as a clean way to GET-modify-SET one channel | Slot positions are right (r=6, g=7, b=8). z3ntu rejected the GET call as unreliable *across devices* ("we can never reliably get back what we've set earlier") and it's why #2336 stalled — but [ticket 01](../issues/01-prototype-status-led-controllability.md) tested it directly on fw v1.2 and **it was reliable, including the cold-read-after-replug seed case**. Keep an authoritative RGB triple in daemon state anyway (safe cross-device; the daemon must write unconditionally on connect regardless). (§3.3) |
| "An `effect_none` … on LED `0x0B` is plausible but untested" | Not done by either impl; [ticket 01](../issues/01-prototype-status-led-controllability.md) tried it — **`effect_none` ACKs (`status 0x02`) but does nothing to the LEDs.** Off = static frame with channel byte `0x00`. (§2) |
| arg0 "= `0x01` VARSTORE (persists in onboard memory)" — stated as the only option | **Nothing persists** on fw v1.2 — [ticket 01](../issues/01-prototype-status-led-controllability.md) found neither VARSTORE nor NOSTORE survives re-enumeration, and read-back always echoes `arg0 = 0x01`. Send `arg0 = 0x00` for intent-clarity; the byte is cosmetic here. The orange LED is the firmware power-on default, not a stored write. (§6) |
| PR #1577 cited as an implementation ("Profile LEDs are now set together as one RGB led (0xb)") | That phrase is from the PR's commit message / description. **The #1577 diff contains no `0x0B` code** — it treats the Pro exactly like the Tartarus V2 (backlight-only, `BACKLIGHT_LED`). #2336 is the only real implementation of the `0x0B` path. |

Also worth noting (not a contradiction): grounding §5 says the merged PR #2710 "deliberately
omits the status LEDs". Confirmed against `master` — `RazerTartarusPro` in both the kernel
`razer_kbd_probe` and the daemon class creates **no** `profile_led_*` files. The status
LEDs remain unimplemented in any shipping OpenRazer release; #2336 is the sole working
reference and it lives only as an unmerged PR.

---

## 8. Implementation-ready frames

All three go through `build_razer_cmd(txn, class, id, args)`
(`daemon/src/capture/analog.rs:159`) unchanged. `txn = 0x1F`. 91-byte buffer; struct byte =
buffer index − 1. CRC = `XOR(buf[3..89])`.

### 8.1 Write frame — set all three channels

`build_razer_cmd(0x1F, 0x0F, 0x02, &[0x00, 0x0B, 0x01, 0x00, 0x00, 0x01, r, g, b])`

> **Ticket 01 settled `arg0 = 0x00`** (the storage byte is inert on fw v1.2; `0x00` just
> states intent — nothing persists either way). Tables updated accordingly; `build_razer_cmd`
> computes the CRC itself so the worked values are only for cross-checking a USB capture.

| buf idx | value | field |
|---------|-------|-------|
| `[0]`  | `0x00` | report number (leading byte, always 0) |
| `[1]`  | `0x00` | status |
| `[2]`  | `0x1F` | transaction_id |
| `[3..5]` | `00 00` | remaining_packets |
| `[5]`  | `0x00` | protocol_type |
| `[6]`  | `0x09` | data_size |
| `[7]`  | `0x0F` | command_class |
| `[8]`  | `0x02` | command_id (write) |
| `[9]`  | `0x00` | arg0 — storage byte; **send `0x00`** (inert on fw v1.2 — ticket 01/§6) |
| `[10]` | `0x0B` | arg1 — LED id `SIDE_STRIPE_LED` |
| `[11]` | `0x01` | arg2 — effect id: static |
| `[12]` | `0x00` | arg3 — unused for static |
| `[13]` | `0x00` | arg4 — unused for static (CommandPost sends `0x01`; fallback only) |
| `[14]` | `0x01` | arg5 — colour count = 1 |
| `[15]` | `r`    | arg6 — red LED   (`0x00` off … `0xFF` on) |
| `[16]` | `g`    | arg7 — green LED |
| `[17]` | `b`    | arg8 — blue LED |
| `[18..88]` | `00` | padding |
| `[89]` | `0x0F ^ r ^ g ^ b` | crc |
| `[90]` | `0x00` | reserved |

CRC derivation: `0x09^0x0F^0x02 ^ 0x00^0x0B^0x01 ^ 0x00^0x00^0x01 ^ r^g^b = 0x0F ^ r ^ g ^ b`.
(`build_razer_cmd` computes the CRC — these values are just for cross-checking a capture.)

Worked examples (`arg0 = 0x00`):

| intent | r g b | crc | arg bytes `[9..18]` |
|--------|-------|-----|----------------------|
| all off | `00 00 00` | `0x0F` | `00 0B 01 00 00 01 00 00 00` |
| green only | `00 FF 00` | `0xF0` | `00 0B 01 00 00 01 00 FF 00` |
| red only | `FF 00 00` | `0xF0` | `00 0B 01 00 00 01 FF 00 00` |
| all on | `FF FF FF` | `0xF0` | `00 0B 01 00 00 01 FF FF FF` |

### 8.2 Off frame

Not a distinct command — it is §8.1 with `r = g = b = 0x00`
(arg bytes `00 0B 01 00 00 01 00 00 00`, crc `0x0F` with `arg0 = 0x00`). To turn off just one
channel, send the full frame with that channel `0x00` and the other two at their cached values.
`effect_none` is **not** an off frame — ticket 01 confirmed it ACKs but does nothing.

### 8.3 Read-back frame — `command_id 0x82`

`build_razer_cmd(0x1F, 0x0F, 0x82, &[0x00, 0x0B, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00])`
(OpenRazer's GET helper passes `0x01` here; the storage byte is equally inert — match the
write's `0x00`. Ticket 01 confirmed the read itself is reliable on fw v1.2.)

| buf idx | value | field |
|---------|-------|-------|
| `[2]`  | `0x1F` | transaction_id |
| `[6]`  | `0x09` | data_size |
| `[7]`  | `0x0F` | command_class |
| `[8]`  | `0x82` | command_id (read) |
| `[9]`  | `0x00` | arg0 — match the write's storage byte (inert) |
| `[10]` | `0x0B` | arg1 — LED id |
| `[11]` | `0x01` | arg2 — effect id: static |
| `[12]` | `0x00` | arg3 |
| `[13]` | `0x00` | arg4 |
| `[14]` | `0x01` | arg5 — colour count |
| `[15..18]` | `00 00 00` | arg6–8 (zero on request) |
| `[89]` | `0x8F` | crc (`0x8E` if `arg0 = 0x01`) |

CRC: `0x09^0x0F^0x82 ^ 0x00^0x0B^0x01 ^ 0x01 = 0x8F` (`build_razer_cmd` computes this).

**Response** (via `HIDIOCGFEATURE`, 91-byte buffer, `RESP_ARGS = 9`):

| resp buf idx | meaning |
|--------------|---------|
| `[1]` | status — accept `0x00`/`0x01`/`0x02` (as `analog.rs` `response_echoes` already does) |
| `[7]` | command_class echo — expect `0x0F` |
| `[8]` | command_id echo — expect `0x82` |
| `[15]` | **red channel** (`arguments[6]`) |
| `[16]` | **green channel** (`arguments[7]`) |
| `[17]` | **blue channel** (`arguments[8]`) |

Channel values may come back as `0x00`/`0xFF` (CommandPost's assumption) or `0x00`/`0x01`
(OpenRazer clamps). **Treat any non-zero as "on"; do not trust this read as authoritative
(§3.3) — use it only to seed daemon state at startup.**

---

## 9. Answers to the six open questions

*[Ticket 01](../issues/01-prototype-status-led-controllability.md) verified 1, 2, 4, 5 on
hardware exactly as stated; it corrected the hardware-behaviour predictions in 3 and 6 (noted
inline).*

1. **`arg3`/`arg4` semantics.** `arg3` and `arg4` are the two effect-specific sub-parameter
   slots (direction / speed for wave/starlight/reactive); the **static** effect uses
   neither, so both are `0x00` in Razer's own captured frames and in OpenRazer. `arg5 =
   0x01` is the colour-count byte and is the value both impls actually share. CommandPost's
   `arg4 = 0x01` sits in the ignored speed slot and is very likely a no-op.
   **Send `arg3 = 0x00, arg4 = 0x00, arg5 = 0x01`** (= `razer_chroma_extended_matrix_effect_static`);
   keep CommandPost's `arg4 = 0x01` as a documented fallback.

2. **The "off" frame.** Static effect, channel byte(s) `0x00`. Neither reference impl ever
   sends `effect_none` (`0x00` / `data_size 0x06`) on LED `0x0B` — OpenRazer's `effect_none`
   for the Pro targets the backlight. Do not use `effect_none` here.

3. **Read-back frame.** `command_id 0x82`, `transaction_id 0x1F`, `data_size 0x09`, args
   `01 0B 01 00 00 01 00 00 00` (byte-identical to the write frame bar `command_id` and the
   zeroed RGB). Channels return in `arguments[6]/[7]/[8]` (Acheron resp buffer `[15]/[16]/[17]`).
   z3ntu rejected the GET as unreliable *across devices*, but **[ticket 01](../issues/01-prototype-status-led-controllability.md)
   verified it reliable on fw v1.2**, including cold reads after a replug. Keep an
   authoritative RGB triple in daemon state anyway (safe cross-device; the daemon re-asserts
   on every connect regardless), and a startup `0x82` seed is trustworthy on this unit.

4. **`data_size` / argument count.** `0x09` (9 arg bytes) — confirmed in OpenRazer
   (`_effect_base(0x09,…)`), CommandPost (`[arguments count]` = 9, doc-comment `09`), and
   consistent with `struct razer_report` (`data_size` at byte `[5]`, CRC over `[2..87]`).
   Expressible unchanged through `analog.rs::build_razer_cmd`.

5. **Driver-mode dependency.** None. CommandPost never sends any device-mode command;
   OpenRazer's daemon sets `DRIVER_MODE = False` for this device alone and its `0x0F`
   lighting still works; the kernel LED handlers issue the frame with no mode call. The
   status-LED frame is independent of Acheron's Capture mode — send it on a standalone
   Interface-2 fd. (Grounding §3's "enable driver mode first" is wrong.)

6. **VARSTORE vs NOSTORE.** Both impls use VARSTORE (`0x01`); there is no NOSTORE variant
   of `0x0F/0x02` in the OpenRazer tree. Source analysis predicted VARSTORE would persist and
   flash stale state on boot — **[ticket 01](../issues/01-prototype-status-led-controllability.md)
   disproved this on fw v1.2: neither VARSTORE nor NOSTORE survives a USB re-enumeration**
   (the firmware reclaims the LEDs to its orange-only power-on default), and read-back always
   echoes `arg0 = 0x01`, so the byte appears inert. **Send `arg0 = 0x00` for intent-clarity;
   no config knob** (nothing observable to toggle). The real requirement this surfaces:
   **the daemon must assert Status-LED state on startup *and* every device reconnect**
   (feeds [ticket 03](../issues/03-daemon-architecture-for-status-leds.md)).

---

## 10. Sources (all primary)

- **OpenRazer PR #2336** — raw diff `https://patch-diff.githubusercontent.com/raw/openrazer/openrazer/pull/2336.diff`;
  review thread `https://api.github.com/repos/openrazer/openrazer/pulls/2336/comments` and
  `.../issues/2336/comments` (z3ntu ↔ plxty, Dec 2024 – Mar 2026). Closed 2026-03-11
  "in favor of #2710 … with reduced feature set".
- **OpenRazer `master` driver** — `driver/razerchromacommon.c` (`_effect_base` :481,
  `_effect_static` :511, `_effect_none` :498), `driver/razercommon.{c,h}`
  (`razer_calculate_crc` :111, `get_razer_report` :129, `struct razer_report` /
  `NOSTORE`/`VARSTORE` defines), `driver/razerkbd_driver.c` (`razer_get_report_params`
  Tartarus Pro → `report/response_index 0x02` :363; `razer_set_device_mode` → `0x1F` :545;
  probe skip-normal-mode carve-out :5798; Tartarus Pro device-file list, no `profile_led_*`
  :5653).
- **OpenRazer `master` daemon** — `daemon/openrazer_daemon/hardware/keyboards.py`,
  `class RazerTartarusPro`: `DRIVER_MODE = False`, `MATRIX_DIMS = [1, 21]`, no
  `keypad_*_profile_led_*` methods.
- **OpenRazer PR #1577** — raw diff `https://patch-diff.githubusercontent.com/raw/openrazer/openrazer/pull/1577.diff`:
  treats the Pro identically to the Tartarus V2 (`BACKLIGHT_LED`, `transaction_id 0x1F`);
  contains no `0x0B` code.
- **CommandPost** — `extensions/razer/HSRazerTartarusProDevice.m` (status-LED set/get
  :284–535) and `extensions/razer/HSRazerDevice.m`
  (`sendRazerReportToDeviceWithTransactionID:` :502, `data_size = [arguments count]` :527,
  CRC :551, `wLength 90` / `wValue 0x300` / `wIndex self.index`).
- **Acheron** — `daemon/src/capture/analog.rs` (`build_razer_cmd` :159, response offsets
  `RESP_*` :127–130, `hidioc*feature` :140).
