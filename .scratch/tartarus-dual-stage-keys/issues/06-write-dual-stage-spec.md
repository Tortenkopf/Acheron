Type: task
Blocked by: 02, 03, 04, 05
Status: resolved (Charon, 2026-09-04)

## Question

Write the destination: a reviewed **`.scratch/tartarus-dual-stage-keys/spec.md`** for the
"Dual-stage keys" feature, plus the ADR and the `CONTEXT.md` terms. Pure consolidation of
tickets 01–05 — no new decisions. Model on `tartarus-status-leds/spec.md`.

### Deliverables

1. **`spec.md`** — feature summary, the Actuation-stage model, the four staging modes with
   their exact event sequences (from ticket 02), the depth-ordering constraint, the config
   schema + `config.toml` example (from ticket 03), the D-Bus `Edit` surface, the
   `ConfigError` additions, the GUI layout in text (from ticket 04), the runtime-behavior
   / interaction section (from ticket 05), pipeline architecture (from ticket 01),
   Digital-mode degradation, out-of-scope, and vocabulary.
2. **ADR** — `docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md` (or next
   number), from ticket 01's drafted ADR. Note if it refines ADR-0002.
3. **`CONTEXT.md` entries** — `Actuation stage`, `deep stage` (and `primary stage` if it
   earns its own line), `Staging mode` (decide: one combined entry or one per mode — the
   held fog patch). Use the `/domain-modeling` CONTEXT format. Cross-reference
   `Actuation point`, `Release point`, `Trigger mode`, `Analog-repeat`, `Chord`.
4. **`.scratch/README.md`** — flip this effort's line to "spec ready".
5. State plainly in `map.md` that **implementation is a fresh effort**, and that this map
   can be archived.

### Not deliverables (belong to the impl effort)

- The `rules.py` code itself (its *shape* was decided in ticket 03).
- README user-facing copy — draft it here if cheap, but it's fine to leave to impl.
- Any daemon or GUI production code.

## Answer

Pure consolidation of tickets 01–05, no new decisions except the two small fog patches this
ticket owned by charter (both settled by precedent, not by grilling):

1. **`spec.md`** written at [`.scratch/tartarus-dual-stage-keys/spec.md`](../spec.md), modelled
   on `tartarus-status-leds/spec.md`'s structure: Problem Statement, Solution, 10 User Stories,
   Implementation Decisions (pipeline architecture, the four Staging-mode state tables verbatim
   from ticket 02, config schema + `config.toml` example + `ConfigError` additions from ticket
   03, the D-Bus `Edit` surface, the swap-toggle GUI layout from ticket 04, the runtime-behavior
   sweep from ticket 05, Digital-mode degradation, domain vocabulary), Testing Decisions, Out of
   Scope, Further Notes.
2. **ADR-0007** filed at
   [`docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md`](../../../docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md),
   from ticket 01's drafted content — rewritten into the repo's actual house style (freeform
   prose under one title, no Status/Context/Decision/Consequences subheadings — ADR-0006 was
   the template; ticket 01's draft had used subheadings that don't match precedent). States
   explicitly that it refines the *spirit* of the Axis/Analog-repeat pattern and does not
   supersede or refine ADR-0002.
3. **`CONTEXT.md` entries** — six, added between **Release point** and **Status LED**:
   `Actuation stage` (one entry covering both primary and deep stage by contrast — "primary
   stage" did not earn its own line, it's just "the ordinary Binding" restated), `Staging mode`
   (the governing concept), and one short entry each for `Handoff`, `No-Return`, `Additive`,
   `Quick-Skip` — **the held fog patch, settled as one-entry-per-mode**, mirroring the existing
   Trigger-mode → Fire-once/Hold-to-repeat/Toggle/Analog-repeat structure already in the file
   rather than one combined "Staging mode" paragraph. Chosen for consistency with that existing
   precedent, not re-litigated by grilling — a straightforward format call once the precedent
   was checked.
4. **`.scratch/README.md`** — this effort's line flipped to "**spec ready**", summarized with
   the same density as the `tartarus-status-leds` line it's modelled on.
5. **`map.md`** — added a "Destination reached" section stating implementation is a fresh
   effort and this map can be archived (same close-out language `tartarus-status-leds/map.md`
   would use), and cleared **Not yet specified** (both patches graduated — see below).

**README user-facing copy** — the other held fog patch — resolved as **not a deliverable of this
spec**: `spec.md`'s Out of Scope section states it's left to the implementation effort, the same
way most feature README copy is written during implementation rather than during charting/spec.
It was cheap to draft here but would have been throwaway prose disconnected from the actual
Actuation-stage/Staging-mode vocabulary an implementer will want to word it with once the code
exists — deferring costs nothing and avoids stale copy.

No decision in tickets 01–05 was found to need correcting or re-opening while writing the spec;
every cross-reference (the post-release ticket 15 `Slots<K>` correction, the ticket 05
corrections to tickets 01/02's Layer-switch and disconnect-hook assumptions) was already folded
into the relevant ticket's own Answer and is carried into `spec.md` as the current, not the
originally-drafted, design.
