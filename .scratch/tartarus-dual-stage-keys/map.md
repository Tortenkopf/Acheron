Label: wayfinder:map

# Tartarus dual-stage grid keys

## Destination

A reviewed **`spec.md`** for "Dual-stage keys" — a grid key in analog Capture mode may
carry a **second Actuation stage** (its own Actuation/Release point pair + its own
`Binding`) that fires at a Depth strictly deeper than the primary stage, with a
user-selectable **staging mode** governing how the two stages hand off. Plus the
`CONTEXT.md` terms and one ADR. **Ready to hand to a separate implementation effort** —
the destination is the spec, not the build.

No kill-gate — this is all software. [Ticket 01](./issues/01-staged-event-pipeline.md)
de-risks the one non-trivial architectural question (how staged events flow through the
capture-source → dispatch pipeline) before the spec locks, but it is a design decision,
not a feasibility gate.

## Notes

**This map plans, it does not execute.** Every ticket resolves to a decision. The one
`prototype` ticket ([04](./issues/04-dual-stage-binding-editor-layout.md)) builds a
*throwaway* GUI layout under `prototype/` to react to — not a step toward the shipped
editor.

**Precedent:** modelled on `tartarus-status-leds` — charting → gated `spec.md` +
`CONTEXT.md` terms + ADR → fresh implementation effort. Same discipline: the spec is the
deliverable, implementation is a separate effort that does not resume this map.

### Grounding facts found while charting (2026-09-03)

- `ActuationPoint { actuation: u8, release: u8 }` (`daemon/src/config.rs:367`) is per-Input
  per-Profile, **shared across Base and Held** — `default_actuation` +
  sparse `actuation_overrides: HashMap<Input, ActuationPoint>` (`config.rs:146/151`).
  Hysteresis invariant `release < actuation` is checked at the `Command`/`config::validate`
  layer (`ConfigError::ReleaseNotBelowActuation`, `config.rs:907/1280`), not in the type.
- `Binding { trigger: TriggerMode, action: Action }` (`config.rs:351`), keyed by `Input`
  per Layer: `base` / `held` sparse maps on `Profile` (`config.rs:133/138`). Parallel
  per-Layer maps already exist for Chords (`chords_base`/`chords_held`) and Axis
  assignments (`axis_base`/`axis_held`) — the idiom for "a second thing keyed like a
  Binding."
- The analog capture source (`daemon/src/capture/analog.rs`) synthesizes **one** Down/Up
  per grid key from **one** `ActuationPoint` via the pure `observe(prev, depth, point)`
  (`analog.rs:235`) — crossing up through `actuation` fires `Down`, down through `release`
  fires `Up`. The dispatch `InputEvent` carries `(input, state, depth)` with **no stage
  discriminator**.
- Two Depth-fed engines **already run in the dispatch task** off the `rx_depth`
  `watch<HashMap<Input,u8>>`: Axis conflict resolution (`handle_depth_update`,
  `dispatch.rs:379`) and the Analog-repeat rate curve (`update_analog_repeats`,
  `dispatch.rs`, `daemon/src/analog_repeat.rs`). Staged-depth interpretation is a
  natural third — this is ticket 01's leaning.
- Chord detection uses a **~50ms** window between first and last member Input going down;
  a member's individual Binding is **suppressed** for a press the Chord claims
  (`CONTEXT.md`: Chord). Quick-Skip reuses this constant and this suppression pattern.
- Analog-repeat is grid-key-only, fires from a depth-driven background task with a
  **fixed deadzone** (`ANALOG_REPEAT_DEADZONE`, `analog_repeat.rs:135`) that ignores the
  key's Actuation point, and holds the key solid above a near-full-travel threshold.
- GUI Actuation bar: `build_actuation_section` (`gui/acheron_gui/binding_editor.py:246`),
  a 2-marker draggable bar (green Actuation, amber Release) with collision-constraint
  keeping Actuation strictly above Release (`binding_editor.py:298`), plus a live Depth
  fill (`ticket 26`). The key/controller-button picker is large and must never be shown
  twice at once (user constraint, Q12).
- `force_release_stuck` (`dispatch.rs:351/361`, `trigger` module) is the precedent for
  releasing a held Binding on a Layer/Profile switch or capture-mode flip.

### Decisions settled during the charting grilling — not to be re-litigated per ticket

- **Destination = gated `spec.md`** handed to a fresh impl effort (Q1). No kill-gate.
- **Terminology (Q2):** the concept is an **Actuation stage** — a `(ActuationPoint,
  Binding)` pair. A grid key has a **primary stage** and an optional **deep stage**.
  User-facing feature name: **"Dual-stage keys."** Glossary entries land with
  [ticket 06](./issues/06-write-dual-stage-spec.md) (lazy discipline — no `CONTEXT.md`
  entry until the model is settled and known to survive).
- **Staging mode (Q3):** a user choice per key, one of four:
  - **Handoff** — crossing the deep band releases the primary stage and presses the deep
    stage; crossing back down releases the deep stage and re-presses the primary. Exactly
    one stage held at a time; symmetric (camera-shutter model).
  - **No-Return** — like Handoff going deeper, but on the way back up the primary does
    **not** re-fire.
  - **Additive** — both stages fire and are held simultaneously; deeper press adds the
    deep stage, does not release the primary.
  - **Quick-Skip** — if the deep band is reached within ~50ms of crossing the primary
    Actuation point, the primary's Down is **suppressed** (never fires); otherwise the
    primary fires (≤50ms late) and the key behaves as **Handoff** for the rest of that
    press. Release from deep when the primary was skipped does not fire the primary
    (release path = No-Return). Costs up to 50ms latency on the primary Down.
- **Cap at 2 stages (Q5)** — primary + deep only. N-stages out of scope (key travel makes
  it impractical).
- **Deep stage requires a primary Binding on that Layer (Q6).** No primary ⇒ no deep
  stage; deleting the primary cascade-deletes the deep stage (detail:
  [ticket 05](./issues/05-interaction-sweep.md)).
- **Digital Capture mode (Q7):** the deep stage is **silently inert** — only the primary
  Binding fires (like Analog-repeat degrading to Hold-to-repeat). GUI greys the
  deep-stage controls with a "requires analog" note.
- **Depth-ordering constraint (Q4):** the two hysteresis bands are **fully disjoint and
  stacked** — `deep.release > primary.actuation`. Rejected at the `Command` /
  `config::validate` layer, a sibling of `ReleaseNotBelowActuation`.
- **Scoping (Q8):** deep **Binding** is **per-Layer** (parallel `deep_base` / `deep_held`
  maps); deep **ActuationPoint** and **staging mode** are **per-Input per-Profile, shared
  Base/Held** (they interpret physical travel, like the primary Actuation point). A deep
  stage configured on Base only still owns the shared deep slider pair + mode — same as
  primary Actuation today.
- **Trigger mode per stage (Q10):** each stage is a full `Binding` with its own
  independent Trigger mode; all existing per-Action validation applies to each stage
  independently. **Exception:** a key that has a deep stage may use **Analog-repeat on
  neither stage** (v1) — the Analog-repeat background task ignores the Actuation point and
  would fight the staging logic. New validation rule. Revisit post-v1.
- **Interactions (Q11):**
  - **Chord member ⇒ no deep stage** (v1) — the two depth interpretations tangle.
    Enforced at `SetChordBinding` and the deep-stage setter.
  - **Axis assignment** — already mutually exclusive with any Binding on an Input/Layer;
    the deep stage is a Binding, so it is covered for free.
  - **Profile Switch as the deep-stage Action — allowed.** The deep stage runs through
    ordinary dispatch, not the Chord state machine, so `InvalidChordProfileSwitch` has no
    analogue here.
- **No new `GetState()` field, no new signal (Q14)** — the live Depth bar with 4 markers
  shows which band is hot. Config is the single source of truth. Same call status LEDs
  made.
- **ADR warranted (Q15):** "dual-stage depth interpretation runs in the dispatch task,
  not the analog capture source" — drafted in [ticket 01](./issues/01-staged-event-pipeline.md)'s
  answer, filed by [ticket 06](./issues/06-write-dual-stage-spec.md). Contingent on
  ticket 01 landing on route (b).
- **GUI prototype (Q12):** [ticket 04](./issues/04-dual-stage-binding-editor-layout.md)
  builds a throwaway layout. Hard constraint: **never two key/controller-button pickers
  on screen at once** — the picker is large.

**Skills to consult:** `/grilling` + `/domain-modeling` for every `grilling` ticket
(01, 02, 03, 05); `/prototype` for ticket 04; `/codebase-design` where a ticket touches
module seams (01, 03).

## Decisions so far

<!-- one line per closed ticket: enough to judge relevance, then zoom the link -->

- [Staged-event pipeline](./issues/01-staged-event-pipeline.md) — **Route (b)**: capture
  source unchanged (no `PhysicalEvent.stage` field), all staged-depth interpretation in a
  new pure `daemon/src/stage.rs` + non-pure `stage::Engine` on `DispatchState` (third
  sibling of `axis` / `analog_repeat`, runs in the existing `rx_depth.changed()` arm).
  Deep-band hysteresis reuses the pure `observe()`. Deep stage fires via the generic
  `perform_trigger<K>` keyed by a new `StageKey(Input)`, into `ChordRuntime`-shaped
  `firings`/`toggles` on the engine. **Quick-Skip** suppresses/delays the primary Down via
  a dispatch-side buffer modelled on the Chord machine — one narrow `handle_event`
  divert (Quick-Skip keys only, gated `depth.is_some()`) + a new 4th `select!` arm
  (`wait_for_stage_deadline`, mirroring the chord deadline trio); non-Quick-Skip modes
  never intercept the primary edge. `stage::Engine::stop_all()` wired to the
  `analog_repeat.stop_all()` teardown sites. `rx_depth` is already the raw un-throttled
  stream (30 Hz limit is D-Bus-only) — fast enough for the 50 ms window; **no spike**.
  ADR-0007 drafted in the ticket answer for [ticket 06](./issues/06-write-dual-stage-spec.md)
  to file.
- **Cross-effort (2026-09-04):** post-release-development ticket 15 landed
  `trigger::Slots<K>` — the `(firings, toggles)` handle pair with both maps private
  (`slot` firing-wins / `snapshot` toggle-wins / `perform` / `force_release` /
  `stop_toggle` / `stop_all_toggles`). `stage::Engine` should hold a
  `Slots<StageKey>` rather than a fourth hand-rolled copy of the pair;
  `perform_trigger<K>` / `slot_for` / the `trigger_ctx!` macro named in ticket 01
  §2/§4 no longer exist (absorbed as `Slots` methods, `trigger::compile_action`
  moved alongside). Ticket 01's design intent is unchanged — spec ticket 06 folds
  in the new names.

## Not yet specified

<!-- in-scope fog; graduates to tickets as the frontier advances -->

- **`rules.py` mirror detail** — the GUI's validation mirror needs the new rules
  (disjoint-band constraint, Analog-repeat-on-dual-stage ban, Chord-member ban). Exact
  shape folds into [ticket 03](./issues/03-config-dbus-surface.md) or the impl effort.
- **README / user-facing copy** — the "Dual-stage keys" feature section, mode
  descriptions, the driving-sim framing. Folds into [ticket 06](./issues/06-write-dual-stage-spec.md).
- **Exact validation-error strings** — `ConfigError` variant names and `Display` text for
  the new rules. Folds into [ticket 03](./issues/03-config-dbus-surface.md).
- **Whether the four staging modes each get a full `CONTEXT.md` glossary entry or one
  combined "Staging mode" entry** — settled when [ticket 06](./issues/06-write-dual-stage-spec.md)
  writes the terms.

## Out of scope

<!-- ruled beyond this destination; never graduates -->

- **Three or more Actuation stages** — capped at 2 (Q5); key travel makes more
  impractical.
- **A deep stage with no primary Binding** — the model is "primary + optional second"
  (Q6).
- **Analog-repeat on either stage of a dual-stage key** — v1 restriction (Q10); revisit
  as a fresh effort.
- **Deep-stage participation in Chord detection** — only the primary stage is
  Chord-eligible, and a Chord member cannot have a deep stage at all in v1 (Q11).
- **A daemon→GUI "active stage" signal / `GetState()` field** — the visual Depth bar
  covers it (Q14).
- **Any digital-mode approximation of the deep stage** — inert in Digital mode, full stop
  (Q7).
- **The implementation itself** — a separate effort, handed the finished `spec.md`.
