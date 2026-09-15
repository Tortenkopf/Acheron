# Research: Tartarus Pro backlight — implementation-ready wire protocol

Ticket: [01-research-backlight-wire-protocol](../issues/01-research-backlight-wire-protocol.md)

Byte-by-byte wire spec for every Lighting effect `RazerTartarusPro` exposes
(`/usr/lib/python3/dist-packages/openrazer_daemon/hardware/keyboards.py:211-231`): static,
spectrum, none, reactive, breath (random/single/dual), wave, starlight (random/single/dual),
ripple (+ random-colour), custom/`set_key_row`, get/set brightness. Modelled on
[`tartarus-status-leds/research/status-led-wire-protocol.md`](../../tartarus-status-leds/research/status-led-wire-protocol.md)
— same depth, same "confirmed from source" vs "needs hardware" framing. Every claim below is
cited to file + line/function in the vendored OpenRazer 3.12.4 tree.

---

## Bottom line

- All 13 effects ride the **same extended-matrix command family** the Status LEDs use
  (`command_class 0x0F`), but almost all of them are `command_id 0x02` ("set effect"), not
  `0x82` ("get") — there is no analogue of the Status LEDs' read-back path for backlight
  effects (brightness is the one exception: it has both a `0x04` set and an `0x84` get).
- The backlight's dedicated LED id is **`BACKLIGHT_LED = 0x05`**
  (`razercommon.h:52`) — confirmed different from the Status LEDs' `SIDE_STRIPE_LED = 0x0B`.
  `transaction_id 0x1F` and `VARSTORE` (`0x01`) for every effect-select frame, exactly like
  the Status LEDs (§1).
- **Brightness is the one place BACKLIGHT_LED is *not* used** — the Tartarus Pro (and V2)
  brightness get/set frames target **`ZERO_LED = 0x00`**, a Tartarus-specific quirk; every
  other keyboard in the switch uses `BACKLIGHT_LED` for brightness (§9).
- **`ripple` and `ripple_effect_random_colour` are not device-side effects at all.** The
  daemon's dbus handlers for them only cache persistence state and emit an internal
  notification — no sysfs write happens. A background Python thread
  (`RippleEffectThread.run`, ~25 Hz) computes the moving ripple and **streams it as repeated
  `matrix_custom_frame` + `matrix_effect_custom` writes** — the same two commands the
  Custom-layout path uses. There is no dedicated ripple firmware command to reverse-engineer
  (§7). This has real implications for scope/cost that ticket 02 needs to weigh.
- **One real driver quirk found:** the Tartarus Pro/V2 breathing-effect handler builds the
  frame with `transaction_id = 0x3F`, sends it, and *only then* overwrites
  `request.transaction_id.id = 0x1F` — a dead store with no second send
  (`razerkbd_driver.c:3172-3176`, `3178-3183`, `3185-3190`). **The frame Breath actually
  transmits uses `transaction_id 0x3F`**, not `0x1F` like every other Tartarus Pro effect.
  Worth a hardware sanity-check even though the driver undisputedly sends `0x3F` (§6).
- **The Custom/`set_key_row` two-step is confirmed from source**: a `matrix_custom_frame`
  (`0x0F/0x03`) write of the pixel row(s) must be followed by a separate
  `matrix_effect_custom` (`0x0F/0x02`, effect `0x08`) write to "arm"/display it — both the
  kernel driver's `_store` functions and the daemon's own `RippleManager.refresh_keyboard()`
  (which calls `_set_key_row` then `_set_custom_effect`, in that order) demonstrate the
  sequencing (§10.3).
- **The scroll wheel's column index cannot be determined from source.** The custom-frame
  driver path treats the Tartarus Pro completely generically — `row_id`/`start_col`/`stop_col`
  are opaque bytes the kernel passes straight through with no Tartarus-Pro-specific remap
  table anywhere in `razerkbd_driver.c`. The daemon's only per-device key/LED-position mapping
  table (`keyboard.py`'s `TARTARUS_KEY_MAPPING`) is for the *classic* Tartarus's *input* events,
  is never referenced for RGB, and does not apply to the Pro at all (grep confirms zero
  `TARTARUS_PRO` hits in `keyboard.py`). **This is a real "needs hardware" gap, not a guess to
  avoid making** — say so plainly to ticket 02 (§10.4).

---

## 1. Shared frame anatomy

Every backlight command in this file rides the identical `struct razer_report` /
`get_razer_report(command_class, command_id, data_size)` / CRC machinery already nailed down
for the Status LEDs — see
[`status-led-wire-protocol.md` §4](../../tartarus-status-leds/research/status-led-wire-protocol.md#4-data_size--argument-count)
for the byte-for-byte struct layout and CRC fold; it's not re-derived here.
(`razercommon.h:135-143` `struct razer_report`; `razercommon.h:164-169` prototypes.)

**Report/response index** — Tartarus Pro is grouped with Ornata V2/V3, BlackWidow V3 Pro
Wired, BlackWidow V4 X, etc.: `report_index = response_index = 0x02`
(`razerkbd_driver.c:376-381`, `razer_get_report_params`, case list ending
`USB_DEVICE_ID_RAZER_TARTARUS_PRO`). Same index the Status LED frame uses (ADR-0006).

**LED id constants** (`razercommon.h:44-58`):

| constant | value | used by |
|---|---|---|
| `ZERO_LED` | `0x00` | Tartarus Pro/V2 **brightness only** (§9) |
| `BACKLIGHT_LED` | `0x05` | every backlight *effect* frame in this file |
| `SIDE_STRIPE_LED` | `0x0B` | Status LEDs (different feature, different ticket) |
| `VARSTORE` / `NOSTORE` | `0x01` / `0x00` | storage-mode byte, `razercommon.h:44-45` |

**`transaction_id`**: `0x1F` for every Tartarus Pro effect-select frame
(`razerkbd_driver.c`, confirmed per-effect below) **except** the breathing effect, which
actually transmits `0x3F` due to a driver quirk (§6).

---

## 2. Static — `command_id 0x02`, effect `0x01`

`razer_chroma_extended_matrix_effect_static(VARSTORE, BACKLIGHT_LED, rgb)`
(`razerchromacommon.c:511-520`), built on `_effect_base(0x09, ...)` (`:481-490`). Wired for
the Pro at `razerkbd_driver.c:2866-2873` (`razer_attr_write_matrix_effect_static`,
`USB_DEVICE_ID_RAZER_TARTARUS_PRO` case): `transaction_id.id = 0x1F`. Daemon:
`set_static_effect` writes `bytes([red,green,blue])` straight to sysfs `matrix_effect_static`
(`chroma_keyboard.py:280-307`).

| arg | value | meaning |
|---|---|---|
| a0 | `0x01` | VARSTORE |
| a1 | `0x05` | `BACKLIGHT_LED` |
| a2 | `0x01` | effect id: static |
| a3 | `0x00` | unused |
| a4 | `0x00` | unused |
| a5 | `0x01` | colour count |
| a6/a7/a8 | r/g/b | colour |

`data_size = 0x09`. Doc-comment worked example (`razerchromacommon.c:507`, generic
transaction `0x3f`, illustrative only — Tartarus Pro overrides to `0x1F`):
`01 05 01 00 00 01 ff 00 00` (RGB `0xFF0000`).

## 3. None — `command_id 0x02`, effect `0x00`

`razer_chroma_extended_matrix_effect_none(VARSTORE, BACKLIGHT_LED)` = `_effect_base(0x06, ...,
0x00)` (`razerchromacommon.c:498-501`). Wired at `razerkbd_driver.c:2110-2136`
(`USB_DEVICE_ID_RAZER_TARTARUS_PRO` case in `razer_attr_write_matrix_effect_none`):
`transaction_id.id = 0x1F`. Daemon `set_none_effect` writes `'1'` to sysfs `matrix_effect_none`
(`chroma_keyboard.py:361-376`) — the payload content is ignored; any write triggers it.

| arg | value |
|---|---|
| a0 | `0x01` VARSTORE |
| a1 | `0x05` BACKLIGHT_LED |
| a2 | `0x00` effect id: none |
| a3–a5 | `0x00 0x00 0x00` |

`data_size = 0x06`.

## 4. Spectrum — `command_id 0x02`, effect `0x03`

`razer_chroma_extended_matrix_effect_spectrum(VARSTORE, BACKLIGHT_LED)` = `_effect_base(0x06,
..., 0x03)` (`razerchromacommon.c:605-608`). Wired at `razerkbd_driver.c:2485-2491`
(`USB_DEVICE_ID_RAZER_TARTARUS_PRO` in `razer_attr_write_matrix_effect_spectrum`):
`transaction_id.id = 0x1F`. Daemon `set_spectrum_effect` writes `'1'`
(`chroma_keyboard.py:341-357`, payload ignored, same trigger-by-any-write pattern as none).

| arg | value |
|---|---|
| a0 | `0x01` VARSTORE |
| a1 | `0x05` BACKLIGHT_LED |
| a2 | `0x03` effect id: spectrum |
| a3–a5 | `0x00 0x00 0x00` |

`data_size = 0x06`. No colour/speed args — spectrum cycles autonomously in firmware.

## 5. Wave — `command_id 0x02`, effect `0x04`

`razer_chroma_extended_matrix_effect_wave(VARSTORE, BACKLIGHT_LED, direction)` = `_effect_base
(0x06, ..., 0x04)`, `arguments[3] = direction` (clamped `0x00..0x02`), `arguments[4] = 0x28`
fixed speed (`razerchromacommon.c:531-544`). Wired at `razerkbd_driver.c:2280-2303`
(`USB_DEVICE_ID_RAZER_TARTARUS_PRO` in `razer_attr_write_matrix_effect_wave`):
`transaction_id.id = 0x1F`. Daemon `set_wave_effect(direction)` writes the direction as an
ASCII decimal string to sysfs `matrix_effect_wave` (`chroma_keyboard.py:225-248`); `direction`
is validated against `self.WAVE_DIRS` first — `RazerTartarusPro` doesn't override `WAVE_DIRS`,
so it inherits the base-class default **`(1, 2)`** (`hardware/device_base.py:45`; no
Pro-specific override anywhere in `keyboards.py`).

| arg | value | meaning |
|---|---|---|
| a0 | `0x01` | VARSTORE |
| a1 | `0x05` | BACKLIGHT_LED |
| a2 | `0x04` | effect id: wave |
| a3 | `direction` (1 or 2 from daemon; driver clamps 0–2) | direction |
| a4 | `0x28` | speed (fixed, lower = faster per comment) |
| a5 | `0x00` | unused |

`data_size = 0x06`.

## 6. Reactive — `command_id 0x02`, effect `0x05`

`razer_chroma_extended_matrix_effect_reactive(VARSTORE, BACKLIGHT_LED, speed, rgb)` =
`_effect_base(0x09, ..., 0x05)`, `arguments[4] = speed` (clamped `0x01..0x04`),
`arguments[5] = 0x01` colour count, `arguments[6..8] = rgb` (`razerchromacommon.c:639-652`).
Wired at `razerkbd_driver.c:2605-2630` (`USB_DEVICE_ID_RAZER_TARTARUS_PRO` in
`razer_attr_write_matrix_effect_reactive`): single send after the switch, `transaction_id.id
= 0x1F` (`:2629`, `:2710`) — **clean, no quirk** (contrast with Breath below). Daemon
`set_reactive_effect(r,g,b,speed)` writes `bytes([speed,r,g,b])`, clamping speed to `4` if not
in `(1,2,3,4)` (`chroma_keyboard.py:395-431`).

| arg | value |
|---|---|
| a0 | `0x01` VARSTORE |
| a1 | `0x05` BACKLIGHT_LED |
| a2 | `0x05` effect id: reactive |
| a3 | `0x00` unused |
| a4 | speed `0x01–0x04` |
| a5 | `0x01` colour count |
| a6/a7/a8 | r/g/b |

`data_size = 0x09`.

## 7. Breath — random / single / dual — `command_id 0x02`, effect `0x02`

Three builders, all `_effect_base(_, ..., 0x02)` (`razerchromacommon.c:662-695`):

| variant | builder | `arg3` | `arg5` (colour count) | `arg6..` | `data_size` |
|---|---|---|---|---|---|
| random | `_effect_breathing_random` (`:662-666`) | `0x00` (unset) | `0x00` (unset) | — | `0x06` |
| single | `_effect_breathing_single` (`:667-679`) | `0x01` | `0x01` | rgb1 | `0x09` |
| dual | `_effect_breathing_dual` (`:680-695`) | `0x02` | `0x02` | rgb1, rgb2 (a9–a11) | `0x0C` |

Daemon dispatch — all three write to the **same** sysfs file `matrix_effect_breath`, and the
kernel switches on the *byte count written* (not a sub-command byte) to pick the variant:
`set_breath_random_effect` writes `b'1'` (1 byte) (`chroma_keyboard.py:434-452`),
`set_breath_single_effect(r,g,b)` writes 3 bytes (`:455-483`), `set_breath_dual_effect(...)`
writes 6 bytes (`:486-523`). Kernel side switches on `count` in
`razer_attr_write_matrix_effect_breath` (`razerkbd_driver.c:3154` / `3170`, `switch(count)`
with `case 1/3/6`).

**⚠ Driver quirk — the transmitted `transaction_id` is `0x3F`, not `0x1F`.** For the
`USB_DEVICE_ID_RAZER_TARTARUS_PRO` case (`razerkbd_driver.c:3167-3196`), all three branches
do:
```c
request = razer_chroma_extended_matrix_effect_breathing_single(VARSTORE, BACKLIGHT_LED, ...);
request.transaction_id.id = 0x3F;
razer_send_payload(device, &request, &response);   // <-- sent here, with 0x3F
request.transaction_id.id = 0x1F;                  // <-- dead store, no second send
break;
```
Every other Tartarus Pro effect handler sets `transaction_id.id = 0x1F` *before* the single
`razer_send_payload` call. Breath is the one handler that sets `0x3F`, sends, then sets `0x1F`
on a `request` value that is never sent again. **This reads like a copy/paste bug** (the
Tartarus V2/Pro case block was probably adapted from the single-send-0x3F devices above it in
the same function, and the trailing `0x1F` line was added but the `razer_send_payload` call
wasn't duplicated). Whether the firmware actually requires `0x1F` and silently accepts/ignores
a `0x3F`-tagged Breath frame, or whether `0x3F` is in fact necessary for Breath specifically,
is **not resolvable from source — flag for hardware verification** (send Breath with both
`0x1F` and `0x3F` and confirm both work, or that only `0x3F` — what the driver actually does
today — works).

| arg | random | single | dual |
|---|---|---|---|
| a0 | `0x01` VARSTORE | same | same |
| a1 | `0x05` BACKLIGHT_LED | same | same |
| a2 | `0x02` effect id: breathing | same | same |
| a3 | `0x00` | `0x01` | `0x02` |
| a4 | `0x00` | `0x00` | `0x00` |
| a5 | `0x00` | `0x01` | `0x02` |
| a6–a8 | — | rgb1 | rgb1 |
| a9–a11 | — | — | rgb2 |

## 8. Starlight — random / single / dual — `command_id 0x02`, effect `0x07`

Three builders (`razerchromacommon.c:558-597`), speed always clamped `0x01..0x03`:

| variant | builder | `arg4` (speed) | `arg5` (colour count) | `arg6..` | `data_size` |
|---|---|---|---|---|---|
| random | `_effect_starlight_random` (`:558-566`) | speed | `0x00` (unset) | — | `0x06` |
| single | `_effect_starlight_single` (`:567-580`) | speed | `0x01` | rgb1 | `0x09` |
| dual | `_effect_starlight_dual` (`:581-597`) | speed | `0x02` | rgb1, rgb2 (a9–a11) | `0x0C` |

Wired for the Pro at `razerkbd_driver.c:3034-3054` (`razer_attr_write_matrix_effect_starlight`,
`USB_DEVICE_ID_RAZER_TARTARUS_PRO` case) — **clean single send per branch**,
`transaction_id.id = 0x1F` uniformly for all three (no Breath-style quirk here), dispatched by
`count` (`1` → random, `4` → single, `7` → dual). Daemon: `set_starlight_random_effect(speed)`
writes `bytes([speed])` (`chroma_keyboard.py:648-665`); `set_starlight_single_effect(r,g,b,speed)`
writes `bytes([speed,r,g,b])` (`:668-686`); `set_starlight_dual_effect(...)` writes
`bytes([speed,r1,g1,b1,r2,g2,b2])` (`:689-707`).

| arg | random | single | dual |
|---|---|---|---|
| a0 | `0x01` VARSTORE | same | same |
| a1 | `0x05` BACKLIGHT_LED | same | same |
| a2 | `0x07` effect id: starlight | same | same |
| a3 | `0x00` | `0x00` | `0x00` |
| a4 | speed `0x01–0x03` | speed | speed |
| a5 | `0x00` | `0x01` | `0x02` |
| a6–a8 | — | rgb1 | rgb1 |
| a9–a11 | — | — | rgb2 |

## 9. Ripple / ripple-random-colour — **not a device-side command**

The two daemon dbus endpoints do **no sysfs write of their own**:
`set_ripple_effect(r,g,b,refresh_rate)` only calls `set_persistence` and
`send_effect_event('setRipple', r,g,b,refresh_rate)` (`chroma_keyboard.py:604-629`);
`set_ripple_effect_random_colour(refresh_rate)` is the same minus the colour
(`:631-645`). `RazerTartarusPro` inherits `_RippleKeyboard`
(`keyboards.py:211`, class defined `keyboards.py:40-71`), whose `__init__` starts a
`_RippleManager` (`misc/ripple_effect.py`) that **subscribes to that `'setRipple'`
notification** (`RippleManager.notify`, `ripple_effect.py:217-240`) and enables/disables a
background `RippleEffectThread`.

`RippleEffectThread.run()` (`ripple_effect.py:100-170`) loops at `self._refresh_rate` (daemon
default `0.040` s = 25 Hz, `:30`, or the caller's `refresh_rate` argument), recomputing an
expanding-circle colour grid over a `KeyboardColour(rows, cols)` object sized from the device's
own `MATRIX_DIMS` (`:35`, so `1×21` for the Pro), then on every tick:
```python
self._parent.set_rgb_matrix(payload)   # -> self._parent._set_key_row(payload)  (ripple_effect.py:202-209)
self._parent.refresh_keyboard()        # -> self._parent._set_custom_effect()   (ripple_effect.py:211-215)
```
i.e. it drives the *exact same two commands* as Custom/`set_key_row` (§10), just repeatedly
and computed in software. **There is no `matrix_effect_ripple` sysfs file, no
`razer_chroma_*_ripple` builder anywhere in `razerchromacommon.{c,h}`, and no ripple-specific
kernel command at all** — confirmed by grep across the whole driver tree (zero `ripple` hits
outside a `logo` unrelated string). If Acheron wants a firmware-autonomous ripple (no host
involvement per keypress, as the map's Q3 decision implies for "reactive"), **that does not
exist for this effect** — Razer's own ripple is host-driven and would cost a steady
~25 Hz stream of 66-byte custom-frame writes for as long as it's active, a materially
different resource profile than every other effect in this file (all one-shot). Flag this
explicitly for ticket 02/03 — it changes ripple from "one more static effect byte table" to
"a decision about whether Acheron implements a streaming effect at all."

`payload` shape from `KeyboardColour.get_row_binary` (`keyboard.py:505-522`): `[row_id, 0x00,
columns-1]` + 3 bytes RGB per column — i.e. exactly the `matrix_custom_frame` wire shape in
§10, confirming ripple is implemented purely in terms of that command.

## 10. Custom effect / `set_key_row` — the per-key path

Two distinct commands, confirmed two-step from source:

### 10.1 `matrix_custom_frame` — write pixel data — `command_id 0x03`

`razer_chroma_extended_matrix_set_custom_frame(row_index, start_col, stop_col, rgb_data)` →
`razer_chroma_extended_matrix_set_custom_frame2(..., packetLength=0x47)`
(`razerchromacommon.c:746-776`):

```c
report = get_razer_report(0x0F, 0x03, data_length);   // data_length = 0x47 (fixed, see below)
report.arguments[2] = row_index;
report.arguments[3] = start_col;
report.arguments[4] = stop_col;
memcpy(&report.arguments[5], rgb_data, row_length);    // row_length = (stop_col+1-start_col)*3
```
Note `arguments[0]`/`[1]` are **left zero** — this command carries no `variable_storage` /
`led_id` concept at all (unlike every effect-select frame above).

Wired for the Pro at `razerkbd_driver.c:4134-4161`
(`razer_attr_write_matrix_custom_frame`, `USB_DEVICE_ID_RAZER_TARTARUS_PRO` grouped with
Tartarus V2/BlackWidow Elite/Ornata V2-V3/BlackWidow V3 family): `transaction_id.id = 0x1F`,
`want_response = true` (uses `razer_send_payload`, not the no-response variant some other
devices get). The handler loops, consuming `[row_id, start_col, stop_col, RGB...]` chunks from
the userspace write buffer until `count` bytes are exhausted (`:4088-4258`) — so a single sysfs
write can carry multiple rows back-to-back; for the Pro's `1×21` matrix there is only row `0`.

**`data_size` is a fixed `0x47` (71) regardless of actual row length** — `packetLength=0x47` is
passed explicitly by `razer_chroma_extended_matrix_set_custom_frame` and is non-zero, so the
`row_length + 5` fallback in `_set_custom_frame2` never triggers (`:751-766`, "most devices are
happy with 0x47" comment). For the Pro's full 21-column row (`start_col=0, stop_col=20`),
`row_length = 21*3 = 63` bytes of real RGB data at `arguments[5..67]`; the frame still declares
`data_size 0x47` (71 arg bytes) with the tail zero-padded (struct is zero-initialized).

| arg | value | meaning |
|---|---|---|
| a0 | `0x00` | unused (no varstore concept here) |
| a1 | `0x00` | unused (no led_id concept here) |
| a2 | `row_index` | `0x00` — the Pro's only row |
| a3 | `start_col` | `0x00` for a full-row write |
| a4 | `stop_col` | `0x14` (20) for a full-row write — **21 columns, 0-indexed inclusive** |
| a5.. | RGB triples | one per column from `start_col` to `stop_col` inclusive |

`data_size = 0x47` fixed. Daemon-level payload shape from `KeyboardColour.get_row_binary`
(`keyboard.py:505-522`): `bytes([row_id, 0x00, columns-1]) + 3 bytes-per-column`; for the Pro
`columns-1 = 20 = 0x14`, matching `stop_col` above exactly.

### 10.2 `matrix_effect_custom` — "arm"/display the frame — `command_id 0x02`, effect `0x08`

`razer_chroma_extended_matrix_effect_custom_frame()` (`razerchromacommon.c:697-706`):
```c
return razer_chroma_extended_matrix_effect_base(0x0C, 0x00, 0x00, 0x08);
```
**`variable_storage` and `led_id` are hardcoded to `0x00` here, for every device** — this is
the one effect-select frame in the whole family that does *not* pass `VARSTORE`/`BACKLIGHT_LED`
through; it's a generic "display whatever's in the custom frame buffer" trigger, not an
LED-addressed command. Wired for the Pro at `razerkbd_driver.c:3513-3537`
(`razer_attr_write_matrix_effect_custom`, grouped with Tartarus V2 etc.):
`transaction_id.id = 0x1F`. Daemon `set_custom_effect()` writes `b'1'` to sysfs
`matrix_effect_custom` (`device_base.py:1057-1068`, payload content ignored — pure trigger, same
pattern as `set_none_effect`/`set_spectrum_effect`).

| arg | value |
|---|---|
| a0 | `0x00` (not VARSTORE — hardcoded) |
| a1 | `0x00` (not BACKLIGHT_LED — hardcoded) |
| a2 | `0x08` effect id: custom |
| a3–a11 | `0x00` (all unused) |

`data_size = 0x0C` (12).

### 10.3 Confirmed: two-step sequence, write-then-arm

Both reference call sites in the daemon issue `matrix_custom_frame` write(s) **before**
`matrix_effect_custom`:

- The dbus surface exposes them as two separate calls — `setKeyRow` →
  `_set_key_row(payload)` (`device_base.py:1070-1088`, → `matrix_custom_frame`) and `setCustom`
  → `_set_custom_effect()` (`:1057-1068`, → `matrix_effect_custom`)
  (`chroma_keyboard.py:575-601`) — callers (GUI, scripts) are expected to push one or more
  `setKeyRow` writes, then a single `setCustom` to display them.
- `RippleManager` demonstrates the same order programmatically every tick:
  `self._parent.set_rgb_matrix(payload)` (→ `_set_key_row`) *then*
  `self._parent.refresh_keyboard()` (→ `_set_custom_effect`) (`ripple_effect.py:163-167`,
  `202-215`).

So: **yes, `matrix_effect_custom` must follow one or more `matrix_custom_frame` writes to take
effect** — confirmed from source, not inferred, matching the pattern the ticket named for
"some Razer devices."

### 10.4 Scroll wheel column index — **cannot be determined from source**

`MATRIX_DIMS = [1, 21]` (`keyboards.py:223`) says the Pro's matrix is logically 1 row × 21
columns — 20 grid keys + presumably the scroll wheel makes 21. But:

- The kernel's `matrix_custom_frame` handler (`razerkbd_driver.c:4078-4261`) is **fully
  generic** for the Pro: `row_id`/`start_col`/`stop_col`/RGB bytes are opaque values copied
  straight into the report with no per-device remap, no bounds table, no "column N is special"
  logic anywhere in the `USB_DEVICE_ID_RAZER_TARTARUS_PRO` case or its shared code path
  (contrast with, e.g., the `needslogohandling` special-case in `RippleEffectThread.run` for
  *other* devices with a 6×22 matrix and a logo LED at a fixed logical position,
  `ripple_effect.py:109-155` — the Pro's `1×21` shape triggers no equivalent branch anywhere).
- The daemon's only device-specific key-to-grid-cell table, `TARTARUS_KEY_MAPPING`
  (`keyboard.py:39-64`), is for the **classic Tartarus** (a different, non-Chroma, non-Pro
  product — 3 rows × 8 cols, different key layout, used only for `GamepadKeyManager` input
  *event* mapping in `misc/key_event_management.py:628`, never for RGB). It does not apply to
  the Pro, and grepping `keyboard.py` for `TARTARUS_PRO` returns **zero hits** — there is no
  Pro-specific key/LED position table anywhere in this file.
- No other file in the vendored daemon or driver tree mentions the Tartarus Pro's physical key
  order, scroll-wheel position, or any column-remap for its `1×21` matrix.

**Conclusion: the source is completely silent on which of the 21 columns is the scroll
wheel.** This is not a case where the driver treats it generically *and* documents the intent
elsewhere — it's a genuine gap. The map's own charting note ("the scroll wheel, not the Mode
key, is the 21st lit matrix cell") is stated as the user's own hardware knowledge, not
something recoverable from this source tree. **Settling the actual column index (grid-then-
wheel at index 20, or some other order) requires the real device** — send a single-column
`matrix_custom_frame` write (`start_col == stop_col == N`) for each `N` in `0..20` and observe
which physical LED lights, exactly the kind of sweep ticket 02 should run. Do not guess a
specific index in the spec without that hardware pass.

## 11. Brightness — `command_id 0x04` (set) / `0x84` (get)

`razer_chroma_extended_matrix_brightness(variable_storage, led_id, brightness)` =
`get_razer_report(0x0F, 0x04, 0x03)`, `arguments[0]=storage, [1]=led_id, [2]=brightness`
(`razerchromacommon.c:708-723`). `razer_chroma_extended_matrix_get_brightness(storage, led_id)`
= `get_razer_report(0x0F, 0x84, 0x03)`, `arguments[0]=storage, [1]=led_id`
(`:725-739`); response brightness comes back at `response.arguments[2]`
(`razerkbd_driver.c:4016-4018`, the non-blade-laptop branch).

**Tartarus Pro/V2-specific: targets `ZERO_LED` (`0x00`), not `BACKLIGHT_LED`.** Confirmed at
both call sites:
- Set: `razerkbd_driver.c:3689-3694` (`razer_attr_write_matrix_brightness`,
  `USB_DEVICE_ID_RAZER_TARTARUS_PRO`/`_V2` case): `razer_chroma_extended_matrix_brightness
  (VARSTORE, ZERO_LED, brightness)`, `transaction_id.id = 0x1F`.
- Get: `razerkbd_driver.c:3860-3865` (`razer_attr_read_matrix_brightness`, same case):
  `razer_chroma_extended_matrix_get_brightness(VARSTORE, ZERO_LED)`, `transaction_id.id =
  0x1F`.

Every other device in both switches (BlackWidow Lite, Ornata, Huntsman, BlackWidow V3/V4,
etc.) uses `BACKLIGHT_LED` for brightness — the Tartarus family (V2 and Pro both) is the
outlier. Both `matrix_brightness` sysfs files are created unconditionally for every
mouse-protocol-interface device (`razerkbd_driver.c:5009`, `CREATE_DEVICE_FILE(...,
&dev_attr_matrix_brightness)`), *outside* the per-model switch at `:5353` that lists the
Pro's other lighting files — so brightness is present for the Pro even though it isn't named
in that per-model block.

**Value range: `0x00–0xFF` on the wire (not 0–100).** The daemon does the 0–100 ↔ 0–255
mapping: `set_brightness(brightness: float 0-100)` clamps to `[0,100]`, then
`brightness = round(brightness * 255/100)` before writing the ASCII-decimal string to sysfs
(`chroma_keyboard.py:23-50`); `get_brightness()` returns the daemon's own persisted 0–100
value from `self.zone["backlight"]["brightness"]` (`:10-20`) rather than re-reading hardware —
i.e. the daemon does **not** call the `0x84` get path on every `getBrightness()` dbus call, it
trusts its own cache (parallel to the Status LEDs' authoritative-triple design ADR-0006
settled on, for the same "read-back isn't cheap/trustworthy to lean on" reasons).

| arg (set) | value | | arg (get) | value |
|---|---|---|---|---|
| a0 | `0x01` VARSTORE | | a0 | `0x01` VARSTORE |
| a1 | `0x00` ZERO_LED | | a1 | `0x00` ZERO_LED |
| a2 | brightness `0x00-0xFF` | | — | (2-byte request) |

`data_size = 0x03` both directions. Response brightness byte: `arguments[2]`
(struct index; **not** the a6/a7/a8 slots the Status LED read-back uses — different response
shape, single scalar not RGB).

---

## 12. Summary table

| effect | `cmd_id` | effect byte | `data_size` | `led_id` | `txn_id` | notes |
|---|---|---|---|---|---|---|
| static | `0x02` | `0x01` | `0x09` | `0x05` | `0x1F` | |
| none | `0x02` | `0x00` | `0x06` | `0x05` | `0x1F` | |
| spectrum | `0x02` | `0x03` | `0x06` | `0x05` | `0x1F` | |
| wave | `0x02` | `0x04` | `0x06` | `0x05` | `0x1F` | direction default `(1,2)` |
| reactive | `0x02` | `0x05` | `0x09` | `0x05` | `0x1F` | |
| breath random | `0x02` | `0x02` | `0x06` | `0x05` | **`0x3F`** | quirk, §7 |
| breath single | `0x02` | `0x02` | `0x09` | `0x05` | **`0x3F`** | quirk, §7 |
| breath dual | `0x02` | `0x02` | `0x0C` | `0x05` | **`0x3F`** | quirk, §7 |
| starlight random | `0x02` | `0x07` | `0x06` | `0x05` | `0x1F` | |
| starlight single | `0x02` | `0x07` | `0x09` | `0x05` | `0x1F` | |
| starlight dual | `0x02` | `0x07` | `0x0C` | `0x05` | `0x1F` | |
| ripple / ripple-random | — | — | — | — | — | **not a device command**, §9 |
| custom-frame write (`set_key_row`) | `0x03` | n/a | `0x47` fixed | n/a (a0/a1 unused) | `0x1F` | §10.1 |
| custom effect arm | `0x02` | `0x08` | `0x0C` | `0x00` (hardcoded, not `0x05`) | `0x1F` | §10.2 |
| brightness set | `0x04` | n/a | `0x03` | `0x00` (`ZERO_LED`) | `0x1F` | §11 |
| brightness get | `0x84` | n/a | `0x03` | `0x00` (`ZERO_LED`) | `0x1F` | §11 |

All VARSTORE (`arg0 = 0x01`) except the custom-effect-arm frame (hardcoded `0x00`/NOSTORE, and
that same frame's `led_id` is hardcoded `0x00` too — see §10.2).

---

## 13. What's implementation-ready vs. what needs hardware

**Implementation-ready straight from source** (11 of 13 effects, both custom-path commands,
both brightness directions): static, none, spectrum, wave, reactive, starlight
(random/single/dual), custom-frame write + custom-effect arm (mechanics and sequencing only —
not the column mapping, see below), brightness get/set. Every byte is pinned to a specific
driver function and confirmed wired for `USB_DEVICE_ID_RAZER_TARTARUS_PRO` specifically (not
inferred from a sibling device).

**Needs hardware verification:**
1. **Breath's transaction_id** — driver sends `0x3F` due to what looks like a copy/paste bug
   (§7); confirm the frame actually lands correctly, and whether `0x1F` also works (probably,
   given every other effect uses it, but untested here).
2. **Ripple's cost/shape as a streaming effect** — confirmed from source it's software-driven
   (§9), not a byte-table gap, but ticket 02/03 need to decide whether Acheron implements a
   ~25 Hz custom-frame stream at all, given the map's existing ADR-0006 "occasional one-shot
   write" design assumption for the `led` task.
3. **The scroll wheel's column index within the 1×21 matrix** — genuinely undetermined by any
   source in this tree (§10.4). Requires a single-column sweep on the real device.

Nothing else in this file is a source-analysis guess; every byte table above traces to a named
function and line/case in the vendored driver or daemon.

---

## 14. Sources (all primary, all local)

- `/usr/src/openrazer-driver-3.12.4/driver/razerkbd_driver.c` — per-effect `USB_DEVICE_ID_
  RAZER_TARTARUS_PRO` sysfs handlers: `razer_get_report_params` (`:338-382`),
  `razer_attr_write_matrix_effect_none` (`:2079`, Pro case `:2110-2136`),
  `razer_attr_write_matrix_effect_wave` (`:2238`, Pro case `:2280-2303`),
  `razer_attr_write_matrix_effect_spectrum` (`:2410`, Pro case `:2485-2491`),
  `razer_attr_write_matrix_effect_reactive` (`:2569`, Pro case `:2605-2630`),
  `razer_attr_write_matrix_effect_static` (`:2720`, Pro case `:2866-2873`),
  `razer_attr_write_matrix_effect_starlight` (`:2933`, Pro case `:3034-3054`),
  `razer_attr_write_matrix_effect_breath` (`:3145`, Pro case `:3167-3196`, transaction-id
  quirk),
  `razer_attr_write_matrix_effect_custom` (`:3485`, Pro case `:3513-3537`),
  `razer_attr_write_matrix_custom_frame` (`:4078`, Pro case `:4134-4161`),
  `razer_attr_write_matrix_brightness` (`:3681`, Pro case `:3689-3694`),
  `razer_attr_read_matrix_brightness` (`:3852`, Pro case `:3860-3865`, response byte `:4016-
  4018`), unconditional `matrix_brightness` file creation (`:5009`), Pro's per-model lighting
  file list (`:5353-5364`).
- `/usr/src/openrazer-driver-3.12.4/driver/razerchromacommon.{c,h}` — frame builders:
  `_effect_base` (`:481-490`), `_effect_none` (`:498-501`), `_effect_static` (`:511-520`),
  `_effect_wave` (`:531-544`), `_effect_starlight_random/single/dual` (`:558-597`),
  `_effect_spectrum` (`:605-608`), `_effect_reactive` (`:639-652`),
  `_effect_breathing_random/single/dual` (`:662-695`), `_effect_custom_frame` (`:703-706`),
  `_matrix_brightness` / `_get_brightness` (`:714-739`), `_set_custom_frame` /
  `_set_custom_frame2` (`:746-776`).
- `/usr/src/openrazer-driver-3.12.4/driver/razercommon.{c,h}` — `struct razer_report`
  (`:135-143`), LED-id constants incl. `ZERO_LED`/`BACKLIGHT_LED`/`SIDE_STRIPE_LED`
  (`:48-58`), `VARSTORE`/`NOSTORE` (`:44-45`); CRC/report-struct mechanics already nailed down
  by the Status LEDs' research file, not re-derived here.
- `/usr/lib/python3/dist-packages/openrazer_daemon/hardware/keyboards.py` — `RazerTartarusPro`
  (`:211-231`: `METHODS` list, `HAS_MATRIX`, `MATRIX_DIMS = [1, 21]`, no `WAVE_DIRS` override);
  `_RippleKeyboard` (`:40-71`).
- `/usr/lib/python3/dist-packages/openrazer_daemon/dbus_services/dbus_methods/
  chroma_keyboard.py` — every `set_*_effect`/`get_brightness`/`set_brightness`/`set_key_row`
  dbus handler and its exact sysfs payload construction (full file read; see per-effect
  sections above for line numbers).
- `/usr/lib/python3/dist-packages/openrazer_daemon/hardware/device_base.py` — `_set_custom_
  effect` (`:1057-1068`), `_set_key_row` (`:1070-1088`), default `WAVE_DIRS = (1, 2)` (`:45`).
- `/usr/lib/python3/dist-packages/openrazer_daemon/keyboard.py` — `KeyboardColour`
  (`:405-554`: `get_row_binary`/`get_total_binary` payload shape), `TARTARUS_KEY_MAPPING`
  (`:39-64`, classic-Tartarus-only, confirmed unused for the Pro).
- `/usr/lib/python3/dist-packages/openrazer_daemon/misc/ripple_effect.py` — `RippleEffectThread`
  (`:16-170`), `RippleManager` (`:173-257`); confirms ripple is a software-streamed
  `matrix_custom_frame`/`matrix_effect_custom` loop, not a device command.
