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

**Status:** ready-for-agent

- [ ] `docs/adr/0008-physical-plausibility-ceiling-for-synthetic-output.md`: the
      "**Kernel-shaped repeat (`value=2`)**" paragraph loses "**gated and pending a
      fresh implementation effort**" and states it landed (name this effort:
      `.scratch/kernel-shaped-repeat-impl/`). Keep the design rationale (inject
      `value=2` ourselves; the surface list; the "not touched" list). "After it
      lands, …" becomes present tense.
- [ ] `CONTEXT.md` "Physical-plausibility ceiling" entry (line ~140): confirm the
      "A held or repeated single key presents as genuine kernel autorepeat" sentence
      is accurate as shipped and reads present-tense; adjust only if the
      implementation diverged from the spec (e.g. the deep-stage inclusion — add a
      few words if worth noting). The `Toggle` entry (line ~76) — check whether
      "held down, for a Keypress" now wants a nod to the autorepeat envelope.
- [ ] `.scratch/humane-output-rate/map.md`: a Decisions-so-far line (or an update to
      the ticket 07 line) recording that the implementation effort completed, with a
      link to this directory.
- [ ] `.scratch/humane-output-rate/issues/08-lock-macro-repetition-floor-tests.md`:
      mark resolved, `## Answer` pointing at ticket 05 here for the Toggle→Macro
      regression split and noting the Hold-to-repeat→Macro re-fire test rides the
      same seam. (Confirm with the reader/owner if ticket 08 has non-test scope
      still open — per the effort decision the test work is done here.)
- [ ] `.scratch/README.md`: flip this effort's line from active to done/archived as
      appropriate, and refresh the `humane-output-rate` line's ticket-07 / ticket-08
      status.
- [ ] README: check the Hold-to-repeat / Toggle feature descriptions for any
      "sends repeated keypresses" wording that the `value=2` change makes inaccurate;
      tighten if so. (The user-facing *tips* are a separate spec —
      `.scratch/humane-output-rate/spec.md`, ticket 05 there — not this ticket.)
