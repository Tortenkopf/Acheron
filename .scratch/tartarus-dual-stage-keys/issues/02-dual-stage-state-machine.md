Type: grilling
Blocked by: 01
Status: resolved

## Question

Nail the **exact event sequence for all four staging modes** — the precise Down/Up
ordering a dual-stage key produces as Depth ramps up through both bands, dwells, and ramps
back down, for **Handoff, No-Return, Additive, Quick-Skip**.

Grilling + `/domain-modeling`. Decision only. Build on [ticket 01](./01-staged-event-pipeline.md)'s
chosen pipeline route and state-machine home.

### Settled inputs (do not re-open)

- Mode definitions in `map.md` Notes (Q3). Bands are disjoint and stacked.
- Quick-Skip window = ~50ms (Chord constant); primary Down suppressed if deep reached
  inside it, otherwise primary fires late and the key runs as Handoff for the rest of the
  cycle; release from a skipped primary does not fire it.

### Settle at least

- A **state table** per mode: `(prev band, new band, mode) -> [emitted events]`, covering
  every transition including skipping a band in one report (fast ramp) and the dwell case
  where `observe()` emits nothing.
- **Handoff release path** — deep Up then primary Down as Depth falls through the deep
  band: exact order, and whether the primary's *Down* on the way back up respects the
  primary's own hysteresis (`release`/`actuation`) or just fires.
- **Additive** — order of the two Ups as Depth falls (deep first, then primary), and what
  happens if Depth drops straight from full to zero in one report (both Ups, order).
- **Quick-Skip** — the buffered/delayed primary Down: does a Layer/Profile switch during
  the 50ms window cancel it? Does releasing below the primary band during the window
  cancel it (yes — never crossed deep, never held long enough)?
- **Trigger-mode composition** — each stage has its own Trigger mode. Spell out
  Handoff + (primary Toggle / deep Fire-once), Additive + (both Hold-to-repeat), etc. Does
  "release the primary stage" on a Handoff mean *stop its Toggle* or *send a synthetic
  Up*? (Compare `StopAllToggles` vs `force_release_stuck`.)
- **Macro on a stage** — a running primary Macro when Handoff hands off to deep: does it
  stop, or run to completion? (Compare Toggle-Macro looping semantics.)
- Whether any mode needs a `config::validate` rule beyond the disjoint-band constraint.

### Output

An `## Answer` with the four state tables and every edge case resolved, ready for ticket
06 to transcribe into `spec.md`.

## Answer

Resolved by a grilling session against the real pipeline (`observe()` in
`capture/analog.rs`, `trigger::decide`/`Slots<K>`, `relay_grid_blocking`'s report loop,
`chord.rs`'s race analysis), building on [ticket 01](./01-staged-event-pipeline.md)'s
Route (b) architecture, 2026-09-04.

### Grounding facts established by inspection (not decisions)

- **Deep-before-primary ordering is forced, not chosen.** `deep.release > primary.actuation
  > primary.release` (the disjoint-band constraint) means depth can never be low enough to
  cross `primary.release` while still `>= deep.release`. "Deep always releases before
  primary re-engages" falls out of the math, not a state-machine rule.
- **A synthetic `RepressPrimary` never needs to re-check primary's own hysteresis** — by
  the same math, the depth at which it fires is always `>= primary.actuation`. Fires
  unconditionally.
- **A running Macro on a stage can never be interrupted.** `FiringHandle::force_release_
  stuck` (`executor.rs:248`) is a no-op against a balanced Macro (only cleans up an
  unbalanced bare `KeyDown`); a Fire-once spawn has no `CancellationToken` (unlike
  `ActiveToggle`). A primary Macro always runs to completion when Handoff hands off —
  inherited precedent, nothing dual-stage-specific.
- **"Release the primary/deep stage" is never a new primitive** — always whatever an
  ordinary Up would do to that key's live slot: `Slots::stop_toggle` if it's holding a
  Toggle, `Slots::force_release` otherwise.
- **Fire ops = `decide(binding, Down, slot)` + `perform`; Release ops = `decide(binding,
  Up, slot)` + `perform`**, against the stage's own `Slots` map (`self.individual` for
  primary, `self.stage: Slots<StageKey>` for deep). A stage's synthetic edge is
  indistinguishable to `decide` from a physical one — no new per-trigger-mode logic
  anywhere. Consequence: `RepressPrimary` on a Toggle primary always starts a **fresh**
  Toggle loop, never resumes the old one (`decide(Toggle, Down, None)` has no "resume").
  Intentional, not a bug.
- A single incoming hidraw report delivers one new depth value, so a same-report "skip"
  can only jump thresholds in **one** direction: `(Up,Up)→(Down,Down)` or
  `(Down,Down)→(Up,Up)` directly — never both in sequence within one report.

### Settled architecture decisions

1. **Same-report double-crossing: mechanical replay, not short-circuit.** The full
   logical op sequence for the mode always walks, even collapsed into one tick — no
   fast-ramp special-casing in the tables. Matches how Axis/Analog-repeat/Chord already
   don't special-case fast ramps.
2. **Ordering is made deterministic per-event, not per-channel-race.** For all four modes
   (not just Quick-Skip), `handle_event` resolves a same-report double-crossing
   synchronously using the arriving event's own `.depth` field — reusing
   `begin_quick_skip`'s "resolves immediately if the deep band is already hot" trick
   (ticket 01 §3) — rather than depending on `rx_events`/`rx_depth.changed()` arrival
   order under `tokio::select!`'s unordered tie-break.
3. **Quick-Skip: an early Up (before deadline, before deep reached) cancels the pending
   primary entirely** — the buffered Down is dropped for good, nothing fires, no
   compensating synthetic event. Same cancellation path as a Layer/Profile switch or
   capture-mode flip during the window (`stage::Engine::stop_all()`, ticket 01 §5).

### State tables

Combined state = (primary band, deep band) ∈ {Up,Down}²; `(Up,Down)` is structurally
impossible per the grounding facts above.

**Handoff**

| Transition | Emitted ops, in order |
|---|---|
| (Up,Up)→(Down,Up) | Primary Down (real) |
| (Down,Up)→(Down,Down) | Release Primary → Fire Deep |
| (Down,Down)→(Down,Up) | Release Deep → Repress Primary |
| (Down,Up)→(Up,Up) | Primary Up (real) |
| (Up,Up)→(Down,Down), 1-report skip | Primary Down (real) → Release Primary → Fire Deep |
| (Down,Down)→(Up,Up), 1-report skip | Release Deep → Repress Primary → Primary Up (real) |
| no crossing | Nothing |

**No-Return** — identical to Handoff except the down-direction never represses:

| (Down,Down)→(Down,Up) | Release Deep only — key goes quiet; primary's eventual real Up is a no-op (its slot was already released) |
| (Down,Down)→(Up,Up), 1-report skip | Release Deep → Primary Up (real) — no repress inserted |

(up-direction rows identical to Handoff's table)

**Additive** — the engine never touches primary at all:

| Transition | Emitted ops, in order |
|---|---|
| (Up,Up)→(Down,Up) | Primary Down (real) |
| (Down,Up)→(Down,Down) | Fire Deep |
| (Down,Down)→(Down,Up) | Release Deep |
| (Down,Up)→(Up,Up) | Primary Up (real) |
| (Up,Up)→(Down,Down), 1-report skip | Primary Down (real) → Fire Deep |
| (Down,Down)→(Up,Up), 1-report skip | Release Deep → Primary Up (real) |
| no crossing | Nothing |

**Quick-Skip** — a per-press runtime state (Armed → Skipped / Late) layered on top of
Handoff's mechanics:

| From Armed | Result |
|---|---|
| Deep reached within window (incl. resolved synchronously if already hot on the triggering report) | → **Skipped**: Suppress Primary (buffered Down dropped for good) → Fire Deep |
| Deadline elapses, deep never reached | → **Late**: Repress Primary (retroactive, via `dispatch_individual_down`) → runs as ordinary Handoff for the rest of the press |
| Up arrives first (deadline not elapsed, deep never reached) | → cancelled: buffered Down dropped, nothing emitted |
| Layer/Profile switch or capture-mode flip while Armed | → cancelled via `stage::Engine::stop_all()`, same as above |

Once **Skipped**, every subsequent dip into/out of deep for the rest of *this* press
behaves like Additive-with-no-primary (Fire Deep / Release Deep each crossing, primary
permanently inert) — "release path = No-Return" governs every release for the rest of the
press, not just the first. Once **Late**, the rest of the press runs plain Handoff.

### Trigger-mode composition

No per-combination logic needed beyond the "Fire ops = Down through `decide`, Release ops
= Up through `decide`" rule above — FireOnce/HoldToRepeat/Toggle on either stage behave
exactly as they already do for a physical Down/Up. Worked examples:

- **Handoff, primary=Toggle, deep=FireOnce**: primary Toggle loop starts on the real Down;
  `Release Primary` → `Slots::stop_toggle` (loop stops, force-released) when deep engages;
  `Fire Deep` → `SpawnFireOnce`; `Release Deep` is a no-op/cleans-a-stray-hold when the
  dip ends; `Repress Primary` → a **fresh** `StartToggleLoop`, not a resume.
- **Additive, both Hold-to-repeat**: primary holds/repeats on its own real Down/Repeat
  cadence, completely untouched; `Fire Deep` spawns deep's own hold-or-repeat via the same
  `decide` arms; `Release Deep` runs `decide(HoldToRepeat, Up)` → `ForceReleaseStuck`
  against `self.stage`'s slot only — primary's own slot is never touched.
- **Macro on a stage**: always runs to completion regardless of any Release op targeting
  it (grounding fact above) — no dual-stage-specific interruption exists.

### Config validation

No new `config::validate` rule beyond what's already settled (disjoint-band,
no-AnalogRepeat-on-either-stage, no-Chord-member-with-deep-stage). Nothing in the
state-machine semantics itself needs a new config-time check.

### Flagged forward, not resolved here

A primary `Action::ProfileSwitch` fires immediately on the ordinary Down (unintercepted
for Handoff/No-Return/Additive) — since deep ActuationPoint/mode are per-Profile (Q8),
that switch changes *which* deep stage (if any) governs the rest of the physical press,
mid-gesture. Already within [ticket 05](./05-interaction-sweep.md)'s "Profile switch
mid-press" scope — no new ticket needed, just noting the concrete trigger for that
session to pick up.
