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
- [Dual-stage state machine](./issues/02-dual-stage-state-machine.md) — full event-sequence
  state tables for Handoff/No-Return/Additive/Quick-Skip. Same-report double-crossings are
  handled by **mechanical replay** (no fast-ramp short-circuiting) resolved
  **synchronously per-event** via the arriving event's own `.depth` field (reusing
  `begin_quick_skip`'s trick for all four modes, not just Quick-Skip), closing a
  `tokio::select!` ordering race ticket 01 hadn't fully covered. Fire/Release ops are
  uniformly `decide(binding, Down/Up, slot)` + `perform` against the stage's own `Slots`
  map — no new per-trigger-mode logic. Quick-Skip is a per-press runtime state (Armed →
  Skipped/Late); an early Up or a Layer/Profile/capture-mode change while Armed cancels
  the buffered primary outright. No new `config::validate` rule needed. Flags the
  primary-`ProfileSwitch`-mid-press interaction forward into
  [ticket 05](./issues/05-interaction-sweep.md) (already in its scope).
- [Config/D-Bus surface](./issues/03-config-dbus-surface.md) — `DeepStageConfig{actuation,
  mode}` bundled into one sparse `deep_stages: HashMap<Input, DeepStageConfig>`, plus parallel
  `deep_base`/`deep_held: HashMap<Input, Binding>` (mirrors `chords_base`/`axis_base`); no
  `schema_version` bump. Seven new `ConfigError` variants (`InvalidDeepStageInput`,
  `DeepStageReleaseNotBelowActuation`, `DeepStageBandOverlapsPrimary`,
  `DeepStageWithoutPrimary`, `DeepStageMissingConfig`, `AnalogRepeatOnDualStageKey`,
  `ChordMemberDeepStageConflict`), appended to `validate` after the existing checks.
  `profile_all_binding_sites` chains `deep_base`/`deep_held` in as `BindingSite::Individual`
  so `check_binding` (ticket 14's seam) covers deep Bindings for free. Four granular `Edit`
  variants mirroring the primary stage's own split — `SetDeepStage`/`ClearDeepStage` (the
  Binding, per-Layer) and `SetDeepActuation`/`SetStagingMode` (the config, per-Profile shared
  Base/Held) — all relying on the trailing `config::validate(&next)?`, no inline checks, no
  new `Effect`s (the deep-band engine lives in dispatch and reads `Config` directly). Live
  force-release-on-edit and cascade-delete-on-primary-clear are explicitly flagged forward to
  [ticket 05](./issues/05-interaction-sweep.md), not resolved here. `StagingMode` marshals as
  a flat lowercase D-Bus string (`handoff`/`no_return`/`additive`/`quick_skip`), same
  convention as `AxisTarget`. No `GetState()`/`active_toggles` change. **`rules.py` gets
  nothing** — all seven new rules are whole-`Config` checks and land in `daemon_stub.py`
  only.
- [Dual-stage binding-editor layout](./issues/04-dual-stage-binding-editor-layout.md) —
  **Variant A, "swap toggle"**, confirmed live against the real Key/Controller-button
  pickers: one shared 4-marker Actuation bar (fixed-width, tuned to the real picker row's
  own natural width — a live/hexpand width made dragging jump), a Primary/Deep toggle row
  where `+ Add deep stage` occupies the Deep slot until pressed then becomes `[Deep]` + a
  square red `✕` (tooltip "Remove deep stage"), a staging-mode row below that shown only
  once a deep stage exists, and a single editor slot below holding the real Trigger/Action
  fields (incl. the actual `key_picker`/`controller_picker` widgets, deep stage's own
  selection tinted the deep-actuation marker's blue) for whichever stage is toggled —
  structurally only one picker ever mounted (Q12). Variants B (stacked bars + accordion)
  and C (tabs) were built and rejected. Prototype:
  `prototype/04-dual-stage-binding-editor-layout/prototype.py` on `dev`.
- [Interaction sweep](./issues/05-interaction-sweep.md) — held/mid-press dual-stage key vs.
  the rest of the runtime, all six areas settled: **Layer switch** and **Profile switch**
  both force-release both stages via a new `Effect::StopAllStages` (`stage::Engine.stop_all()`),
  deliberately stricter than the plain-key precedent — correcting tickets 01/02's assumption
  that Layer switch already force-releases (it doesn't; it live-rebinds the next event
  against the new Layer instead, and that quirk isn't propagated into dual-stage). A
  Profile-switch fired by the deep stage's own Action completes its firing before `stop_all()`
  tears down, mirroring the Chord-member-fires-`ProfileSwitch` precedent via
  `commit_input_edits`. **Capture-mode flip to Digital** reuses the same `stop_all()` at the
  existing `analog_repeat.stop_all()` call site; primary inherits existing Digital-transition
  behavior unchanged. **Output suppression**: Chord-style suppression is moot (a dual-stage
  key can never be a Chord member); GUI-focus `StopAllToggles` is extended to drain the deep
  `Slots<StageKey>` too. **Cascade-delete**: new per-key `Effect::StopStage(input)` force-releases
  a live deep stage immediately on a primary delete/overwrite; no GUI confirmation dialog
  (verified `binding_editor.py` confirms no Binding delete today, primary or otherwise).
  **Reconnect**: a new, explicit disconnect hook in `handle_connection_change`'s
  `connected == false` branch resets the engine's per-key state — corrected finding: no
  generic disconnect mechanism exists today for *anything* (Analog-repeat has no dropout
  handling either); that pre-existing gap is explicitly out of scope, not fixed here.
  **Chord-window-vs-non-member proximity**: confirmed non-issue by inspection
  (`chord::feed`'s membership gate), no decision needed.
- **Cross-effort (2026-09-04):** post-release-development ticket 15 landed
  `trigger::Slots<K>` — the `(firings, toggles)` handle pair with both maps private
  (`slot` firing-wins / `snapshot` toggle-wins / `perform` / `force_release` /
  `stop_toggle` / `stop_all_toggles`). `stage::Engine` should hold a
  `Slots<StageKey>` rather than a fourth hand-rolled copy of the pair;
  `perform_trigger<K>` / `slot_for` / the `trigger_ctx!` macro named in ticket 01
  §2/§4 no longer exist (absorbed as `Slots` methods, `trigger::compile_action`
  moved alongside). Ticket 01's design intent is unchanged — spec ticket 06 folds
  in the new names.
- [Write the dual-stage spec](./issues/06-write-dual-stage-spec.md) — pure consolidation, no
  new decisions. [`spec.md`](./spec.md) written (feature summary, the Actuation-stage model,
  the four Staging-mode event tables, config schema + `config.toml` example, D-Bus `Edit`
  surface + seven `ConfigError` variants, the swap-toggle GUI layout, the runtime-behavior /
  interaction section, Digital-mode degradation, testing decisions, out-of-scope). ADR-0007
  filed. `CONTEXT.md` gained six terms (`Actuation stage`, `Staging mode`, `Handoff`,
  `No-Return`, `Additive`, `Quick-Skip`) — one entry per Staging mode, mirroring Trigger
  mode's own governing-concept-plus-variants structure; `primary stage` did not earn its own
  line (covered by contrast inside the `Actuation stage` entry). README user-facing copy ruled
  a non-deliverable, left to the implementation effort. `.scratch/README.md` flipped to "spec
  ready". Map destination reached — implementation is a fresh effort.

## Not yet specified

<!-- in-scope fog; graduates to tickets as the frontier advances -->

Empty — both patches graduated with [ticket 06](./issues/06-write-dual-stage-spec.md): README
copy was ruled a spec non-deliverable (left to the implementation effort, see spec.md's Out of
Scope), and the glossary-entry-count question was settled as one entry per Staging mode,
mirroring Trigger mode's own structure.

## Destination reached

[**`spec.md`**](./spec.md) is written and gated — every ticket (01–06) is resolved, nothing on
this map is open. [ADR-0007](../../docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md)
and the `CONTEXT.md` terms (`Actuation stage`, `Staging mode`, `Handoff`, `No-Return`,
`Additive`, `Quick-Skip`) are filed alongside it. **Implementation is a separate, fresh effort**
that does not resume this map — this map is done and can be archived (flip its `.scratch/README.md`
line to "spec ready" → "archived" once that implementation effort opens, same precedent as
`tartarus-status-leds`).

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
- **Fixing Analog-repeat's pre-existing lack of dropout/disconnect handling** — surfaced
  while resolving [ticket 05](./issues/05-interaction-sweep.md)'s reconnect question
  (Analog-repeat has no disconnect hook today and would sit stale on frozen Depth across a
  replug, with or without dual-stage keys). Pre-existing, unrelated to this feature; the new
  deep-stage engine gets its own explicit disconnect hook so it doesn't inherit the same gap,
  but the existing gap itself is a separate effort.
