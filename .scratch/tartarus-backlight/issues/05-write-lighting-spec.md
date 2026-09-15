Type: task
Blocked by: 01, 02, 03, 04
Status: resolved (Charon, 2026-09-15)

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

Consolidated tickets 01–04 into [`spec.md`](../spec.md) — same section list as
`tartarus-status-leds/spec.md` (Problem Statement, Solution, User Stories, Implementation
Decisions [wire frames / daemon architecture / config schema / D-Bus surface / GUI / startup-
shutdown / domain vocabulary], Testing Decisions, Out of Scope, Further Notes).

Also written:

- [ADR-0012](../../../docs/adr/0012-lighting-shares-the-led-task-no-varstore-shutdown-clear.md)
  — extends ADR-0002/ADR-0006: Lighting shares the `led` task via a second `watch` channel
  (not a sibling task), and gets no shutdown-time clear because backlight effect-select frames
  are VARSTORE (firmware-persisted), unlike Status LEDs' NOSTORE frame.
- Four new `CONTEXT.md` entries under `### Configuration`: **Lighting**, **Lighting
  assignment**, **Fixed effect**, **Custom layout** — avoid-lists per the charting decisions
  ("effect" stays fine for Lighting's built-in patterns, always paired with "Lighting"/"Fixed
  effect", never a bare "Effect" as a standalone concept name; Chroma/backlight-assignment/etc.
  ruled out as synonyms).
- `.scratch/README.md`'s line for this effort flipped from "charting" to "spec ready", with a
  full summary of tickets 01–05's findings and decisions.

No new decisions were made resolving this ticket — it is pure consolidation of tickets 01–04's
already-settled answers, corrected once for two contradictions ticket 04 itself already flagged
and fixed (Reactive/Starlight's `speed` field, Wave's dedicated `WaveDirection` type). Every
ticket on this map is now resolved; implementation is a fresh effort
(`tartarus-backlight-impl`), not a resumption of this map.
