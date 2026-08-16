Type: task

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

_(unresolved)_
