# Ticket 01 prototype — raw run log (2026-09-02)

HITL session: agent drove `prototype/01-status-leds/prototype.py`, human (Charon)
watched the three side LEDs on the real Tartarus Pro (firmware **v1.2**, serial
PM2443F36300141) and reported each state. Machine-side evidence (every frame sent,
every `0x82` read-back, every post-write presence check) is in the `*.jsonl`
files; the human observations are transcribed below.

Device started in normal mode, orange LED lit (map grounding state).

## Criterion 1 — independent control  →  **PASS**

`01-sequence.jsonl`. Entered driver mode once (`device_mode 00 00 → 03 00`, no
re-enumeration), then walked all 8 combinations with `arg0=0x01` VARSTORE,
`arg4=0x01`, `txn 0x1F`:

| frame RGB | read-back (o,g,b) | human saw | match |
|-----------|-------------------|-----------|-------|
| `00 00 00` | `00 00 00` | all three dark (orange went off) | ✅ |
| `FF 00 00` | `ff 00 00` | orange lit, green+blue dark | ✅ |
| `00 FF 00` | `00 ff 00` | green lit, orange+blue dark | ✅ |
| `00 00 FF` | `00 00 ff` | blue lit, orange+green dark | ✅ |
| `FF FF 00` | `ff ff 00` | orange+green lit, blue dark | ✅ |
| `FF 00 FF` | `ff 00 ff` | orange+blue lit, green dark | ✅ |
| `00 FF FF` | `00 ff ff` | green+blue lit, orange dark | ✅ |
| `FF FF FF` | `ff ff ff` | all three lit | ✅ |

All three LEDs are independently host-addressable via
`0x0F/0x02`, LED `0x0B`, static effect. Every combination lit exactly as
commanded. **The effort is not killed.**

## Criterion 2 — read-back  →  **works, and is trustworthy on this unit**

Ticket 02 §3.3 flags read-back as unreliable across Razer devices (z3ntu rejected
the GET call, plxty never got it working, it's why PR #2336 stalled). We tested
this directly on our unit — `01-coldread.jsonl`:

| scenario | samples | result |
|----------|---------|--------|
| read immediately after our own write | ~20 | every one matched the written value, first try |
| **cold read, no preceding host write** (after replug, firmware showing orange-only) | 4 | all `ff 00 00` — the *actual firmware state*, **not** the stale `00 ff ff` we set before the replug, not garbage |
| write, wait 5 s, then read | 1 | matched (`00 ff 00`) |

So on the Tartarus Pro / fw v1.2, `0x82` returns the true current channel state —
including the "seed daemon state at startup" case ticket 02 says is GET's only
safe use. **ticket 02's caution does not bite on our hardware.** The spec should
still keep the daemon's authoritative-triple design (safe cross-device, and the
daemon must write unconditionally on connect anyway — see criterion 5 notes), but
a startup `0x82` seed is reliable here if ever wanted.

Echo normalisation: the device rewrites the effect args in its response —
`arg3=0x00 arg4=0x00 arg5=0x01` regardless of what was sent in `arg4`, and
**`arg0` comes back `0x01` (VARSTORE) even when `0x00` (NOSTORE) was sent**, once
the state has settled (~seconds). Suggests the firmware may not honour the
NOSTORE/VARSTORE distinction at all (consistent with neither value persisting —
see below).

## Criterion 3 — on-device keymap switch  →  **no clobber; N/A on this unit**

`01-keymap.jsonl`. Set `FF FF FF`, human triggered every on-device combo they
could, LEDs **stayed lit**, read-back still `ff ff ff`. Human's assessment: the
Tartarus Pro has **no host-independent on-device keymap switch** — the
LED↔keymap linkage is Synapse-side only. So the "firmware re-asserts and clobbers
host state on an on-device keymap switch" scenario **cannot occur** — there is no
on-device keymap switch to trigger it. (Firmware *does* assert its own default on
boot — see criterion 5.)

## Criterion 4 — no adverse behaviour  →  **PASS**

Across ~20 feature writes + ~18 read-backs + a driver-mode enter + a relock, the
`presence` check after every single write logged `device still present, nodes
unchanged`. The device never left the bus, never re-enumerated mid-session, no
reset loop. `dmesg` clean. The PR #2710 driver-mode reset caution did **not**
reproduce on this unit (consistent with tickets 13/16). Driver mode was entered
and exited cleanly (`00 00 → 03 00 → 00 00`).

## Criterion 5 — is all-off reachable  →  **YES**

Sent `00 00 00` twice (`01-sequence.jsonl` steps 1 and 9), both times the human
confirmed **all three LEDs fully dark**, including turning off the
previously-persistent orange LED. All-off is a hardware-reachable state via the
static frame. → the "clear all LEDs on clean daemon exit" decision **stands**.

`effect_none` (`effect 0x00`, `data_size 0x06`) on LED `0x0B`: `01-off.jsonl` /
`01-arg4-zero.jsonl` — device ACKed `status=0x02` but **LEDs did not change** and
read-back was unchanged. Confirms ticket 02: **do not use `effect_none`**; off =
static frame with channel bytes `0x00`.

## Bonus checks handed down from ticket 02

**No driver mode needed** (`01-nodriver.jsonl`): with `device_mode = 00 00`
(genuinely normal mode), a bare LED write returned `status=0x02` and the human
confirmed green lit. **The frame needs no device-mode call** — confirms ticket 02
§5 on real hardware, contradicts grounding §3's "enable driver mode first".

**Neither NOSTORE nor VARSTORE persists across a re-enumeration** — and the
firmware may not honour the distinction at all:

- `01-nostore.jsonl`: `arg0=0x00` accepted (`status=0x02`), blue lit, read-back
  `00 00 ff`. Unplug/replug → **only orange lit**.
- `01-varstore-persist.jsonl`: `arg0=0x01`, green lit, unplug/replug → **only
  orange lit**.
- `01-coldread.jsonl`: after a NOSTORE write, the settled read-back echoes
  `arg0=0x01` — the firmware reports its state as VARSTORE whatever we sent.

On this unit the side LEDs return to the firmware keymap-indicator default
(orange only) on **every** USB enumeration, regardless of storage mode or last
host write. → ticket 02 §6's "stale *host* state flashes on boot" concern is
**moot on this hardware** (the flash is the firmware default, not our state);
the daemon must assert on startup + every reconnect regardless. `arg0` choice is
**cosmetic on this unit** — send `0x00` for intent-clarity, but expect a flash
write on every change either way (minor: the indicator only changes on Profile
switch).

**`arg4=0x00`** (`01-arg4-zero.jsonl`): ticket 02 §8.1's exact recommended frame
(`01 0B 01 00 00 01 r g b`, NOSTORE) — `status=0x02`, human confirmed
orange+blue. Both `arg4=0x00` and `arg4=0x01` verified working; the device
ignores the slot.

Device left restored to orange-only, normal mode.
