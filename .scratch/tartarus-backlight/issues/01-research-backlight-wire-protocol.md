Type: research
Blocked by: —
Status: resolved

## Question

Nail an implementation-ready wire spec for every Lighting effect this effort covers, the same
way [`tartarus-status-leds/research/status-led-wire-protocol.md`](../../tartarus-status-leds/research/status-led-wire-protocol.md)
did for the Status LEDs. The Tartarus Pro's OpenRazer device entry (`openrazer_daemon/hardware/keyboards.py`,
`RazerTartarusPro`) lists: `set_static_effect`, `set_spectrum_effect`, `set_reactive_effect`,
`set_none_effect`, `set_breath_random_effect`, `set_breath_single_effect`, `set_breath_dual_effect`,
`set_custom_effect`, `set_key_row`, `set_wave_effect`, `set_starlight_random_effect`,
`set_starlight_single_effect`, `set_starlight_dual_effect`, `set_ripple_effect`,
`set_ripple_effect_random_colour`, `get_brightness`, `set_brightness`. `HAS_MATRIX = True`,
`MATRIX_DIMS = [1, 21]`.

Primary sources — all vendored locally, no live device needed for this ticket:

- `/usr/src/openrazer-driver-3.12.4/driver/razerkbd_driver.c` (the sysfs store/show handlers
  per effect, and the `USB_DEVICE_ID_RAZER_TARTARUS_PRO` device-file wiring switch — already
  confirmed to route through `razer_chroma_extended_matrix_effect_*`, `BACKLIGHT_LED`,
  `transaction_id 0x1F`, `VARSTORE`, report_index `0x02`/response_index `0x02`, and to skip the
  normal-mode reset call the way Acheron's own ADR-0006 independently found)
- `/usr/src/openrazer-driver-3.12.4/driver/razerchromacommon.{c,h}` (the frame-builder
  functions themselves — argument layout per effect)
- `/usr/src/openrazer-driver-3.12.4/driver/razercommon.{c,h}` (`struct razer_report`, CRC)
- `/usr/lib/python3/dist-packages/openrazer_daemon/hardware/keyboards.py` (confirms which
  effects are wired for *this* device specifically, vs. the driver's full superset)

For each effect, produce a byte table: `command_class`/`command_id`, the numeric
`BACKLIGHT_LED` id (must differ from the Status LEDs' `0x0B` — confirm the actual value),
argument layout, `data_size`, `transaction_id`, VARSTORE vs NOSTORE. Specifically settle:

- **Static / spectrum / none / breath (random/single/dual) / wave / starlight
  (random/single/dual) / ripple (+ random-colour variant) / reactive** — one static
  "select this effect" frame each; capture every argument (colour(s), direction, speed/duration
  where applicable).
- **Custom + `set_key_row`** — the per-key path. How the 1×21 matrix is addressed (row index
  fixed at 0? column range 0–20?), whether `matrix_effect_custom` must be sent *after* one or
  more `set_key_row` writes to arm them (some Razer devices require this two-step sequence —
  confirm from `razerkbd_driver.c`'s `matrix_custom_frame_store` / `matrix_effect_custom_store`),
  and — critical for this effort — **which column index the scroll wheel's cell occupies**
  relative to the 20 grid keys (grid-then-wheel at index 20, or something else). Cross-reference
  any Tartarus-Pro-specific row/col special-casing in the store functions; if the driver treats
  the Pro generically with no per-device remap table, say so explicitly (it may mean the
  addressing order can only be settled with the real device — feed that forward to ticket 02
  rather than guessing).
- **`get_brightness` / `set_brightness`** — value range (0–255 vs 0–100) and frame shape.

Output: `.scratch/tartarus-backlight/research/backlight-wire-protocol.md`, one byte table per
effect, every claim cited to file + line. Then an `## Answer` here summarizing which frames are
implementation-ready straight from source vs. which need the real device to confirm (feeds
ticket 02).

## Answer

**Implementation-ready for 11 of 13 effects, both custom-path commands, and both brightness
directions.** Full write-up with primary-source citations (OpenRazer 3.12.4
`razerkbd_driver.c` per-effect Tartarus-Pro `switch` cases, `razerchromacommon.{c,h}` frame
builders, daemon `chroma_keyboard.py`/`device_base.py`/`keyboard.py`/`ripple_effect.py`):
[`../research/backlight-wire-protocol.md`](../research/backlight-wire-protocol.md).

All effects ride `command_class 0x0F`, LED id **`BACKLIGHT_LED = 0x05`** (confirmed different
from the Status LEDs' `SIDE_STRIPE_LED = 0x0B`), `transaction_id 0x1F`, VARSTORE, on the same
`report/response_index 0x02` the Status LEDs use. Byte tables for static, none, spectrum, wave,
reactive, breath (random/single/dual), starlight (random/single/dual), the custom-frame write +
custom-effect arm, and brightness get/set are all pinned to a named driver function and
confirmed wired specifically for `USB_DEVICE_ID_RAZER_TARTARUS_PRO` (not inferred from a
sibling device).

Three things surprised me enough to flag loudly:

1. **Ripple and ripple-random-colour are not device-side commands at all.** The daemon's dbus
   handlers for them do no sysfs write — they only cache state and fire a notification that a
   background Python thread (`RippleEffectThread`, ~25 Hz) picks up, computing the ripple in
   software and streaming it as repeated `matrix_custom_frame` + `matrix_effect_custom` writes
   (the same two commands the Custom-layout path uses). There is no `razer_chroma_*_ripple`
   builder anywhere in the driver. **This changes ripple from "one more static effect byte
   table" into a scope/cost decision** — a firmware-autonomous ripple doesn't exist for this
   device; a host-driven one costs a steady ~25 Hz write stream for as long as it's active,
   which conflicts with ADR-0006's "occasional one-shot write" assumption for the `led` task.
   Ticket 03 needs to decide whether Acheron implements streamed ripple at all, or drops it
   from the Fixed-effect list.
2. **A real driver quirk in Breath**: the Tartarus Pro/V2 breathing handler builds the frame
   with `transaction_id = 0x3F`, sends it, and only *then* overwrites the field to `0x1F` — a
   dead store, no second send. Every other Tartarus Pro effect sends with `0x1F`; Breath is the
   only one that actually transmits `0x3F`. Reads like a copy/paste bug in the driver, but it's
   what ships — worth a quick hardware check (confirm `0x3F` works, and whether `0x1F` also
   works) rather than silently "fixing" it to match the other effects.
3. **Brightness targets `ZERO_LED` (`0x00`), not `BACKLIGHT_LED`** — confirmed at both the set
   and get call sites for Tartarus Pro/V2 specifically; every other keyboard in both switches
   uses `BACKLIGHT_LED` for brightness. Wire value range is `0x00–0xFF`; the daemon does the
   0–100 ↔ 0–255 mapping and trusts its own cached value rather than round-tripping the `0x84`
   get on every read (same "own an authoritative value, don't lean on read-back" shape ADR-0006
   already settled for the Status LEDs).

**What still needs the real device** (feeds ticket 02):
- **The scroll wheel's column index within the 1×21 Custom-layout matrix.** Genuinely
  undetermined from source — the kernel's custom-frame handler is fully generic for this
  device (no Tartarus-Pro-specific remap table anywhere), and the daemon's only Tartarus
  key/position table (`TARTARUS_KEY_MAPPING`) is for the *classic* Tartarus's input events, is
  never used for RGB, and doesn't apply to the Pro at all (zero `TARTARUS_PRO` references in
  `keyboard.py`). This is not a guess to avoid making — the source is silent, full stop.
  Ticket 02 needs a single-column sweep (`start_col == stop_col == N` for `N` in `0..20`) on
  the real unit to find which physical cell is the wheel.
- **Breath's `0x3F` transaction_id** — confirm it actually works on hardware (it's what the
  driver sends today) and whether `0x1F` is also accepted.
- **Ripple's viability/cost as a streamed effect** — a product/scope call more than a wire-byte
  question, but it needs a decision, not just a byte table.

The custom-frame write-then-arm two-step (`matrix_custom_frame` writes must precede
`matrix_effect_custom` to display) **is** confirmed from source — both the dbus surface
(`setKeyRow` then `setCustom` as separate calls) and `RippleManager`'s per-tick order
demonstrate it directly, no hardware needed for that part.
