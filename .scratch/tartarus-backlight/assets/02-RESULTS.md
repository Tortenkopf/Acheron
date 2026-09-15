# Ticket 02 hardware verification — raw run log (2026-09-15)

HITL session: agent drove `prototype/02-backlight-verification/prototype.py`,
human (Charon) watched the Custom-layout grid, the scroll wheel, and the
Mode key/thumbstick on the real Tartarus Pro (firmware **v1.2**, serial
PM2443F36300141 — the same unit `tartarus-status-leds` ticket 01 used) and
reported each state live in conversation. Machine-side evidence (every frame
sent, every `0x84` brightness read-back, every post-write presence check) is
in `02-fill.jsonl` / `02-sweep.jsonl` / `02-effects.jsonl` /
`02-brightness.jsonl`; human observations are transcribed below. This effort
is **ungated** (map: no kill-gate) — this is a plain verification pass, not
a criterion that could archive the effort.

All byte layouts are ticket 01's (`research/backlight-wire-protocol.md`);
none needed correction. `BACKLIGHT_LED = 0x05` throughout except brightness
(`ZERO_LED = 0x00`) and the custom-effect-arm frame (hardcoded `0x00`/`0x00`,
not VARSTORE/BACKLIGHT_LED) — all exactly as documented.

## Q6 — which cells light, physically → **CONFIRMED, and stronger than expected**

`02-fill.jsonl`: solid green sent via the Custom-layout two-step
(`matrix_custom_frame` all 21 columns, then `matrix_effect_custom` arm).
Human: "All 20 + the mouse wheel green." Mode key and thumbstick confirmed
dark — and not merely unlit: **"They are solid black plastic"**, i.e. not
RGB-capable at all, a stronger guarantee than "inert" for the spec (there is
no possible byte value that would light them; the Lighting tab's
physical-layout fidelity for these two elements is purely cosmetic).

## Matrix column-addressing order → **SETTLED, and corrects the charting assumption**

Research §10.4 flagged this as genuinely undetermined from source. Three
rounds of `custom-cols` narrowed it live (`02-sweep.jsonl`):

| frame sent | human observation |
|---|---|
| `0-9:ff0000,10-20:0000ff` | keys 1–10 red, keys 11–20 blue, **wheel blue** |
| `0-9:000000,10-19:00ff00,20:ff0000` | keys 1–10 dark, keys **11–19 + wheel** green, **key 20 red** |
| `19:0000ff` alone | only the wheel lit blue — everything else (incl. key 20) dark |
| `0:ff0000,9:00ff00,20:0000ff` | key 1 red, key 10 green, **key 20 blue** |

**Column 0–18 = keys 1–19 in wire order; column 19 = the scroll wheel;
column 20 = key 20.** This corrects the map's charting note ("the scroll
wheel... is the 21st lit matrix cell") — the wheel is **second-to-last**
in wire order, not last; key 20 occupies the final column, after the wheel.
Directionality also confirmed forward (column 0 → key 1, not key 20) via the
same isolation frame. The spec's column-to-physical-cell table should read
`0..18 → grid keys 1..19`, `19 → scroll wheel`, `20 → grid key 20` — not the
grid-then-wheel-last order everyone assumed while charting.

## Fixed effects — wave, breathing (both transaction_ids), static → **all PASS, no driver mode needed**

`02-effects.jsonl`. Every effect frame was sent with `device_mode` reading
`00 00` (normal) immediately beforehand — confirms research's prediction
that, like the Status LEDs, no `set_device_mode` call is needed for any
backlight command.

- **Wave** (`direction=1`): human confirmed it was running, and that analog
  keys/capture kept working normally alongside it (no Capture disruption).
- **Breathing, `transaction_id 0x3F`** (what the driver actually transmits
  per research §7's flagged copy/paste bug): ran correctly (orange,
  fading) from a clean `00 00` state.
- **Breathing, `transaction_id 0x1F`** (the "standard" every other effect
  uses): also ran correctly (purple, fading) — **both transaction ids work**.
  The driver's mismatched `0x1F` dead-store is confirmed harmless: the
  firmware does not appear to validate `transaction_id` for Breath. The spec
  is free to always send `0x1F` for consistency with every other effect
  rather than replicating the driver's `0x3F` quirk.
- **Breathing, dual** (`ff0000`/`0000ff`, `txn 0x1F`): confirmed fading
  between red and blue.
- **Breathing, random** (`txn 0x1F`): confirmed breathing through random
  colours on its own, no colour argument needed.
- **Static** (`00ffff`): solid, non-animated cyan confirmed.

**One genuine surprise mid-session:** `device_mode` was briefly seen at
`03 00` (driver mode) between the wave and breathing writes. Traced live:
not a byte-layout issue, not device instability — the human had started the
real `acheron-daemon` + GUI to answer the "does analog capture still work"
question, and the daemon's own capture supervisor enters driver mode for
analog polling (ADR-0006), independent of anything this prototype sent. The
device tolerated the transition cleanly (stayed present, `relock` via the
already-verified `prototype/01-status-leds/prototype.py relock` path
returned it to `00 00` without incident), and backlight effects/writes
continued to work correctly both before and after — a positive coexistence
finding, not a concern. **Housekeeping note for future hardware sessions on
this unit:** `openrazer-daemon` (a systemd `--user` service, enabled and
running independently of Acheron) was also active at session start and was
stopped for the raw-hidraw portions of this pass to avoid a race with its
own device polling/writes, then restarted at the end — it was not, in the
end, the source of the mode flip, but running it concurrently with direct
hidraw testing is still worth avoiding on principle.

## Brightness → **CONFIRMED 0x00–0xFF range, independent off primitive**

`02-brightness.jsonl`. `0x84` get and `0x04` set both round-tripped exactly
(every set's immediate get echoed the same byte):

| value sent | read-back | human observation |
|---|---|---|
| (initial, no set) | `0x7f` (127) | — |
| `0x28` (40) | `0x28` | "noticeably dimmer" |
| `0xff` (255) | `0xff` | "max brightness" |
| `0x00` (0) | `0x00` | fully dark |

Confirms research §11's prediction against the daemon's own 0–100 mapping:
the wire value really is a full **0x00–0xFF** byte, not 0–100. **Brightness
`0x00` alone blacks out the matrix, independent of the active effect/colour
state** — a simpler "off" primitive than a zero-channel static frame or
`effect_none` (which the Status LEDs' ticket 01 found does *not* work as an
off — untested here for backlight, but brightness `0x00` is proven and
sufcient, so the spec doesn't need to resolve that ambiguity for Lighting).

## Adverse behaviour / destabilization → **PASS**

Every `frame_sent` in all four JSONL logs was followed by a `presence_check`
that logged `device still present, nodes unchanged`, across ~15 feature
writes (custom-frame writes + arms, 3 fixed-effect selects incl. two
breathing transaction_id variants, 4 brightness sets) plus one deliberate
`set_device_mode` relock. No re-enumeration, no vanish, no reset loop at any
point — including through the driver-mode excursion caused by the
concurrent `acheron-daemon` capture session. The ADR-0006 caution is
specifically about the `set_device_mode` transition itself, which this tool
never initiates for backlight purposes; the one relock in this session used
the already-verified `prototype/01-status-leds/prototype.py relock` frame
and behaved exactly as it did in that earlier session.

## Bottom line for ticket 03/04

- Q6, the column-addressing order, and the brightness range are now all
  hardware-confirmed — nothing byte-level remains open from ticket 01's
  "needs hardware" list except Breath's transaction_id choice, which this
  session also closed (either works; use `0x1F` for consistency).
- The **scroll-wheel column index correction** (`19`, not `20`) is the one
  finding that changes a stated fact on the map (Q6's charting note) and
  should be reflected there, not just left in this file.
