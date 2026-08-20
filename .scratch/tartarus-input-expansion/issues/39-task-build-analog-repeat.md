Type: task
Blocked by: 20

## Question

Build Analog-repeat for real: land `TriggerMode::AnalogRepeat` per
[ticket 20](./20-decide-analog-repeat-trigger-mode.md)'s settled design, and tune its numeric
constants live against the real Tartarus Pro. Non-blocking for v1.0.

Scope, per ticket 20's decisions:

- New `TriggerMode::AnalogRepeat` variant: config schema, D-Bus wire encoding
  (`dbus/wire.rs`'s `trigger_mode_str`/`trigger_mode_from_str`), GUI `TRIGGER_OPTIONS`.
- `dispatch.rs` gets its own receiver on the existing `depth_tx` watch channel (mirrors how it
  already holds `actuation_tx`), rather than relying on `PhysicalEvent.depth`'s coarser
  Repeat-cadence delivery.
- A new per-Input background-task type (its own `HashMap<Input, _>` tracking, alongside the
  existing `toggles`/`in_flight`), spawned when a grid key's Depth crosses a small **fixed**
  deadzone threshold going up (deliberately not the key's tunable Actuation point — the rate
  curve wants the key's full 0–255 travel range), cancelled when Depth crosses back down through
  that same fixed threshold. Each tick recomputes its own next sleep duration from the latest
  Depth, linearly mapped across the full 0–255 range to a min/max Hz.
- Each fire is a Down+Up pulse of a fixed short duration (not Depth-scaled).
- Above a fixed near-full-travel threshold, the key holds down solid instead of continuing to
  tap.
- Digital Capture mode (no Depth): falls back to plain Hold-to-repeat, kernel-autorepeat cadence
  — matches how every other grid key already behaves there.
- GUI: "Analog-repeat" greyed out as a Trigger-mode option for non-Grid Inputs (mirrors the
  existing `is_grid_input()` gate on the Actuation & release section).
- **Tune live against the real device**: the deadzone threshold, the min/max Hz bounds, the
  fixed fire-duration, and the hold-solid threshold are all TBD — pick values that feel right
  against actual key travel, not blind numbers carried over from the design ticket.

## Answer

_(unresolved)_
