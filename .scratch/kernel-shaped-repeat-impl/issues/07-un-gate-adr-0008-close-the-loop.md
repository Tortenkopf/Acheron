# 07 — Un-gate ADR-0008, close the loop

**What to build:** The paper trail that says the `value=2` rebuild shipped. ADR-0008's
"gated and pending a fresh implementation effort" language flips to shipped, the
`CONTEXT.md` term reads as present-tense reality (it mostly already does — verify),
the `humane-output-rate` map records the resolution, and that effort's ticket 08 is
marked resolved with a pointer to ticket 05 here (which wrote its regression tests).
No daemon code.

Source of truth: [`spec-kernel-shaped-repeat.md`](../../humane-output-rate/spec-kernel-shaped-repeat.md)
§9 (cross-ticket effects).

**Blocked by:** 04 — Hold-to-repeat `value=2`; 05 — Toggle sustained autorepeat;
06 — Analog-repeat hold-solid `value=2`.

**Status:** done

- [x] `docs/adr/0008-physical-plausibility-ceiling-for-synthetic-output.md`: the
      "**Kernel-shaped repeat (`value=2`)**" paragraph loses "**gated and pending a
      fresh implementation effort**" and states it landed (name this effort:
      `.scratch/kernel-shaped-repeat-impl/`). Keep the design rationale (inject
      `value=2` ourselves; the surface list; the "not touched" list). "After it
      lands, …" becomes present tense.
- [x] `CONTEXT.md` "Physical-plausibility ceiling" entry (line ~140): confirm the
      "A held or repeated single key presents as genuine kernel autorepeat" sentence
      is accurate as shipped and reads present-tense; adjust only if the
      implementation diverged from the spec (e.g. the deep-stage inclusion — add a
      few words if worth noting). The `Toggle` entry (line ~76) — check whether
      "held down, for a Keypress" now wants a nod to the autorepeat envelope.
- [x] `.scratch/humane-output-rate/map.md`: a Decisions-so-far line (or an update to
      the ticket 07 line) recording that the implementation effort completed, with a
      link to this directory.
- [x] `.scratch/humane-output-rate/issues/08-lock-macro-repetition-floor-tests.md`:
      mark resolved, `## Answer` pointing at ticket 05 here for the Toggle→Macro
      regression split and noting the Hold-to-repeat→Macro re-fire test rides the
      same seam. (Confirm with the reader/owner if ticket 08 has non-test scope
      still open — per the effort decision the test work is done here.)
- [x] `.scratch/README.md`: flip this effort's line from active to done/archived as
      appropriate, and refresh the `humane-output-rate` line's ticket-07 / ticket-08
      status.
- [x] README: check the Hold-to-repeat / Toggle feature descriptions for any
      "sends repeated keypresses" wording that the `value=2` change makes inaccurate;
      tighten if so. (The user-facing *tips* are a separate spec —
      `.scratch/humane-output-rate/spec-user-facing-output-safety-guidance.md`, ticket 05 there — not this ticket.)

## Comments

### Resolved 2026-09-06 — no daemon code

Paper-trail only. Blockers 04/05/06 all shipped on `dev`; full daemon suite green
(516 passed).

- **ADR-0008** — "Kernel-shaped repeat (`value=2`)" paragraph reworded: "Before this
  change …" / "The rebuild (… carried out in `.scratch/kernel-shaped-repeat-impl/`) …"
  / "Acheron's held and repeated keyboard output is now timing-indistinguishable …".
  Design rationale, the converts-list, and the not-touched list all kept; the
  converts-list gained **single-key deep-stage Hold-to-repeat** (effort decision, and
  as-built via the deep `Slots<StageKey>`).
- **`CONTEXT.md`** — the "Physical-plausibility ceiling" entry already read
  present-tense and accurate as shipped; left as-is (the generic "A held or repeated
  single key" already covers the Chord / deep-stage cases the ADR enumerates). The
  **"Toggle"** entry was restructured: the old "(looping, for a Macro; held down, for a
  Keypress or a Controller button …)" conflated three now-distinct behaviours —
  rewritten as multi-step Macro loops / single keyboard key held as genuine kernel
  autorepeat (`value=1` then `value=2`, cross-ref the ceiling term) / mouse-or-gamepad
  button held as one solid press. All ticket-78/82/75/76 citations preserved.
- **`humane-output-rate/map.md`** — ticket-07 Decisions-so-far line gained an
  "Implemented and shipped" paragraph linking here; the "Not yet specified" bullet for
  the `value=2` repeat struck through and marked done.
- **`humane-output-rate` ticket 08** — `Status: resolved` + `## Answer`. Item 1
  (Toggle→Macro split) landed with this effort's ticket 05
  (`toggle_single_key_macro_holds_the_same_autorepeat_as_the_equivalent_keypress` +
  `toggle_multi_step_macro_still_loops_at_target_lap`). Item 2 (Hold-to-repeat→Macro
  overlap guard) was already covered by the pre-existing
  `overlapping_same_input_firings_are_dropped_not_queued` + the `decision_table`
  `FiringUnfinished`→`Nothing` rows; chord-keyed variant transitive. Item 3 (comments)
  — the floor rationale is documented at `MIN_TOGGLE_LAP` / `run_toggle_loop` /
  `trigger::decide`. Test-only scope, all satisfied — nothing left open, no daemon
  code touched here.
- **`.scratch/README.md`** — this effort's line → "**done / archivable** (all 7
  tickets resolved 2026-09-06)"; the `humane-output-rate` line's ticket-07 marked
  implemented, ticket-08 marked resolved, the "implement the `value=2` repeat" fog
  removed.
- **README** — checked: the Trigger-modes bullet only names the modes, and
  "Mouse-button hold … is a real sustained press" is still accurate. Nothing to
  tighten. (The `## Output safety` section is `humane-output-rate` ticket 05's
  `spec.md`, a separate effort.)
- **`spec-kernel-shaped-repeat.md`** — status header flipped to "implemented"; two
  "As implemented" notes added (§3.2, §7) fixing the pre-implementation guess that a
  Layer switch / capture flip stops an autorepeat Toggle — it does not; only the
  second press, a Profile switch, or the GUI-focus `StopAllToggles` do
  (`single_key_autorepeat_toggle_survives_a_layer_switch`). This was the open
  spec-wording follow-up from ticket 05's `/code-review`.
