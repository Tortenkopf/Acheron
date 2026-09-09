<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 19 — One `DispatchState::tear_down(reason)` match for lifecycle teardown; merge the two deadline helpers

**What to build:** Two changes to `dispatch.rs`, no new module, no trait, no
behaviour change.

1. **`DispatchState::tear_down(&mut self, reason: TeardownReason)`** — a single
   `match reason { … }` that is the *one place* the "what gets released on a
   Layer switch / Profile switch / disconnect / Digital-mode flip" matrix
   lives. Every arm names every participant — `axis`, `analog_repeat`, `stage`,
   `individual` (firings, and toggles separately), `chord_machine`,
   `chord_slots` — with an explicit `//` line for each participant it
   deliberately leaves alone.

   ```rust
   // daemon/src/dispatch.rs
   #[derive(Clone, Copy, Debug, PartialEq, Eq)]
   pub(crate) enum TeardownReason {
       /// Mode key edge under `ModeKeyRole::LayerSwitch` — `active_layer`
       /// flipped. Individual Toggles deliberately survive (a Toggle held
       /// across a Layer switch keeps running — CONTEXT.md Toggle).
       LayerSwitch,
       /// `Edit::SwitchProfile` committed. Strongest sweep: individual
       /// Toggles drain too. Chord Toggles still survive (edit.rs — an active
       /// Chord Toggle survives a Profile switch today).
       ProfileSwitch,
       /// Device reported disconnected.
       Disconnect,
       /// Capture mode flipped to Digital (no Depth).
       CaptureModeToDigital,
   }

   impl DispatchState {
       async fn tear_down(&mut self, reason: TeardownReason) {
           match reason {
               TeardownReason::LayerSwitch => {
                   for w in self.axis.reset() {
                       let _ = self.injector.set_axis_value(w.code, w.value).await;
                   }
                   self.analog_repeat.stop_all().await;
                   self.stage.stop_all(&self.injector).await;
                   self.individual.drain_firings(&self.injector).await;
                   // individual.toggles: survive a Layer switch (CONTEXT.md).
                   // chord_machine / chord_slots: NOT reset today — see ticket 20.
               }
               TeardownReason::ProfileSwitch => {
                   self.individual.stop_all_toggles().await;
                   self.individual.drain_firings(&self.injector).await;
                   for w in self.axis.reset() {
                       let _ = self.injector.set_axis_value(w.code, w.value).await;
                   }
                   self.analog_repeat.stop_all().await;
                   self.stage.stop_all(&self.injector).await;
                   // chord_slots: Chord Toggles survive a Profile switch (edit.rs) — ticket 20.
                   // chord_machine: not reset — ticket 20.
               }
               TeardownReason::Disconnect => {
                   self.stage.stop_all(&self.injector).await;
                   self.individual.drain_firings(&self.injector).await;
                   // axis / analog_repeat: NOT torn down on disconnect today.
                   //   analog_repeat has no dropout handling at all (dual-stage
                   //   spec.md, Out of Scope). — ticket 20.
                   // chord_*: untouched. — ticket 20.
               }
               TeardownReason::CaptureModeToDigital => {
                   self.analog_repeat.stop_all().await;
                   self.stage.stop_all(&self.injector).await;
                   self.individual.drain_firings(&self.injector).await;
                   // axis: NOT reset on the Digital flip today. — ticket 20.
                   // chord_*: untouched. — ticket 20.
               }
           }
       }
   }
   ```

   The match body above **is today's behaviour**, transcribed from the four
   call sites — this ticket changes none of it (ticket 20 decides whether the
   `NOT torn down` cells become real calls).

2. **Merge `wait_for_chord_deadline` and `wait_for_stage_deadline`** — they are
   byte-identical bar the name — into one `wait_for_deadline(Option<Instant>)`.
   The two `select!` arms keep their own next-deadline source
   (`chord::next_deadline(&state.chord_machine)` vs `state.stage.next_deadline()`
   — different internals, same `Option<Instant>`), only the wait wrapper
   collapses.

## The friction

Lifecycle teardown is a **4-participant × 4-situation matrix** spread across
four structurally different pieces of code, with no single place a reviewer can
read it, and holes that nothing marks as decisions rather than bugs.

| situation | code today | axis | analog_repeat | stage | individual firings | individual toggles | chord |
|---|---|---|---|---|---|---|---|
| Layer switch | `handle_layer_switch` (`dispatch.rs:1030`), direct calls | `reset()` | `stop_all` | `stop_all` | `drain_firings` | — survive | — |
| Profile switch | `edit::plan` `SwitchProfile` → 5 `Effect`s → `run_effects` (`dispatch.rs:629`) | `reset()` | `stop_all` | `stop_all` | `drain_firings` | `stop_all_toggles` | — survive |
| Disconnect | `handle_connection_change` (`dispatch.rs:1085`), direct calls | **—** | **—** | `stop_all` | `drain_firings` | — | — |
| → Digital | `handle_capture_mode_change` (`dispatch.rs:1122`), direct calls | **—** | `stop_all` | `stop_all` | `drain_firings` | — | — |

- **Four method names for one idea.** `reset` / `stop_all` / `drain_firings` /
  `stop_all_toggles` / `stop_stage`, each a different signature, each
  hand-threaded as a `&mut self.<field>` argument into a bespoke
  `#[allow(clippy::too_many_arguments)]` leaf function.
- **Two mechanisms.** Profile switch runs teardown as data (`Effect` variants
  through `run_effects`) because a Profile switch mutates `Config` and the
  `run` loop is the sole commit point; the other three are direct calls
  because they touch only momentary `DispatchState`. Both are legitimate — but
  there is no shared fan-out they both feed.
- **The `—` cells are invisible.** `axis` / `analog_repeat` not torn down on
  disconnect, `axis` not reset on the Digital flip — deliberate or missing?
  Nothing in the code says. The dual-stage `05-interaction-sweep.md` and the
  09-06 review both flagged the same holes.
- **`wait_for_stage_deadline` is a byte copy of `wait_for_chord_deadline`**
  (`dispatch.rs:994` / `1007`) — its own doc says "mirrors
  `wait_for_chord_deadline`'s exact shape one line above."
- **`CONTRIBUTING.md` is already stale** — its `select!`-handler list omits
  `update_stages` / `tick_stages`, and there is no "Changing dual-stage
  behaviour" bullet though `axis` / `analog_repeat` / `chord` / `stepper` /
  `trigger` all have one.

**Deletion test.** Delete `tear_down` and the matrix scatters back to four
functions and one `Effect` fan-out, and the holes go invisible again.
Concentrating it: a reviewer reads the whole matrix in one `match`; each `—`
cell has a written rationale next to it; a fifth engine (or a new
`TeardownReason`) is one visible edit per arm; and the Quick-Skip / Toggle
survival rules stop living only in an `edit.rs` doc comment. This *concentrates*
— there was no single home before, so nothing is merely moved.

## Why not a `DepthEngine` trait (the recurring suggestion)

Three consecutive architecture reviews raised "give the Depth-fed engines a
shared interface" and escalated it — 2026-09-03 Candidate 6 (**Speculative**,
axis-emit dedup only), 2026-09-03b Candidate 4 (**Worth exploring**, a
`reset()` fan-out method *or* "a small `DepthEngine` trait"), 2026-09-06
Candidate 3 (**Worth exploring**, a full
`trait DepthEngine { wants; observe; next_deadline; on_teardown }` behind
`Vec<dyn DepthEngine>`, with the fallback "even adopting only the
`on_teardown(reason)` half"). No review rated it Strong; none was rejected,
only deprioritised behind ticket 17 (now done).

The grilling settled on the **minimal cut** (`tear_down(reason)` match, no
trait). Reasons, load-bearing enough to record as **ADR-0010**:

- **The four "engines" are radically heterogeneous.** `axis::Engine` is
  sync / infallible / holds no `&Injector` / returns `Vec<AxisWrite>` the caller
  emits; `analog_repeat::Engine` is async and owns spawned tokio tasks with
  their own `CancellationToken` timers; `stage::Engine` is async / fallible
  (`io::Result`) with a 7-field `EngineDeps` and reaches into
  `DispatchState::individual` + `stepper`; `chord`'s `ChordMachine` holds **no
  handles at all** and is a pure free-fn family whose teardown target is a
  *sibling* field (`chord_slots`), and it is not fed from `rx_depth`. A uniform
  `observe(snapshot) -> Ops` / `wants(cfg, layer)` interface fits none of them
  without a lowest-common-denominator that hides more than it reveals.
- **`Vec<Box<dyn DepthEngine>>` with async methods fights edition 2024 and this
  codebase.** Async-fn-in-trait is not `dyn`-compatible without `async-trait`
  (only a transitive dep via `zbus` today) or hand-boxed `Pin<Box<dyn Future>>`;
  the daemon has **zero** `dyn` trait objects — its established pattern is a
  pure sync core plus one async shell (`trigger::Slots::perform`,
  `analog_repeat::Engine`, `stage::Engine`).
- **The `rx_depth` arm is the sub-millisecond hot path** (drives continuous
  analog-stick `ABS_*` output). A `dyn`-dispatched `observe` there is a real
  cost; a `dyn`-dispatched `on_teardown` would be fine (human-scale events) but
  is not worth the boilerplate over a `match`.
- **Precedent.** Ticket 10 already declined a `depth::` umbrella module: the
  engines "share only the `rx_depth` snapshot value … CONTEXT.md has no such
  concept." Teardown is even less of a domain concept.

ADR-0010 refines nothing in ADR-0007 (that decision is about *where* staged
Depth is interpreted, not lifecycle) — cite it as adjacent.

## Integration in `dispatch.rs`

- **`handle_layer_switch`** (`dispatch.rs:1030`): body becomes
  `*active_layer = new_layer;` → `state.tear_down(TeardownReason::LayerSwitch)`
  → the existing `Daemon::active_layer_changed` signal emit. The four inline
  engine calls move into `tear_down`. Signature loses its per-engine `&mut`
  args, gains `&mut DispatchState` (or stays a method — see below).
- **`handle_connection_change`** (`dispatch.rs:1085`): the
  `if !connected { … }` block becomes
  `state.tear_down(TeardownReason::Disconnect)`; the signal emit and the
  `push_status_leds` on connect stay.
- **`handle_capture_mode_change`** (`dispatch.rs:1122`): the
  `if mode == CaptureMode::Digital { … }` block becomes
  `state.tear_down(TeardownReason::CaptureModeToDigital)`.
- **Profile switch — collapse the 5 teardown `Effect`s into one.**
  `edit::plan`'s `SwitchProfile` arm currently pushes
  `StopAllToggles, ReleaseAllHolds, RepublishActuation, ResetAxisOutputs,
  StopAllAnalogRepeats, StopAllStages, AssertStatusLeds, AnnounceProfileChange`.
  Replace the five teardown variants with one `Effect::TearDown(TeardownReason
  ::ProfileSwitch)`, leaving `RepublishActuation` / `AssertStatusLeds` /
  `AnnounceProfileChange` as-is. `run_effects` gains one arm
  (`Effect::TearDown(r) => self.tear_down(r).await`) and loses five. The
  toggles→holds→axis→analog→stage order moves into `tear_down`'s
  `ProfileSwitch` arm.
  - **`Effect::StopStage(Input)` stays** — it is a targeted per-Input release
    from the primary cascade (and, after ticket 18, `ClearDeepStage`), not a
    matrix sweep.
  - `edit.rs` and `dbus` unit tests that assert
    `outcome.effects == vec![Effect::StopAllStages, …]` update to
    `vec![Effect::TearDown(TeardownReason::ProfileSwitch), …]`.

### Static dispatch — how the fields stay concrete

`tear_down` is a `&mut self` method on `DispatchState` that names each field
directly (`self.axis`, `self.stage`, …). No `Vec`, no enum wrapper, no trait
object. The leaf functions that currently take `&mut axis::Engine` etc. either
become `DispatchState` methods too, or keep taking `&mut DispatchState` (the
CONTRIBUTING.md "narrow borrow is fine" carve-out still applies to anything
that is genuinely one-or-two-field). Match the surrounding style during
implementation.

## Behaviour-preservation protocol

Latency-critical path; every Layer/Profile switch and capture flip runs
through it.

- **Diff the four `tear_down` arms line-by-line against the four call sites at
  `HEAD`.** Load-bearing: `LayerSwitch` drains firings but **not** toggles;
  `ProfileSwitch` drains **both**; `Disconnect` and `CaptureModeToDigital`
  touch neither `axis` nor (disconnect) `analog_repeat`; the axis `reset()`
  return is emitted through `injector.set_axis_value` in every arm that calls
  it.
- **Verify `RepublishActuation` ordering is behaviour-neutral.** It currently
  sits *between* `ReleaseAllHolds` and `ResetAxisOutputs` in the
  `SwitchProfile` effect vec; after the collapse it lands *after* the whole
  `TearDown`. `publish_actuation_snapshot` only re-pushes the actuation
  watch-channel snapshot to the capture grid task — independent of axis
  centering and hold draining — so the move is expected to be inert. Confirm
  with the `switch_profile_publishes_the_new_profiles_own_actuation_points`
  test and the axis-centering-on-switch test both green unchanged.
- **`cargo test -p acheron-daemon` fully green** — the ~40 `dual_stage_*`
  tests, the layer-switch / profile-switch / disconnect / capture-flip tests —
  **before** any new test is added.
- **`/code-review`** on Standards + Spec axes, as tickets 05–17 did.

## Tests: keep the net, add the matrix

- **New — `DispatchState::tear_down` unit tests** (`dispatch.rs` test module,
  built on the existing `Seam::new` path — a real recording-sink `Injector`,
  stub channels, no `run()`): for each `TeardownReason`, seed live state in
  every participant (an individual firing + an individual Toggle, a spawned
  analog-repeat task, a live deep stage, a live axis output, a chord firing +
  a chord Toggle), call `state.tear_down(reason)`, and assert **exactly** which
  are released and which survive — the matrix above as executable assertions.
  This becomes the matrix's spec.
- **Kept, unchanged:** the `dual_stage_*` pipeline tests, the layer/profile/
  disconnect/capture integration tests (they still earn their place proving
  the *wiring* — that `handle_layer_switch` actually calls `tear_down`, that
  the `select!` arm reaches `handle_capture_mode_change`), and `stage.rs` /
  `chord.rs` / `axis.rs` pure tests.
- **Not deleted:** end-to-end teardown coverage. The report's "one test
  instead of N" was aspirational for the full trait; this cut adds a focused
  test and keeps the net.
- One deadline-helper test (`wait_for_deadline(None)` never resolves;
  `Some(past)` resolves promptly) if one doesn't already exist.

## Docs

- **New ADR-0010** — `docs/adr/0010-runtime-engine-teardown-one-match-not-a-trait.md`.
  Content: lifecycle teardown for the runtime engines is one
  `DispatchState::tear_down(reason)` `match`, not a `DepthEngine` trait; the
  four "why not the trait" reasons above (heterogeneity, `dyn`/`async-trait`
  vs edition 2024 and the codebase idiom, the hot `rx_depth` path, the ticket
  10 umbrella precedent); the two teardown *entry points* (direct call for
  momentary state, `Effect::TearDown` for the Config-commit path) both feed the
  one `match`; adjacent to ADR-0007, refines nothing. Records the decision so
  a sixth architecture review does not re-propose the trait.
- **`CONTRIBUTING.md`** — (a) refresh the stale `select!`-handler list
  (`update_stages`, `tick_stages`); (b) add a "Changing dual-stage /
  Staging-mode behaviour" bullet pointing at `stage.rs` (`advance` / `StageOp`
  / the transition tables) + `stage::Engine`, matching the other per-engine
  bullets; (c) add a "Changing lifecycle teardown" bullet:
  *the matrix lives in `DispatchState::tear_down` + `TeardownReason`; a new
  engine adds a line to each arm; behaviour changes to a `—` cell are ticket
  20's, not a drive-by.*
- **`DispatchState` doc comment** (`dispatch.rs:52`) — one sentence naming
  `tear_down` as the single lifecycle-teardown site.
- **ADR-0007** — no change (lifecycle was always out of its scope; its ticket
  17 Refined paragraph already says the teardown contract is "a future
  ticket").
- **No `CONTEXT.md`** — `tear_down` / `TeardownReason` are dispatch-internal
  plumbing; no domain noun (ticket 10 / 14 / 15 / 17 precedent of declining an
  entry for a locus refactor).
- **`.scratch/README.md`** — extend the `post-release-development` line with
  ticket 19.

## Decisions from the grilling (2026-09-09)

- **Q2 = Cut A.** `tear_down(reason)` match + deadline-helper merge, no trait.
  Not Cut A-minus (dedup only — leaves the matrix scattered), not Cut B
  (`on_teardown` trait — idiom-breaking boilerplate for marginal gain), not
  Cut C (full `DepthEngine` trait — `dyn` on the hot path, `chord` doesn't fit,
  `stage`'s deps don't fit a uniform `observe`).
- **Q4 = preserve behaviour, document the skips.** The refactor makes each
  matrix cell explicit (a call or a `//`-marked skip); it changes no
  behaviour. Turning a skip into a real call is ticket 20, one at a time, with
  its own reasoning (and possibly a spec.md change — disconnect handling is
  currently declared out of scope).
- **Q5 = collapse the `Effect`s.** The five profile-switch teardown variants
  become one `Effect::TearDown(reason)`; `StopStage(Input)` stays separate;
  `RepublishActuation` / `AssertStatusLeds` / `AnnounceProfileChange` stay.
  Verify the `RepublishActuation` reorder is inert.
- **Q8 = fold the deadline-helper merge into this ticket** — it is the "unify
  the `select!` deadline arm" half of the same candidate.
- **Q11 = `enum TeardownReason { LayerSwitch, ProfileSwitch, Disconnect,
  CaptureModeToDigital }`**, method `DispatchState::tear_down`. No CONTEXT.md
  term. (`DigitalFallback` is the fallback name if `CaptureModeToDigital`
  reads oddly.)
- **Q3 = `chord_machine` / `chord_slots` are named participants in every arm,
  no chord behaviour change.** "Should a Layer switch reset the chord window /
  drain chord firings? Should disconnect touch chord?" → ticket 20.
- **Q12 = focused `tear_down` test as the matrix's executable spec; keep the
  integration tests.** No deletion of end-to-end coverage.
- **Q10 = write ADR-0010.** The trait idea has recurred three times and will
  recur; a future explorer needs the "why not the trait" reasoning.
- **Q14 = ticket 18 (bug batch) lands first**; this ticket rebases on it. No
  real conflict (`Effect::StopStage` is untouched by the collapse), but each
  diff stays single-purpose.
- **Q13 = filed as `post-release-development` ticket 19** — that effort's
  purpose is architecture-review-driven deepening (tickets 03–17); this is the
  fifth review's top candidate.

## Facts dug from the code during the grilling (not asked of the user)

- `handle_layer_switch` (`dispatch.rs:1030`, body 1040–1069): `axis.reset()`
  → `set_axis_value` loop, `analog_repeat.stop_all()`, `stage.stop_all(injector)`,
  `individual.drain_firings(injector)`, then `Daemon::active_layer_changed`.
- `handle_connection_change` (`dispatch.rs:1085`, body 1093–1108): on
  `!connected` — `stage.stop_all`, `individual.drain_firings`; then
  `device_connection_changed`. Signature takes no `&mut axis::Engine` /
  `&mut analog_repeat::Engine`. Doc: "no generic one exists anywhere else in
  the daemon today (Analog-repeat has no dropout handling at all — a
  pre-existing gap … spec.md's 'Out of Scope')".
- `handle_capture_mode_change` (`dispatch.rs:1122`, body 1131–1153): on
  `mode == Digital` — `analog_repeat.stop_all`, `stage.stop_all`,
  `individual.drain_firings`; then `capture_mode_changed`. No `&mut
  axis::Engine`.
- `Edit::SwitchProfile` (`edit.rs:468–512`) pushes, in order:
  `StopAllToggles, ReleaseAllHolds, RepublishActuation, ResetAxisOutputs,
  StopAllAnalogRepeats, StopAllStages, AssertStatusLeds, AnnounceProfileChange`.
- `run_effects` (`dispatch.rs:629`, arms ~662–675) maps each 1:1:
  `StopAllToggles→individual.stop_all_toggles`, `ReleaseAllHolds→individual
  .drain_firings`, `StopAllAnalogRepeats→analog_repeat.stop_all`,
  `StopAllStages→stage.stop_all`, `StopStage(i)→stage.stop_stage(i,…)`,
  `ResetAxisOutputs→axis.reset()` loop.
- `edit.rs:479–486`: "`StopAllToggles` drains only the individual `Slots<Input>`
  … never the `ChordKey`-keyed `chord_slots` — an active Chord Toggle survives
  a Profile switch today."
- `wait_for_chord_deadline` (`dispatch.rs:994–999`) and `wait_for_stage_deadline`
  (`dispatch.rs:1007–1012`) — token-for-token identical bodies
  (`match deadline { Some(d) => sleep_until(d).await, None => pending().await }`).
- `chord::next_deadline(&ChordMachine)` (`chord.rs:293`) — single global
  window. `stage::Engine::next_deadline(&self)` (`stage.rs:1072`) — `.min()`
  across a per-key `HashMap`. Both `-> Option<Instant>`.
- `axis::Engine` has no timer / `Instant` / deadline method
  (`axis.rs:15` doc: "contains no `async fn`"). `analog_repeat::Engine` relies
  entirely on its per-Input spawned tasks (`tokio_util::sync::CancellationToken`,
  `analog_repeat.rs:23`) — nothing surfaced to the `select!`.
- `chord_machine` is touched only at: field decl, `DispatchState::new`,
  `handle_event` (`chord::feed`), the deadline arm (`chord::next_deadline` +
  `chord::tick`). **Reset by no lifecycle event.** `chord_slots` swept by no
  lifecycle event.
- `DispatchState::new` (`dispatch.rs:113`) is called directly from `Seam::new`
  (`dispatch.rs:1344+`) with stub channels + a spawned recording-sink
  `Injector` — a `tear_down` unit test needs no `run()`.
- `daemon/Cargo.toml`: `edition = "2024"`. `async-trait` is **not** a direct
  dep — transitive only, via `zbus` (`Cargo.lock`). No `rust-toolchain.toml`.
  Grep for `dyn ` trait objects in `daemon/src`: none of the engine kind.
- `rx_depth.changed()` arm (`dispatch.rs:888–904`): one snapshot →
  `handle_depth_update` (axis) + `update_analog_repeats` + `update_stages`.
  Published by `capture::analog::relay_grid_blocking` once per HID report
  `0x06` — "sub-millisecond while moving" (`analog.rs` comment); coalescing
  `watch`. `handle_depth_update` doc: "runs on every live-Depth tick
  (sub-millisecond while a key is moving)".
- `CONTRIBUTING.md:221–237` "Adding a new piece of dispatch runtime state" —
  its handler list omits `update_stages` / `tick_stages`. No "Changing
  dual-stage behaviour" bullet exists (`axis`/`analog_repeat`/`chord`/`stepper`/
  `trigger` each have one at `CONTRIBUTING.md:166–206`).
- `DispatchState` doc: `dispatch.rs:52–60`.

**Blocked by:** ticket 18 (bug batch — lands first; this rebases on it).
Ticket 17 (`stage::Engine::feed`, the "one event entry point" precondition) is
**done**.

**Status:** done — filed 2026-09-09, implemented 2026-09-10 on `dev`.

- [x] `TeardownReason` enum + `DispatchState::tear_down(reason)` — the four
      arms transcribed from `HEAD`, every participant named, every skip
      `//`-marked.
- [x] `handle_layer_switch` / `handle_connection_change` /
      `handle_capture_mode_change` reduced to state-mutation + `tear_down(…)` +
      signal emit; now `&mut self` methods on `DispatchState`, per-engine args
      dropped, `#[allow(clippy::too_many_arguments)]` gone.
- [x] `Effect::TearDown(TeardownReason)` replaces the five profile-switch
      teardown effects (`StopAllToggles` / `ReleaseAllHolds` /
      `ResetAxisOutputs` / `StopAllAnalogRepeats` / `StopAllStages` all
      removed) in `edit::plan`'s `SwitchProfile` arm; `run_effects` gains one
      arm, loses five; `edit.rs` effect-assertion test updated (no `dbus`
      test asserted these). `Effect::StopStage(Input)` unchanged.
- [x] `RepublishActuation` reorder verified behaviour-neutral —
      `switch_profile_publishes_the_new_profiles_own_actuation_points` and
      `a_layer_switch_centers_any_live_axis_output` green unchanged.
- [x] `wait_for_chord_deadline` + `wait_for_stage_deadline` merged into
      `wait_for_deadline(Option<Instant>)`; both `select!` arms call it. New
      `wait_for_deadline` unit test (`None` never resolves; `Some(past)`
      resolves promptly).
- [x] `DispatchState::tear_down` unit tests — `dispatch::tests::tear_down_*`,
      one per `TeardownReason`, seeded live state in every participant
      (individual firing + Toggle, spawned Analog-repeat task, live deep
      stage, live axis output, Chord Toggle) via the `Seam` seam.
- [x] All `dual_stage_*` + layer/profile/disconnect/capture integration tests
      green unchanged.
- [x] `docs/adr/0010-runtime-engine-teardown-one-match-not-a-trait.md` written.
- [x] `CONTRIBUTING.md` — handler list refreshed (`update_stages` /
      `tick_stages` + the three now-method handlers), dual-stage bullet added,
      lifecycle-teardown bullet added. `DispatchState` doc comment extended.
- [x] `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, full
      daemon suite green (553, was 548); GUI suite unaffected.
- [x] `/code-review` on Standards + Spec axes — Spec: faithful, no
      behaviour change, all four arms match the `HEAD` call sites; Standards:
      flagged the `edit → dispatch` import cycle and the co-located
      axis-reset duplication. Both addressed (see Comments).
- [x] `.scratch/README.md` `post-release-development` line extended.

## Comments

**2026-09-10** — Implemented on `dev`. `DispatchState::tear_down(reason)` is
the one lifecycle-teardown match; `handle_layer_switch` /
`handle_connection_change` / `handle_capture_mode_change` are now `&mut self`
methods that mutate state, call `tear_down`, and emit their signal (the
`#[allow(clippy::too_many_arguments)]` and the disjoint-`&mut` threading are
gone). `Effect::TearDown(TeardownReason)` replaces the five profile-switch
teardown effects; `RepublishActuation` now trails the teardown (verified inert
— `switch_profile_publishes_the_new_profiles_own_actuation_points` +
`a_layer_switch_centers_any_live_axis_output` green unchanged). The two
byte-identical deadline helpers merged into `wait_for_deadline`. New
`dispatch::tests::tear_down_*` (one per reason, every participant seeded live
on the `Seam` seam: an individual firing + Toggle, a spawned Analog-repeat
task, a live deep stage, a live axis output, a Chord Toggle + a Chord firing)
and a `wait_for_deadline` test. `cargo fmt` / `clippy -D warnings` clean;
daemon suite 553 green (was 548); GUI untouched. ADR-0010 written;
CONTRIBUTING.md — handler list refreshed, dual-stage and lifecycle-teardown
bullets added; `DispatchState` doc comment extended.

`/code-review` (Standards + Spec): **Spec** — faithful, no behaviour change,
all four arms match the `HEAD` call sites line-for-line. **Standards** — two
findings, both applied: (1) `TeardownReason` was defined in `dispatch.rs` and
imported by `edit.rs`, making `edit` (a deliberately pure leaf module) depend
on `dispatch` — moved the enum into `edit.rs` next to `Effect` /
`CommandError`, `dispatch` imports it; (2) the axis-reset emit loop was
verbatim in the `LayerSwitch` and `ProfileSwitch` arms — extracted
`DispatchState::reset_axis_outputs`. Also fixed a stale doc table in the test
helper and noted per-arm operation order is the former call site's.

**2026-09-09** — Filed from the fifth architecture review
(`research/architecture-review-2026-09-09.html`, candidate 1 of 6, the "Tackle
first" pick). The review over-stated the candidate as "Strong" and bundled
three things (teardown, deadline dedup, the `ClearDeepStage` bug); the grilling
(5 rounds) corrected the strength, split the bug fix to ticket 18, ruled out
the `DepthEngine` trait for load-bearing reasons (→ ADR-0010), and scoped the
matrix-hole *behaviour* questions to ticket 20. What lands here: one readable
`tear_down(reason)` match that is the whole lifecycle-teardown matrix, the
deadline-helper merge, ADR-0010, and the CONTRIBUTING.md catch-up. Behaviour
identical. Not yet implemented — handed to a fresh session.
