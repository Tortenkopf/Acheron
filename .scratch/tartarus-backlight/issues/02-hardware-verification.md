Type: task
Blocked by: 01
Status: open

## Question

On the real Tartarus Pro, verify ticket 01's source-derived frames — the same shape as
[`tartarus-status-leds/assets/01-RESULTS.md`](../../tartarus-status-leds/assets/01-RESULTS.md),
but as a plain verification task, not a kill-gate (this effort is ungated per the map).

- Send a static colour via the Custom-layout / `set_key_row` path and confirm it lights the 20
  grid keys **and the scroll wheel**, leaving the Mode key and thumbstick dark — settles Q6
  physically.
- Send an alternating-colour Custom-layout frame and confirm the column-ordering question
  ticket 01 flagged (or left open pending hardware).
- Cycle through 2–3 Fixed effects (e.g. breathing, wave) and confirm: they run without a
  Capture/driver-mode change (mirrors `tartarus-status-leds` ticket 01's "no driver mode needed"
  finding), and don't disturb analog capture or remapping while active.
- Read and set brightness; confirm its real value range against ticket 01's prediction.
- Confirm none of the above destabilizes the device the way the normal-mode transition does
  (ADR-0006's existing caution about the Tartarus Pro resetting on that specific command).

Record results as `.scratch/tartarus-backlight/assets/02-RESULTS.md`.

## Answer

