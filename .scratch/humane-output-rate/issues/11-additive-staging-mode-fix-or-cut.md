# Additive Staging mode — fix or cut

Type: grilling
Status: resolved
Blocked by: —
Parent: [Humane output rate](../map.md)

## Question

The **Additive** Staging mode (Dual-stage keys) lets a grid key hold its primary stage
*and* its deep stage simultaneously — crossing into the deep band adds the deep stage's
firing without releasing the primary. Decide whether to **fix** it (pin down the exact
intended behaviour and close the gaps below) or **cut** it (drop the mode, migrate configs,
and let a deep stage bound to a **Macro** cover the "press two things" case instead).

This lands on *this* map, not the (archived) Dual-stage keys map, because the humane-output-rate
work is what destabilised Additive and what raises the plausibility question against it:
- The `value=2` kernel-shaped-repeat rebuild ([ticket 07](07-spec-kernel-shaped-repeat.md),
  shipped in [`kernel-shaped-repeat-impl/`](../kernel-shaped-repeat-impl/issues/)) converted
  single-key deep-stage Hold-to-repeat to genuine kernel autorepeat — so an Additive key with
  both stages Hold-to-repeat now emits **two `value=2` autorepeat streams from one finger
  press**.
- Both streams ride the **same** synthesized `RepeatSchedule` pulse cadence
  (`capture::analog`, primary Actuation point), so they fire **phase-locked** — a regularity
  tell a physical two-finger press could never produce. This is squarely the
  "Physical-plausibility ceiling" (ADR-0008) question.

### What "doesn't work quite right" — leads for the grilling (2026-09-07 code read)

1. **Deep stage has no independent `Repeat` source.** `capture::analog` only synthesizes a
   Hold-to-repeat pulse stream against the **primary's** Actuation point. A Hold-to-repeat
   deep Binding is kept repeating by `stage::Engine::deep_repeat`
   (`daemon/src/stage.rs:833`, `daemon/src/dispatch.rs:295`,`:322`) riding those primary
   pulses — added as a **post-ship correction** (`tartarus-dual-stage-keys-impl` #03, "Second
   correction"), after `dual_stage_additive_holds_both_stages…` had *codified* the deep stage
   firing only once. So the deep stage's repeat timing is not its own delay→period envelope
   from the deep crossing; it inherits whatever phase the primary's schedule is already in.
2. **Phase-lock / lockstep.** When both stages are Hold-to-repeat, every primary pulse also
   drives `deep_repeat` — the two keys autorepeat in perfect sync. (See plausibility point
   above.)
3. **`handle_event` special-casing.** Additive is threaded through the hot per-event path via
   `primary_handed_off` (`daemon/src/dispatch.rs:333`) and the `deep_repeat` call — narrow,
   Additive-shaped branches the other three modes don't need in that loop.
4. **History of corrections generally.** `tartarus-dual-stage-keys-impl` #03 carries three
   post-ship fixes clustered on the primary↔deep handoff / repeat interaction; Additive was
   entangled in each ("Additive test updated to expect…").

### The cut alternative — what it does and doesn't cover

Bind the **deep stage to a Macro** that presses both keys, under Handoff or No-Return.
- **Covers:** a single deep crossing that taps A+B; with a Hold-to-repeat multi-step Macro,
  *repeated* taps of A+B via the existing `run_toggle_loop`/`target_lap` floored path.
- **Does NOT cover** (grill this): Additive's actual signature — the **primary key held
  continuously** (its own `value=1`/`value=2` stream, uninterrupted) **while a second key is
  added** on the deep crossing and released on the deep exit. A one-shot Macro is
  fire-and-forget; a looped Macro machine-guns the whole A+B sequence rather than holding A
  and tapping B. Whether Acheron's Macro model can express "hold these keys until the deep
  band releases" at all is an open question.
- Trigger-mode composition: an Additive key today allows primary and deep to each be
  Fire-once / Hold-to-repeat / Toggle independently. A Macro deep stage inherits the
  Macro-path restrictions (no Analog-repeat + Macro — [ticket 09](09-disallow-analog-repeat-macro-binding.md);
  the value=2 single-key path is bypassed for multi-step Macros).

### To settle

1. **Fix or cut.** Is Additive's unique capability (continuous primary + added held deep)
   worth its complexity and plausibility cost — or does Handoff/No-Return + a Macro deep
   stage cover enough that Additive should go?
2. **If fix:**
   - Define the intended deep-stage repeat behaviour precisely: rides the primary's
     schedule (status quo), or gets its own delay→period envelope seeded from the deep
     crossing? Does the plausibility ceiling *require* de-phasing the two streams (jitter is
     out of scope per the map — but is a fixed offset different)?
   - Does the ceiling cap the *combined* rate of two simultaneous streams from one press, or
     is "each stream individually kernel-shaped" sufficient?
   - Scope: execution-in-scope on this map (like tickets 06/09) or a spec handoff?
   - ADR-0008 + `CONTEXT.md` "Physical-plausibility ceiling" wording: does a two-stream-from-
     one-press rule need a sentence?
3. **If cut:**
   - Migration: `StagingMode::Additive` variant removal, `dbus/wire.rs::staging_mode_from_str`,
     `daemon_stub._STAGING_MODES`, `binding_editor.STAGING_MODES`, the `additive()` table +
     tests in `stage.rs`. What happens to an existing `config.toml` with
     `staging_mode = "additive"` — hard `ConfigError` at load, or silent downgrade to
     Handoff/No-Return?
   - Do the README "Dual-stage keys" section and ADR-0007 need updating?
   - Is a "how to get the old Additive behaviour with a Macro" note owed to the new
     `## Output safety` / Dual-stage docs?
   - Execution-in-scope vs handoff.
4. **Downstream tickets.** Whichever way it goes, graduate the follow-on work (spec or
   removal) from the fog.

## Answer

Resolved by grilling with Charon (2026-09-07). **Cut Additive.**

### Decision — remove the mode

Additive is dropped. `StagingMode` becomes **Handoff / No-Return / Quick-Skip**.

**Rationale**
1. **No use case.** Nothing in the specs, and nothing Charon can name, needs Additive's
   signature capability (continuous primary + an *added* held deep stage, both from one
   grid-key press). Handoff / No-Return cover the "hand off to a deeper action" need;
   "fire several keys on the deep press" is a Macro bound to the deep stage.
2. **The output is physically impossible, not merely a tell.** A real keyboard autorepeats
   only the *most-recently-pressed* key — you cannot hold two keys in autorepeat at once.
   Additive with both stages Hold-to-repeat emits **two concurrent `value=2` streams** from
   one press (the `value=2` rebuild, [ticket 07](07-spec-kernel-shaped-repeat.md), made the
   single-key deep-stage repeat genuine kernel autorepeat; `stage::Engine::deep_repeat`
   drives it off the *primary's* `RepeatSchedule` pulses, so the two streams are also
   phase-locked). That is a shape no physical keyboard produces — squarely an ADR-0008
   ("Physical-plausibility ceiling") violation, not a borderline regularity question.
3. **Always the shakiest mode.** `tartarus-dual-stage-keys-impl` #03 carried three post-ship
   corrections clustered on the primary↔deep repeat interaction, Additive entangled in each.
4. **Clean invariant after the cut.** With Additive gone, **no staging mode ever emits two
   concurrent autorepeat streams**: Handoff / No-Return release the primary in the deep band
   (`primary_handed_off`), Quick-Skip's `Skipped` phase suppresses it entirely. This is the
   outcome ADR-0009 records.

**Settled sub-questions**
- **Q2 (ceiling: per-stream or aggregate?)** — moot. The specific output Additive produces
  (two held autorepeating keys) is impossible on real hardware regardless of how the ceiling
  is read, so the question does not need answering to decide this ticket. (A user who wants a
  fast alternating-keystroke macro that *looks* like autorepeat can build one — that is on
  them, and it is the Macro exception working as intended.)
- **Q3 (phase-lock disqualifying?)** — a 2-key looping Macro produces the same phase-locked
  signature and the ADR already accepts that as the Macro exception's cost; the issue with
  Additive is impossibility (Q2), not phase-lock.

### Migration — hard `ConfigError` at load

An existing `config.toml` with `staging_mode = "additive"` is **rejected at load** with a
clear, Additive-specific error — no silent downgrade. There are no known users of the mode,
and dual-stage keys is a new niche feature, so a hard break is acceptable and is the most
explicit. **Mechanism is [ticket 13](13-remove-additive-staging-mode.md)'s call** under one
constraint: the error must name Additive specifically (not a generic serde parse failure).
Noted preference — keep `Additive` a serde-parseable variant and reject it in
`load_or_seed` / `config::validate` with a dedicated `ConfigError` variant, rather than a
custom `Deserialize` that errors opaquely.

### Record — new ADR-0009

**New `docs/adr/0009-*.md`** — "Additive staging mode removed: a real keyboard can't hold
two autorepeating keys." Cites ADR-0008 (the reason) and ADR-0007 (the 4-mode context);
one-line pointers added to both ADRs. `CONTEXT.md` `Additive` entry removed and the
`Staging mode` list trimmed to three — done by ticket 13, alongside the code (the ticket-12
precedent: glossary/ADR edits land with the change, not ahead of it). Not an amendment to
ADR-0007 (narrow "depth interpretation runs in dispatch" thesis) or ADR-0008 (ticket 12's
fire-once-dwell note is a *consistency extension*; a shipped-feature removal is a discrete,
surprising, hard-to-reverse decision that earns its own ADR).

### Scope & downstream

**Execution-in-scope on this map**, like tickets 06 / 09 — not a spec handoff. Graduated as
**[ticket 13](13-remove-additive-staging-mode.md)** (`Remove the Additive staging mode`,
Type: task, `Blocked by: 11`). No README "how to get the old behaviour with a Macro" note
(Charon's call — the old behaviour was impossible, so such a note would only describe a
different, simulated thing).

The map is done once ticket 13 and [ticket 12](12-splice-the-fire-once-keypress-dwell.md)
both land — then the destination is reached and it archives.
