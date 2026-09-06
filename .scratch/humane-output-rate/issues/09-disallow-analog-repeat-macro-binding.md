# Disallow the Analog-repeat + Macro Binding

Type: task
Status: resolved
Blocked by: 01
Parent: [Humane output rate](../map.md)

> Execution ticket — resolved on this map, not handed off (map Notes: "execution is in
> scope"). Graduated from ticket 03's decision 4. An **audit-surfaced simplification**,
> not a rate fix — Analog-repeat + Macro is already ≤ 20 Hz; it is banned because it has
> no coherent meaning, not because it is fast.

## Question

Firing a Macro through an Analog-repeat Binding is nonsensical:
`analog_repeat::fire_analog_repeat_pulse` collapses a multi-step Macro to a single
simultaneous pulse (all `KeyDown`s, hold `ANALOG_REPEAT_PULSE_HOLD`, all `KeyUp`s
reversed), **ignoring every embedded `MacroStep::Delay`**. Ticket 03 decided the
combination should be disallowed rather than given semantics.

Implement the ban, following the existing precedent in `daemon/src/config.rs`
(`InvalidChordAnalogRepeat`, `AnalogRepeatOnDualStageKey`, the Fire-once
`ControllerButton` refusal, the Toggle `Step` refusal):

1. **New `ConfigError` variant** — e.g. `AnalogRepeatMacro(String)` carrying the Input
   name. `Display`: `an analog_repeat trigger on a Macro Binding is not allowed — Analog-repeat collapses a multi-step Macro to a single simultaneous pulse (its Delay steps ignored); change the trigger mode`.

2. **Reject at `SetBinding`** — the D-Bus edit path refuses it, so the GUI cannot save it.

3. **Reject at `load_or_seed`** — the Daemon refuses to start on a hand-edited
   `config.toml` carrying the combo (a `TriggerMode::AnalogRepeat` Binding whose `Action`
   is `Action::Macro { .. }`, in `base`/`held`). **No silent coercion.** Add the
   `expect_err` load test alongside the sibling cases.

4. **GUI** — the trigger-mode selector stops offering Analog-repeat when the Action is a
   Macro (or surfaces the same validation error the sibling restrictions do). Mirror in
   `daemon_stub.py` / `rules.py` if the GUI pre-validates there (as the dual-stage
   `ConfigError` set does).

5. **CONTEXT.md — Trigger mode entry.** Append a third exception to the existing "except…"
   list, after the Controller-button/Fire-once clause:

   > …and an Analog-repeat Binding, which disallows a Macro Action — Analog-repeat
   > collapses a multi-step Macro to a single simultaneous pulse (its `Delay` steps
   > ignored), so the combination has no coherent meaning (ticket 09).

   No ADR — the sibling restrictions were recorded as CONTEXT.md + ticket ref, not ADRs.

Feeds ticket 04 (the ADR notes Analog-repeat cannot wrap a Macro) and ticket 05 (the tips
no longer need to warn about this combo — it is structurally impossible).

## Answer

Implemented on `dev`. The ban is one pure per-Binding rule, so both entry points
(`SetBinding` via `edit::plan_edit`'s trailing `config::validate`, and `load_or_seed` via
`parse`) enforce it from a single place — no separate load-path check needed.

1. **`ConfigError::AnalogRepeatMacro(String)`** — new variant (`daemon/src/config.rs`),
   grouped with the other analog-repeat restrictions. The string is the offending Input's
   `Display` form (`grid_r1c1`), like `AnalogRepeatOnDualStageKey`. `Display`:
   *"an analog_repeat trigger on the Macro Binding for `"grid_r1c1"` is not allowed —
   Analog-repeat collapses a multi-step Macro to a single simultaneous pulse (its Delay
   steps ignored); change the trigger mode"*.

2. **`config::binding::check_binding`** — the check lives in the site-shape step, inside
   the `Individual(input)` arm, after the non-grid guard: `trigger == AnalogRepeat`
   + `Action::Macro { .. }` on a Grid Input → `AnalogRepeatMacro`. It is reachable only
   where analog-repeat is otherwise legal — a non-grid Input still trips
   `InvalidAnalogRepeatInput` first, a Chord Binding still trips `InvalidChordAnalogRepeat`
   first (both established, both keep their existing tests). Because `check_binding` runs
   over every binding site in `validate` (base/held **and** deep), a hand-edited deep
   Binding with the combo is refused too.

3. **`schema.rs` fixture** — `trigger_verdict` drives `check_binding` directly, so
   `ACHERON_BLESS=1 cargo test schema` flipped the 20 `{macro, grid_*, analog_repeat}`
   rows to `allowed: false` in `daemon/contract/daemon-schema.json`.

4. **GUI** — `rules.valid_triggers` drops `analog_repeat` when `action_kind == "macro"`
   (the mirror). `binding_editor.render_action_editor` now rebuilds the Trigger-mode model
   from the `rules` matrix for `kind in ("controller_button", "macro")` (was
   `controller_button` only), so the dropdown stops offering Analog-repeat for a Macro and
   a live switch keypress→macro while Analog-repeat is selected falls back to
   Hold-to-repeat (and clears the Analog-repeat GUI hint). `daemon_stub._validate_binding`
   already delegates to `rules.valid_triggers`, so it rejects the combo with no change.

5. **CONTEXT.md** — third exception appended to the **Trigger mode** entry's "except…"
   list, after the Controller-button/Fire-once clause, citing ticket 09. No ADR (sibling
   restrictions were CONTEXT.md + ticket ref only). ADR-0008 / the
   **Physical-plausibility ceiling** term already say "Analog-repeat cannot wrap a Macro"
   (written at ticket 04).

**Tests**: `refuses_to_start_when_an_analog_repeat_grid_binding_wraps_a_macro` (load-path
`expect_err`, alongside the sibling analog-repeat cases; asserts the file is left
untouched — no silent coercion); `check_binding_truth_table` updated (Grid + macro +
analog_repeat → `AnalogRepeatMacro`); `test_rules.py` split into
`test_keypress_allows_every_trigger_on_a_grid_key` +
`test_macro_excludes_analog_repeat_even_on_a_grid_key`;
`test_analog_repeat_hint_tracks_the_selection_across_action_kind_changes` updated for the
new fallback. Full suite green — daemon 517, GUI 503, clippy clean.

This was the last open ticket on the map. The map's destination is reached: the audit is
ratified, the flagged surfaces are fixed (ticket 06), the ADR + term are written (ticket
04), and both specs are gated — `spec-user-facing-output-safety-guidance.md` (ticket 05,
now the [`output-safety-guidance/`](../../output-safety-guidance/issues/) effort) and
`spec-kernel-shaped-repeat.md` (ticket 07, shipped in
[`kernel-shaped-repeat-impl/`](../../kernel-shaped-repeat-impl/issues/)). The map can be
archived.
