Type: task
Status: resolved

## Question

Collect the remaining **hardware facts about driver mode** that the analog data model
([ticket 17](./17-decide-analog-data-model.md)) and the capture rework
([ticket 18](./18-rework-capture-path-for-analog.md)) depend on. HITL — needs the physical
device and a human pressing keys.

[The standalone prototype](./13-task-standalone-analog-capture-prototype.md) settled that
the analog stream works, is genuinely 8-bit, is event-driven, and that driver mode
**silences the grid keys' ordinary evdev keycodes**. It did not settle what follows.
Extend `prototype/13-analog-grid-capture/prototype.py` rather than writing something new,
and **persist evdev events to the JSONL this time** — the previous run counted them
in-session but only wrote `kind: "analog"` records to
[`assets/13-unlocked.jsonl`](../assets/13-unlocked.jsonl), which is why several of the
questions below can't be answered from the existing capture.

Settle at least:

- **Does driver mode silence anything other than the 20 grid keys?** This is the one that
  can invalidate the whole strand. Report `0x06` carries 20 depth bytes for the 20 keycaps
  only — the **Mode key**, the **thumbstick**'s four directions, and the **wheel** are not
  in it, and they live on different evdev nodes (`Node::Main` and `Node::If02`, see
  `daemon/src/input.rs`). If driver mode silences those too, analog-primary capture loses
  the Layer key and the thumbstick with no analog channel to recover them from, and the
  whole design changes. Test each one explicitly, in driver mode, with the Daemon stopped
  so the nodes are ungrabbed.
- **Does a power cycle restore evdev mode?** Unplug/replug the device while it is in driver
  mode and confirm it comes back typing normally without an explicit re-lock. The user's
  assumption is that it does; nothing in the paper trail confirms it.
- **Does the mode survive suspend/resume?** Same question for a laptop lid-close or
  `systemctl suspend` cycle — does the device come back in driver mode, in evdev mode, or
  in an inconsistent state where the Daemon thinks it is in one and it is in the other?
- **What does an unclean Daemon death leave behind?** `SIGKILL` the Daemon (or the
  prototype) while the device is in driver mode and confirm the failure mode a user would
  actually hit: 20 dead grid keys until something re-locks. Confirm that simply restarting
  the Daemon recovers it, and that a re-lock works from a *fresh* process that never sent
  the unlock itself.
- **Is the byte-to-keycap mapping really the identity in reading order?** Ticket 13 inferred
  `byte n = keycap n` from two runs where keys happened to be pressed in layout order and
  flagged it as worth re-confirming. Confirm it per-key, deliberately out of order, so
  ticket 18 can rely on it.

## Answer

**The strand is not invalidated: driver mode silences the 20 grid keys and nothing else.**
Ran against the real unit (serial `PM24XXXXXXXXXXX`, firmware `v1.2`) on 2026-08-16 with the
Daemon stopped throughout, so all three evdev nodes were ungrabbed. Thirteen processes,
appended to one capture: [`assets/16-driver-mode-facts.jsonl`](../assets/16-driver-mode-facts.jsonl)
(2028 records — analog reports, **evdev events this time**, per-keycap mapping observations
and phase markers). Prototype extended in place, per the ticket:
[`prototype/13-analog-grid-capture/prototype.py`](../../../prototype/13-analog-grid-capture/prototype.py)
gained a non-grid-Input panel, a `mapping` command, a `survive` command and per-phase counters.

### 1. Does driver mode silence anything other than the 20 grid keys? No.

Every Input pressed in normal mode, then again after the unlock, in one process each:

| Channel | Normal mode | Driver mode |
|---|---|---|
| grid keycodes (`if01`) | 6 keys, press+release | **0 — silenced** |
| Mode key (`if00`) | press + repeat + release | **5 presses, 19 repeats — alive** |
| thumbstick ×4 (`if00`) | all four | **all four, 7-10 repeats each — alive** |
| wheel scroll ±, middle click (`if02`) | `REL_WHEEL`, `REL_WHEEL_HI_RES`, `BTN_MIDDLE` | **all alive** |
| report `0x06` | 0 | 269 reports, 161 distinct depths |

The 8 non-grid Inputs are untouched by the mode switch, so **ticket 18's "hybrid source" is
the right shape and is now confirmed rather than assumed**: `hidraw` for the 20 grid keys,
evdev for the other 8, both live at once.

**The device's own evdev autorepeat still fires in driver mode** for the Mode key and the
thumbstick. That narrows ticket 18's sharpest flagged regression: Hold-to-repeat loses its
device-generated `Repeat` **for the 20 grid keys only**, not for every Input, so the analog
source has to synthesise repeat for the grid while the other 8 keep working untouched.

### 2. Does a power cycle restore evdev mode? Yes.

Two unplug/replug cycles from driver mode. After each, a fresh process saw **0 analog reports
and grid keys typing normally again**; `device_mode` reads back `00 00`. It is a real
re-enumeration, not a soft reset — the HID IDs changed (`000C`/`000D` → `000F`/`0010`) and
the `if02` evdev node moved (`event26` → `event24`). The user's assumption holds.

### 3. Does the mode survive suspend/resume? Yes — and the device never leaves the bus.

`systemctl suspend`, 41 s suspended, measured **inside one process across the gap**:

| | before suspend | after resume |
|---|---|---|
| report `0x06` | 76 | **51** |
| grid keycodes | 0 | **0** |
| Mode key / thumbstick | alive | **alive** |

Same process, same file descriptors, no re-attach and no disconnect — the open `hidraw` fd
survived the suspend and the stream simply resumed. So the two cases are **opposites**, and
that asymmetry is the finding: after a suspend the Daemon's belief about the mode is still
correct, while after a power cycle it is stale. The recovery trigger is USB re-enumeration,
which ticket 18 already plans to handle via `evdev_source`'s existing `connection_tx`
hotplug path — the re-unlock belongs there, and suspend needs no handling at all.

Note the failure direction is the safe one: a power cycle leaves the user with a **working**
keypad and a Daemon reading a dead `hidraw`, not a dead keypad.

### 4. What does an unclean Daemon death leave behind? 20 dead grid keys, nothing else.

`SIGKILL` to the prototype while in driver mode, then two fresh processes:

- **The mode outlives the process.** A `listen` that sent nothing got 96 analog reports and
  **0 grid keycodes** — the stranded state a user would actually hit. The Mode key,
  thumbstick and wheel kept working throughout, so the keypad is crippled, not dead.
- **A re-lock works from a fresh process that never sent the unlock.** `relock` produced the
  standby report 4 ms later and `KEY_SPACE` was typing again 1.6 s after that.

The ticket also asked to confirm that *restarting the Daemon* recovers it. That is not
testable today and was not tested: the Daemon has no unlock/re-lock code at all yet (analog
is unintegrated), so a restart currently does nothing to the device mode. What is confirmed
is the mechanism such a restart would rely on — an unrelated process can re-lock — which is
what makes ticket 18's "re-lock on clean shutdown, unlock on start" lifecycle recoverable.

### 5. Is the byte-to-keycap mapping the identity? Yes, confirmed per-key, out of order.

All 20 keycaps, prompted one at a time in a fixed permutation with no fixed point and no
adjacent pair (13, 20, 14, 17, 9, 1, 15, 6, 11, 19, 2, 4, 8, 5, 12, 10, 3, 7, 16, 18), each
resolved only when exactly one byte index moved and everything returned to zero. **20/20
identity, 20 distinct bytes, no ambiguous attempts.** Ticket 13's inference rested on keys
having been pressed in layout order; it no longer does. Ticket 18 can rely on `byte n = keycap n`.

### Incidental findings

- **The standby `0x06` report marks a mode *transition*, not every `set_device_mode`.** All
  three transitions into driver mode emitted it within 4 ms; a fourth, redundant unlock sent
  while already in driver mode emitted nothing. Cheap way for the Daemon to tell "I changed
  it" from "it was already there" — one observation, so worth re-checking if leaned on.
- **Still no reset.** Five more `set_device_mode` sends (four unlocks, one re-lock) with the
  device never leaving the bus except when physically unplugged — nine clean sends now across
  both tickets, all with `transaction_id 0x01`.
- **The layout held a third time**: every report 24 bytes, ID `0x06`, depths at bytes 1-20,
  trailing bytes 21-23 zero, across 1546 reports.
- **Any TUI touching this device must not treat `ESC` as a quit key.** The thumbstick types
  arrow keys, and an arrow key *is* an ESC sequence on a tty — the first live run aborted on
  the first thumbstick press. The prototype now strips CSI/SS3 sequences before looking for a
  lone ESC, with a selftest case per arrow.

### What is weaker than the rest

The `survive` command's re-attach was supposed to hold the power cycle inside one process and
print a before/after table. On hardware it twice exited during the wait **without logging
anything**, which no code path in it does — most consistent with the process being signalled
rather than returning. The re-attach path was then hardened (exception-safe `discover()` for
half-populated sysfs nodes mid-replug, `KeyboardInterrupt` handled, a prominent countdown
banner instead of one buried log line) and verified offline against FIFO stand-ins, but
**not re-verified on hardware**. So §2 rests on cross-process evidence — two independent
replug cycles plus the `device_mode` readback — rather than the in-process table. §3, which
needed the same command, did work in-process and is the stronger of the two.
