Type: prototype
Blocked by: —
Status: resolved (Charon, 2026-09-02)

## Question

Build a standalone Python harness — under `prototype/<NN>-status-leds/`, independent of
Acheron's daemon/GUI — that attempts to actually drive the three side **Status LED**s on the
real, connected Tartarus Pro, using the frame documented in
[`research/tartarus-pro-status-leds.md`](../research/tartarus-pro-status-leds.md) §3
(`command_class 0x0F`, `command_id 0x02` write / `0x82` read, **LED ID `0x0B`**, effect =
static, `VARSTORE`; channel bytes at args 6/7/8).

Model it on [`prototype/13-analog-grid-capture/prototype.py`](../../../prototype/13-analog-grid-capture/prototype.py)
— its own `hidraw` discovery (Interface 2 / `CONTROL_INTERFACE`), CRC, and `HIDIOCSFEATURE`
ioctl. The daemon already enters driver mode routinely, so the harness may simply do the same
in its own setup (research §3 / charting Q8); no special caution beyond what analog capture
already lives with.

**This is a feasibility test — HITL, needs the physical device and a human watching the LEDs
in real time.** It is the effort's **kill-gate**: a written negative result on criterion 1
archives the effort.

Settle at least:

1. **Independent control.** Does each of the three LEDs (orange / green / blue) turn on and
   off independently via the `0x0F/0x02`, LED `0x0B`, static-effect frame? Confirm all three
   channels and combinations visually.
2. **Read-back.** Does the `0x82` read frame report back the channel values that were set?
3. **On-device keymap switch.** Set all three LEDs via the frame, then trigger the keypad's
   *own* on-device keymap-switch combo, then read back (`0x82`) and watch visually — does the
   firmware re-assert its own keymap-indicator code and clobber the host-set state? (research
   open question #1; feeds the "re-assert hook" fog item.)
4. **No adverse behaviour.** No reset / re-enumeration / reset-loop on this unit across
   repeated LED writes (cross-check research §6's PR #2710 caution against our own hardware).
5. **Is all-off reachable?** Send `(0, 0, 0)` and observe — is all-three-dark a state the
   hardware actually accepts, or does the firmware force at least one LED on? (Decides the
   shutdown-clear fog item.)

Also note any byte-level deviation from research §3 found on this unit/firmware — in
particular `arg3`/`arg4` (CommandPost sends `00 01`, OpenRazer's helper `00 00`) and whether
`effect_none` (`effect` id `0x00`, `data_size 0x06`) is a cleaner "off" than a static frame
with zero channels. Coordinate with [ticket 02](./02-research-status-led-wire-protocol.md),
which runs in parallel on the same ambiguities.

Deliverable: the prototype under `prototype/<NN>-status-leds/`, raw evidence under this map's
`assets/`, and an `## Answer` recording every criterion's result. If criterion 1 fails,
the Answer is the effort's negative result.

## Answer

**KILL-GATE: PASSED. The effort proceeds.** The three side Status LEDs are fully
host-controllable on the real Tartarus Pro (firmware v1.2, serial PM2443F36300141) via the
`0x0F/0x02`, LED `0x0B`, static-effect frame. No adverse device behaviour. A spec should be
written.

- **Prototype:** [`prototype/01-status-leds/prototype.py`](../../../prototype/01-status-leds/prototype.py)
  (standalone, stdlib-only, `selftest` cross-checks every protocol constant — 50/50). Lives
  under `prototype/` on `dev`, same as ticket 13's analog harness — the release rebuild keeps
  `prototype/` and `.scratch/` out of `main`, so it is already "out of main" without a
  separate branch.
- **Raw evidence:** [`assets/01-RESULTS.md`](../assets/01-RESULTS.md) (human-readable run log)
  + `assets/01-*.jsonl` (every frame sent, every `0x82` read-back, every post-write presence
  check). HITL session: agent drove the harness, Charon watched the physical LEDs.

### Criteria

| # | Criterion | Result |
|---|-----------|--------|
| 1 | Independent control | **PASS** — all 8 on/off combinations lit exactly as commanded; each LED independently addressable; the persistent orange LED was turned off under host control. |
| 2 | Read-back | **Works and is trustworthy on this unit** — tested the case ticket 02's caution is actually about: 4 *cold* reads after a replug (no preceding host write) all returned the true firmware state `ff 00 00`, not the stale `00 ff ff` set before the replug, not garbage. Plus ~20 immediate-after-write reads and a write→5s→read, all matched. ticket 02's cross-device "unreliable GET" caution **does not bite on fw v1.2**. Spec still keeps ticket 02's authoritative-triple design (safe cross-device + daemon must write unconditionally on connect anyway), but a startup `0x82` seed is reliable here. |
| 3 | On-device keymap switch | **No clobber — N/A on this unit.** The Tartarus Pro has no host-independent on-device keymap switch (LED↔keymap link is Synapse-side only). Host-set state held through every combo Charon could trigger. The "re-assert after on-device keymap change" hook is **not needed**. (Firmware *does* assert its own default on boot — see criterion 5.) |
| 4 | No adverse behaviour | **PASS** — ~20 writes + ~18 reads + driver-mode enter + relock; device never left the bus, never re-enumerated, no reset loop, `dmesg` clean. PR #2710's driver-mode caution did not reproduce (as in tickets 13/16). |
| 5 | All-off reachable | **YES** — `(0,0,0)` static frame leaves all three fully dark, confirmed twice. The "clear all LEDs on clean daemon exit" decision **stands** (the contingency in Q6/Q13 does not trigger). |

### Wire-protocol cross-check (prototype vs ticket 02's [`status-led-wire-protocol.md`](../research/status-led-wire-protocol.md))

Every byte ticket 02 derived from source was exercised against real hardware. **They agree.**
The confirmed, correct frame is:

```
build_razer_cmd(txn=0x1F, class=0x0F, id=0x02, data_size=0x09,
                args = [ arg0, 0x0B, 0x01, 0x00, arg4, 0x01, R, G, B ])
                         ^storage        ^static      ^colour count
```

| Field | ticket 02 (from source) | prototype (on hardware) | verdict |
|-------|-------------------------|-------------------------|---------|
| `txn` | `0x1F` | `0x1F` accepted first try; `0x01`/`0xFF` never needed | ✅ agree |
| `class/id` | `0x0F` / `0x02` write, `0x82` read | both confirmed | ✅ agree |
| `data_size` | `0x09` | `0x09` accepted (`status=0x02`) | ✅ agree |
| `arg1` LED id | `0x0B` | `0x0B` | ✅ agree |
| `arg2` effect | `0x01` static | `0x01` | ✅ agree |
| `arg3` | `0x00` | `0x00` | ✅ agree |
| `arg4` | `0x00` (CommandPost `0x01` = inert fallback) | **both `0x00` and `0x01` verified working**; device echoes `0x00` back regardless → confirmed inert slot | ✅ agree — **spec: send `arg4 = 0x00`** |
| `arg5` colour count | `0x01` | `0x01` | ✅ agree |
| channels | `args[6/7/8]` = R/G/B, resp `[15/16/17]` | exact match on every read | ✅ agree |
| CRC | `XOR(buf[3..89])` | all frames accepted | ✅ agree |
| "off" frame | static frame, channel bytes `0x00`; **never `effect_none`** | `effect_none` (`0x00`/`data_size 0x06`) ACKed `status=0x02` but **did nothing** to the LEDs; static-zero works | ✅ agree — **do not use `effect_none`** |
| driver mode | **not required** | confirmed: frame works with `device_mode = 00 00` | ✅ agree — contradicts grounding §3 |
| read-back trust | unreliable across devices → seed-only | **directly tested** — reliable on fw v1.2 incl. the cold-read seed case (4× after replug → true firmware state, not stale); spec still keeps ticket 02's safe cross-device choice | ✅ compatible |

### Deviations / new facts beyond ticket 02

- **Neither VARSTORE nor NOSTORE persists across a USB re-enumeration on this unit.** Wrote
  green with `arg0=0x01` (VARSTORE), replugged → device came back **orange-only** (firmware
  keymap-indicator default). Same with `arg0=0x00` (NOSTORE). → the grounding-file assumption
  that the orange LED persists *because VARSTORE writes to flash* is **not what we observe**;
  "orange only" is simply the firmware power-on default. **Ticket 02 §6's "stale *host* state
  flashes on boot" concern is moot on this hardware.** There is always a brief "orange-only"
  window on connect until the daemon asserts — unavoidable regardless of storage mode.
- **`arg0` (NOSTORE vs VARSTORE) is cosmetic on this unit.** Both accepted; neither persists;
  a settled read-back always echoes `arg0=0x01` regardless of what was sent → the firmware
  likely ignores the distinction and treats every write as VARSTORE. Recommend sending
  `arg0=0x00` for intent-clarity, but the spec should not assume it avoids flash writes. The
  config knob ticket 02/04 imagined for this is probably not worth exposing — there is no
  observable behaviour difference to expose.
- **Consequence for the daemon (informs ticket 03):** asserting Status-LED state on daemon
  startup *and on every device reconnect* is a **hard requirement**, not an optimisation —
  the firmware always reclaims the LEDs on enumeration.
- The device **normalises the effect args in its read-back echo** (`arg3=0x00 arg4=0x00
  arg5=0x01`) regardless of what was sent in `arg4` — independent confirmation that `arg4` is
  an ignored slot.
