Type: task
Blocked by: 01, 02, 03, 04
Status: open

## Question

Consolidate tickets 01–04 into a reviewed `spec.md` (feature summary, wire frames, daemon
architecture, config schema, D-Bus surface, GUI, startup/shutdown, out-of-scope, vocabulary) —
same section list as `tartarus-status-leds/spec.md`. Also write:

- A new ADR extending ADR-0002/ADR-0006 to the backlight matrix (the way ADR-0006 itself
  refined ADR-0002 for Status LEDs).
- New `CONTEXT.md` entries: **Lighting**, **Lighting assignment**, **Fixed effect**, **Custom
  layout** (avoid-lists per the charting decisions — "effect" stays fine for Lighting, paired
  with the word "Lighting"; don't let it read as a synonym for Action).
- Flip `.scratch/README.md`'s line for this effort from "charting" to "spec ready."

Implementation is a separate, fresh effort (a `tartarus-backlight-impl` directory), not a
resumption of this map.

## Answer

