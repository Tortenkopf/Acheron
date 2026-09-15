Type: task
Blocked by: 01
Status: resolved

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

Verified live on the real Tartarus Pro (firmware v1.2, serial
PM2443F36300141) — full run log and JSONL evidence in
[`02-RESULTS.md`](../assets/02-RESULTS.md). Summary:

- **Q6 confirmed, stronger than stated**: Custom-layout fill lights the 20
  grid keys + scroll wheel; the Mode key and thumbstick are solid black
  plastic (not RGB-capable at all, not merely unlit).
- **Column-addressing order settled — corrects the map's charting note**:
  wire columns `0..18` = grid keys `1..19` in order, column `19` = the
  **scroll wheel**, column `20` = grid key `20`. The wheel is
  second-to-last in wire order, not the 21st/last cell as charted.
- **Fixed effects need no driver-mode change**: wave, static, and all three
  breathing variants (single — both `transaction_id 0x1F` and the driver's
  actual `0x3F` — dual, and random) all ran correctly from `device_mode
  00 00`; the Breath transaction_id "bug" is confirmed harmless — either
  value works, so the spec can just use `0x1F` for consistency with every
  other effect.
- **Brightness confirmed `0x00`–`0xFF`** (not 0–100), get/set round-trip
  exactly, and `0x00` alone blacks out the matrix independent of effect
  state — a simpler off primitive than a zero-colour static frame.
- **No adverse behaviour**: ~15 feature writes + one relock, device stayed
  present throughout, no re-enumeration. A brief `device_mode` excursion to
  driver mode mid-session was traced to the real `acheron-daemon`'s own
  analog-capture supervisor being started concurrently for an unrelated
  check — not caused by this tool, and backlight writes worked fine on both
  sides of it (a coexistence finding, not a concern).
