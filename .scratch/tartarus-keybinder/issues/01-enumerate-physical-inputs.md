Type: task
Status: resolved

## Question

Physically enumerate every Input on the Tartarus Pro — all 20 grid keys, the Mode key, all thumbstick directions, and the scroll wheel (both directions) — and record which evdev node and evdev code each one produces.

This is a live-hardware session with the user: capture evdev events from all three device nodes (`.../event-kbd`, `.../if01-event-kbd`, `.../if02-event-mouse`) while systematically pressing every physical control in turn, and produce a complete Input → (node, evdev code) table as this ticket's answer/asset. Known so far (see map Notes): the main node emits only the Mode key as `KEY_LEFTALT`; `if01` carries the grid as standard keycodes (partially confirmed: TAB/Q/W/E/R, A/S/D/F seen so far — the rest of the grid, plus Z/X/C/V/B row and any thumb-row keys, are unconfirmed); `if02` carries the thumbstick as cursor keys and a real wheel, both unconfirmed in detail.

## Answer

Live-hardware session, 2026-08-12. Captured with `python-evdev`, each node grabbed exclusively (`EVIOCGRAB`) via a throwaway script so test presses didn't leak into the desktop. Raw event log: [assets/enumerate-physical-inputs-capture.jsonl](../assets/enumerate-physical-inputs-capture.jsonl).

Grid is 4 rows × 5 columns, scanned top-left → bottom-right, row by row. Thumbstick is 4-way with no center click. Wheel scrolls up/down and also has a middle click.

**Correction to the map's prior assumption**: the thumbstick emits on the **`main`** node (the same node as the Mode key), not `if02`. Only the scroll wheel and middle click are on `if02`. Confirmed by capturing all three nodes simultaneously during a live session — see the raw log.

| Input | Node | Evdev type | Evdev code | Notes |
|---|---|---|---|---|
| Mode key | main | EV_KEY | `KEY_LEFTALT` | press=1/release=0 |
| Grid R1C1 | if01 | EV_KEY | `KEY_1` | |
| Grid R1C2 | if01 | EV_KEY | `KEY_2` | |
| Grid R1C3 | if01 | EV_KEY | `KEY_3` | |
| Grid R1C4 | if01 | EV_KEY | `KEY_4` | |
| Grid R1C5 | if01 | EV_KEY | `KEY_5` | |
| Grid R2C1 | if01 | EV_KEY | `KEY_TAB` | |
| Grid R2C2 | if01 | EV_KEY | `KEY_Q` | |
| Grid R2C3 | if01 | EV_KEY | `KEY_W` | |
| Grid R2C4 | if01 | EV_KEY | `KEY_E` | |
| Grid R2C5 | if01 | EV_KEY | `KEY_R` | |
| Grid R3C1 | if01 | EV_KEY | `KEY_CAPSLOCK` | |
| Grid R3C2 | if01 | EV_KEY | `KEY_A` | |
| Grid R3C3 | if01 | EV_KEY | `KEY_S` | |
| Grid R3C4 | if01 | EV_KEY | `KEY_D` | |
| Grid R3C5 | if01 | EV_KEY | `KEY_F` | |
| Grid R4C1 | if01 | EV_KEY | `KEY_LEFTSHIFT` | |
| Grid R4C2 | if01 | EV_KEY | `KEY_Z` | |
| Grid R4C3 | if01 | EV_KEY | `KEY_X` | |
| Grid R4C4 | if01 | EV_KEY | `KEY_C` | |
| Grid R4C5 | if01 | EV_KEY | `KEY_SPACE` | breaks the QWERTY pattern (not `KEY_V`) |
| Thumbstick Up | main | EV_KEY | `KEY_UP` | |
| Thumbstick Down | main | EV_KEY | `KEY_DOWN` | |
| Thumbstick Left | main | EV_KEY | `KEY_LEFT` | |
| Thumbstick Right | main | EV_KEY | `KEY_RIGHT` | |
| Wheel scroll up | if02 | EV_REL | `REL_WHEEL` (+1), `REL_WHEEL_HI_RES` (+120) | paired events, same SYN_REPORT |
| Wheel scroll down | if02 | EV_REL | `REL_WHEEL` (−1), `REL_WHEEL_HI_RES` (−120) | paired events, same SYN_REPORT |
| Wheel middle click | if02 | EV_KEY | `BTN_MIDDLE` | press=1/release=0 |

All 20 grid keys + Mode key + 4 thumbstick directions + wheel (both scroll directions and middle click) confirmed live, one clean press-release per Input, no ambiguous/duplicate events in the log.
