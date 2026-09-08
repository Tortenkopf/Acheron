Status: ready-for-agent

# Acheron — Dual-stage grid keys

## Problem Statement

Every grid key's Binding fires off a single Actuation point: one Depth crossing, one Down, one
Up. That's the right model for a discrete keypress, but it leaves travel past the Actuation
point doing nothing — Acheron already spends that travel on Analog-repeat's variable-rate
cadence, but a user who wants two genuinely **different** actions at two different press depths
(a light tap for one thing, a full press for another — the way a camera shutter half-presses to
focus and fully presses to shoot) has no way to express it. A grid key's Binding is
all-or-nothing per press.

"Dual-stage keys" lets a grid key in analog Capture mode carry a second **Actuation stage** — a
Binding with its own Actuation/Release point pair — that fires at a Depth strictly deeper than
the key's ordinary (now called **primary**) stage, with a user-selectable **Staging mode**
governing how the two stages hand off as Depth ramps through both bands.

This spec is the hand-off to a **separate implementation effort**. Every decision below is
settled by the map (`.scratch/tartarus-dual-stage-keys/map.md`) and its resolved tickets;
nothing here is left open. There is no kill-gate — unlike the Status-LED effort, this is
entirely software running on an already-verified Depth stream, so no hardware feasibility
question exists to gate on.

## Solution

A grid key's **primary stage** is its Binding exactly as it works today — unchanged in every
respect. Optionally, that same key may also carry a **deep stage**: a second, independent
Binding with its own Actuation/Release point pair, whose band sits strictly above the primary's
(`deep.release > primary.actuation` — the two hysteresis bands are disjoint and stacked, never
overlapping). A deep stage cannot exist without a primary stage on the same Input/Layer; deleting
the primary cascade-deletes the deep stage. A grid key is capped at two stages — key travel makes
a third impractical.

The user picks one of four **Staging modes**, per Input per Profile, governing the handoff:

- **Handoff** — crossing into the deep band releases the primary and presses the deep stage;
  crossing back out releases the deep and re-presses the primary. Exactly one stage held at a
  time, symmetric (the camera-shutter model). The default.
- **No-Return** — identical to Handoff going deeper, but the primary does not re-press on the
  way back out; the key stays quiet until fully released and pressed again.
- **Additive** — both stages fire and are held simultaneously; the deep band adds the deep
  stage without releasing the primary.
- **Quick-Skip** — if the deep band is reached within ~50ms of the primary crossing, the
  primary's Down is suppressed entirely (never fires, and its eventual Up doesn't either);
  otherwise the primary fires (delayed by up to that ~50ms) and the key runs as Handoff for the
  rest of the press. Trades up to 50ms of primary-Down latency for letting a fast full press
  skip the primary action outright.

Each stage's Binding has its own independent Trigger mode (Fire-once / Hold-to-repeat / Toggle),
carries any Action a Binding can carry today (including firing a Profile switch from the deep
stage), and is validated by the same per-Action rules as any other Binding. The one new
restriction: **neither stage of a dual-stage key may use Analog-repeat** — the Analog-repeat
background task ignores Actuation points entirely and would fight the staging logic — and **a
Chord member cannot carry a deep stage** — the two depth interpretations tangle. Both are
enforced at config-validation time, not left as runtime footguns.

In Digital Capture mode (no Depth available) the deep stage is silently inert — only the primary
fires, the same degradation Analog-repeat already has to plain Hold-to-repeat. The GUI greys the
deep-stage controls with a "requires analog" note rather than hiding them.

No new daemon→GUI signal or `GetState()` field is added — the existing live Depth bar, extended
to 4 markers (primary release/actuation, deep release/actuation), already shows which band is
hot; `config.toml` remains the single source of truth for what's configured.

## User Stories

1. As a user, I want a grid key to fire a different Action when I press it further, so that one
   physical key can serve as both a light-tap control and a full-press control (a game's
   aim/zoom-then-fire pattern, or a driving sim's half-throttle/full-throttle).
2. As a user, I want to choose whether the deeper action *replaces* the lighter one while held,
   *adds* to it, or *permanently supersedes* it for that press, so that I can match the staging
   behavior to what the game or workflow actually needs.
3. As a user pressing quickly all the way through, I want the option to skip the light-tap
   action entirely rather than firing it for a fraction of a second on the way past, so that a
   fast full press doesn't leave a stray light-tap keystroke behind.
4. As a user, I want the deep stage's Actuation/Release points and Staging mode to be part of
   the key's physical-travel configuration (like the primary's own Actuation point), not
   something I re-tune per Layer, so that switching between Base and Held doesn't require
   re-setting how deep the second stage engages.
5. As a user, I want the deep stage's Binding itself to be able to differ between Base and Held
   Layers, exactly like the primary Binding already can, so that the Mode key still changes
   *what* fires without changing *how deep* it fires.
6. As a user in Digital Capture mode, I want a dual-stage key to just behave like a normal key
   (its primary Binding only), so that losing analog capture degrades gracefully instead of
   losing the primary action too.
7. As a user, I want a clear warning or simply a disabled control when I try to add a deep stage
   before binding a primary, so that a deep stage is never left dangling with nothing to hand
   off from.
8. As a user editing the binding editor, I want to configure a deep stage without ever having
   two large key/button pickers open on screen at once, so that the editor stays navigable at
   the same footprint as today's single-stage editor.
9. As a user, I want deleting the primary Binding to also remove any deep stage sitting on top
   of it, so that I can never end up with a deep stage that has nothing under it.
10. As a user upgrading from a build without this feature, I want my existing `config.toml` to
    keep working unchanged, with no key having a deep stage until I add one.

## Implementation Decisions

### Pipeline architecture

Settled by [ticket 01](./issues/01-staged-event-pipeline.md), recorded in
**[ADR-0007](../../docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md)**; this
section states the shape, not the rationale.

**The analog capture source is unchanged.** `capture/analog.rs`'s pure `observe(prev, depth,
point)` keeps its existing 6-line body and table tests; `PhysicalEvent` gains no `stage` field.
All staged-depth interpretation lives in dispatch instead — matching the existing pattern where
`Config`-aware Depth interpretation (Axis conflict resolution, the Analog-repeat rate curve)
already runs there, off the live-Depth `watch` channel (`rx_depth`, which is the **raw,
un-throttled** stream — the ~30Hz `DepthChanged` throttle is D-Bus-only, so the engine sees
every hidraw report and the 50ms Quick-Skip window has ample samples inside it).

**Module seam:** a new pure `daemon/src/stage.rs` plus a non-pure `stage::Engine` field on
`DispatchState`, the third depth engine alongside `axis::Engine` and `analog_repeat::Engine`.

- **Pure core** (`stage.rs`, importing nothing from `dispatch`/`edit`/`chord`/`config::Config` —
  the same discipline `chord`/`axis`/`analog_repeat` already hold to): a state-machine `advance`
  function producing a data-only `StageOp` per transition (`FirePrimary` / `ReleasePrimary` /
  `RepressPrimary` / `FireDeep` / `ReleaseDeep` / `SuppressPrimary` / `Nothing` — see "Staging-mode
  state machine" below for the exact per-mode tables), plus a Quick-Skip timeout pair
  (`next_deadline` / `tick`) mirroring `chord::next_deadline` / `chord::tick`.
- **Non-pure `stage::Engine`** on `DispatchState`, owning: per-key deep-band `KeyState` (fed to
  the same `observe()` capture uses for the primary band) and a shadow primary-band `KeyState`
  (so the engine tracks the primary band itself off the depth stream, never needing the primary
  `PhysicalEvent`); per-key Quick-Skip runtime state (pending? deadline? resolved to skip or
  late?); and the deep stage's own **`trigger::Slots<StageKey>`** — the `(firings, toggles)`
  handle pair (post-release ticket 15's generic `Slots<K>`, the same type the individual path
  uses as `Slots<Input>` and the Chord path uses as `Slots<ChordKey>`), because Additive can hold
  both stages live at once and each stage is a fully independent Binding.

  `StageKey(Input)` is a newtype distinguishing the deep slot's keyspace from the primary's bare
  `Input` keyspace — the same shape the Chord executor already has (`ChordKey` vs `Input`).

- `Engine::update(&mut self, config, active_layer, &snapshot) -> Vec<(target, StageOp)>` runs
  from the existing `rx_depth.changed()` `select!` arm, third after `handle_depth_update` and
  `update_analog_repeats`. Deep `ActuationPoint` comes from `profile.deep_stages.get(&input).map(|c|
  c.actuation)` (config schema below).

**Deep-stage firing:** ops are performed directly against `state.stage: Slots<StageKey>` via
`trigger::decide(&deep_binding, ..) ` + `Slots::perform(..)` — no synthetic `PhysicalEvent` round
trip. `ReleasePrimary`/`RepressPrimary` ops reach into the **primary** `Input` keyspace instead
(`Slots::force_release` / `stop_toggle` to release, `dispatch_individual_down` to re-press) — the
`stage` shell touches both keyspaces, precedented by the Chord executor's own
`FireIndividual`/`ForceReleaseIndividual` effects.

**Quick-Skip's primary suppression** is a dispatch-side buffer modeled on the Chord-detection
machine, since capture has already emitted the primary edge by the time the deep band becomes
reachable:

- Only Quick-Skip dual-stage keys divert. `handle_event` gets one narrow insertion at the
  binding-lookup point where the `AnalogRepeat`-swallow already sits, gated on `event.depth.is_some()`
  exactly like that swallow — so a Digital-mode primary press never diverts (the deep stage is
  inert for free in Digital mode).
- `Down` → `stage.begin_quick_skip(input, depth)`, then swallow the event. This resolves
  immediately to *skip* if the deep band is already hot (a same-report or already-arrived deep
  crossing — see "Staging-mode state machine" for why this can't produce an ordering hazard);
  otherwise it arms the ~50ms deadline.
- `Up` while the buffered primary never fired → the event is swallowed for the *primary
  keyspace* (nothing there to force-release — the buffered Down was never given to
  `individual`), but the `Armed` deadline is **disarmed on this `rx_events` edge itself**, in
  `stage::Engine::feed` (`end_quick_skip`), driving the pure core with `next == (Up, Up)` the
  same way `begin_quick_skip` owns the outer Down. It is *not* left to a later coalescing
  `rx_depth` cancel tick: on a quick shallow tap the release edge can coalesce away in the
  `watch` channel before `update` ever observes the excursion, so the deadline would elapse and
  fire `RepressPrimary` into a press whose only real `Up` was already consumed — a permanently
  stuck primary (`tartarus-dual-stage-keys-impl` ticket 13). The `update` cancel row is kept as
  a harmless idempotent double-confirm. A `Skipped` key released in one report straight from
  the deep band takes the same edge-driven path — `ReleaseDeep`, No-Return's release shape, no
  `RepressPrimary`.
- The deadline elapsing with no deep crossing fires the primary retroactively via
  `dispatch_individual_down` (a new fourth `select!` arm, `wait_for_stage_deadline`, mirroring
  `wait_for_chord_deadline`) and flips the key to run as ordinary Handoff for the rest of the
  press.
- Handoff / No-Return keys are **entirely unaffected** — `handle_event` runs
  unchanged for them; the engine only ever watches the deep excursion independently.

**Teardown:** `stage::Engine::stop_all()` is wired to the same call sites as
`analog_repeat.stop_all()` (`handle_layer_switch`, `handle_capture_mode_change`'s Digital
transition, the `SwitchProfile` effect list) plus two additions this effort needs that have no
existing generic hook to reuse — see "Runtime behavior across Layer/Profile/connection changes"
below.

### Staging-mode state machine

Settled by [ticket 02](./issues/02-dual-stage-state-machine.md). Combined state = (primary band,
deep band) ∈ {Up,Down}²; `(Up,Down)` is structurally impossible — the disjoint-stacked-band
constraint (`deep.release > primary.actuation > primary.release`) means Depth can never be low
enough to cross `primary.release` while still `≥ deep.release`, so **deep always releases before
primary re-engages by construction**, not by a state-machine rule. For the same reason a
`RepressPrimary` op never needs to re-check the primary's own hysteresis — it always fires at a
Depth `≥ primary.actuation` — it fires unconditionally.

A same-report double-crossing (a fast ramp jumping both bands in one hidraw report) is handled by
**mechanical replay**, never short-circuited: the full logical op sequence for the mode always
walks, exactly as if the two crossings had arrived in separate reports — matching how
Axis/Analog-repeat/Chord already don't special-case fast ramps. Ordering is resolved
**synchronously per-event**, from the arriving event's own `.depth` field (the same trick
`begin_quick_skip` uses to resolve immediately when the deep band is already hot) — never by
depending on `rx_events`/`rx_depth.changed()` arrival order under `tokio::select!`'s unordered
tie-break. This closes an ordering race ticket 01's design hadn't fully covered.

**Fire ops** are always `decide(binding, Down, slot)` + `perform`; **Release ops** are always
`decide(binding, Up, slot)` + `perform`, against the stage's own `Slots` map — a stage's
synthetic edge is indistinguishable to `decide` from a physical one, so **no new per-trigger-mode
logic exists anywhere**. One consequence: `RepressPrimary` on a Toggle primary always starts a
**fresh** Toggle loop (`decide(Toggle, Down, None)` has no "resume" — it's a fresh press, not the
old one continuing), intentional and unremarkable. A running Macro on either stage can never be
interrupted by a Release op — `FiringHandle::force_release_stuck` is a no-op against a balanced
Macro — a primary Macro always runs to completion when Handoff hands off; this is inherited
Fire-once/Macro precedent, nothing dual-stage-specific.

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

| Transition | Emitted ops, in order |
|---|---|
| (Down,Down)→(Down,Up) | Release Deep only — key goes quiet; the primary's eventual real Up is a no-op (its slot was already released) |
| (Down,Down)→(Up,Up), 1-report skip | Release Deep → Primary Up (real) — no repress inserted |

(every other row is identical to Handoff's table.)

**Additive** — the engine never touches the primary at all:

| Transition | Emitted ops, in order |
|---|---|
| (Up,Up)→(Down,Up) | Primary Down (real) |
| (Down,Up)→(Down,Down) | Fire Deep |
| (Down,Down)→(Down,Up) | Release Deep |
| (Down,Up)→(Up,Up) | Primary Up (real) |
| (Up,Up)→(Down,Down), 1-report skip | Primary Down (real) → Fire Deep |
| (Down,Down)→(Up,Up), 1-report skip | Release Deep → Primary Up (real) |
| no crossing | Nothing |

**Quick-Skip** — a per-press runtime state (Armed → Skipped / Late) layered on top of Handoff's
mechanics:

| From Armed | Result |
|---|---|
| Deep reached within the window (incl. resolved synchronously if already hot on the triggering report) | → **Skipped**: Suppress Primary (buffered Down dropped for good) → Fire Deep |
| Deadline elapses, deep never reached | → **Late**: Repress Primary (retroactive, via `dispatch_individual_down`) → runs as ordinary Handoff for the rest of the press |
| Up arrives first (deadline not elapsed, deep never reached) | → cancelled: buffered Down dropped, nothing emitted |
| Layer/Profile switch or capture-mode flip while Armed | → cancelled via `stage::Engine::stop_all()` |

Once **Skipped**, every subsequent dip into/out of the deep band for the rest of *this* press
behaves like Additive-with-no-primary (Fire Deep / Release Deep each crossing, primary
permanently inert for this press — "release path = No-Return" governs every release from here,
not just the first). Once **Late**, the rest of the press runs plain Handoff.

**Trigger-mode composition** needs no per-combination logic beyond the Fire/Release rule above.
Worked examples: Handoff with primary=Toggle, deep=Fire-once — the primary Toggle loop starts on
the real Down; `Release Primary` stops it (`Slots::stop_toggle`); `Fire Deep` spawns the
Fire-once; `Repress Primary` starts a **fresh** Toggle loop, not a resume. Additive with both
stages Hold-to-repeat — the primary holds/repeats on its own untouched cadence; `Fire Deep` /
`Release Deep` spawn and force-release the deep stage's own Hold-to-repeat independently, never
touching the primary's slot.

No `config::validate` rule is needed beyond what's already listed under "Config schema" below —
nothing in the state-machine semantics itself needs a new config-time check.

### Config schema

Settled by [ticket 03](./issues/03-config-dbus-surface.md). No `schema_version` bump — every new
field is additive with a `#[serde(default)]`/empty-map default, the codebase's unbroken
precedent (status-LEDs, tickets 17/18/51/54): a pre-feature `config.toml` parses with every new
field empty, which *is* "no key has a deep stage."

```rust
/// A grid key's deep-stage physical/behavioral configuration — its own
/// Actuation/Release pair plus which Staging mode governs the handoff with
/// the primary stage. Bundled as one struct (not two parallel maps) because
/// the two fields are only ever meaningful together, mirroring how
/// `ActuationPoint` itself bundles `actuation`+`release`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepStageConfig {
    pub actuation: ActuationPoint,
    #[serde(default)]
    pub mode: StagingMode,
}

/// The four Staging modes governing how a grid key's primary and deep stages
/// hand off. `Default = Handoff`, so `SetDeepActuation`/`SetStagingMode`
/// can `.entry(input).or_default()` a fresh `DeepStageConfig` regardless of
/// which field arrives first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagingMode {
    #[default]
    Handoff,
    NoReturn,
    Additive,
    QuickSkip,
}
```

`Profile` gains three fields, after `axis_base`/`axis_held`:

```rust
/// Deep-stage Bindings active while this Profile's Base Layer is active —
/// `base`'s exact per-Layer-map sibling, one level deeper. An entry here
/// requires a matching entry in `base` for the same Input
/// (`ConfigError::DeepStageWithoutPrimary`) and a matching entry in
/// `deep_stages` (`ConfigError::DeepStageMissingConfig`).
#[serde(default, skip_serializing_if = "HashMap::is_empty")]
pub deep_base: HashMap<Input, Binding>,
/// `deep_base`'s exact mirror for the Held Layer.
#[serde(default, skip_serializing_if = "HashMap::is_empty")]
pub deep_held: HashMap<Input, Binding>,
/// Deep-stage Actuation point + Staging mode, per-Input per-Profile, shared
/// across Base and Held — it interprets physical travel, like
/// `default_actuation`/`actuation_overrides`. Legal with no matching
/// `deep_base`/`deep_held` entry (an unused, inert deep-stage config); illegal
/// the other way around.
#[serde(default, skip_serializing_if = "HashMap::is_empty")]
pub deep_stages: HashMap<Input, DeepStageConfig>,
```

Plus `deep_layer`/`deep_layer_mut` accessors, exact mirrors of `layer`/`layer_mut`.

`profile_all_binding_sites` chains `deep_base`/`deep_held` in as a third `BindingSite::Individual`
source (not a new variant — a deep Binding is legal only on a Grid Input, same shape as any other
individual Binding). This means post-release ticket 14's `check_binding` seam applies the exact
same payload/Trigger-mode/site-shape rules to a deep Binding as to a primary one **for free** —
"each stage is a full Binding, all existing per-Action validation applies to each stage
independently" falls out of this chain without a new rule.

**Seven new `ConfigError` variants**, appended to `validate` after the existing checks:

```rust
/// A deep-stage entry keyed by an `Input` that isn't a `Grid` variant — only
/// grid keys have Depth to threshold a deep stage against.
InvalidDeepStageInput(String),
/// A `deep_stages` entry whose own `release` is not strictly below its own
/// `actuation` — the deep band's internal hysteresis.
DeepStageReleaseNotBelowActuation(String),
/// A `deep_stages` entry whose `release` is not strictly greater than the
/// same Input's resolved primary `actuation` — the disjoint-and-stacked
/// constraint; the two bands overlap.
DeepStageBandOverlapsPrimary(String),
/// A `deep_base`/`deep_held` Binding with no matching Binding in `base`/
/// `held` on the same Layer — no primary, no deep stage.
DeepStageWithoutPrimary(String),
/// A `deep_base`/`deep_held` Binding with no matching entry in
/// `deep_stages` for that Input.
DeepStageMissingConfig(String),
/// A Binding — primary or deep, either Layer — using `analog_repeat` on an
/// Input that has a functioning deep stage.
AnalogRepeatOnDualStageKey(String),
/// An Input that is both a Chord member (on some Layer) and carries a
/// `deep_base`/`deep_held` Binding on that same Layer.
ChordMemberDeepStageConflict(String),
```

`Display` strings and the seven `validate` checks are enumerated in full in
[ticket 03's Answer](./issues/03-config-dbus-surface.md#answer) (§3–4) — mirror them verbatim;
they are locked, not illustrative. Locus is always just the `Input`'s `Display` string, matching
every existing hysteresis/conflict error (`ReleaseNotBelowActuation`, `AxisBindingConflict`, …
never name the Layer either).

Sample `config.toml` fragment:

```toml
[profiles.Default]
default_actuation = { actuation = 128, release = 112 }

[profiles.Default.base.grid_r1c1]
trigger = "hold_to_repeat"
type = "keypress"
key = "KEY_W"

[profiles.Default.deep_base.grid_r1c1]
trigger = "fire_once"
type = "keypress"
key = "KEY_LSHIFT"

[profiles.Default.deep_stages.grid_r1c1]
actuation = { actuation = 220, release = 200 }
mode = "handoff"
```

(`deep.release` 200 > `primary.actuation` 128 — disjoint and stacked; `deep.release` 200 <
`deep.actuation` 220 — the deep pair's own hysteresis.)

**`GetState()` / `active_toggles`: no change.** The 4-marker live Depth bar is the runtime
indicator; `active_toggles` stays `Input`-scoped (primary Toggles only).

### D-Bus `Edit` surface

Settled by [ticket 03](./issues/03-config-dbus-surface.md) §5–7. Four new `daemon/src/edit.rs`
`Edit` variants, granular and mirroring the primary stage's own `SetBinding`/`SetActuationPoint`
split — not one combined call — so the GUI can reuse the existing drag-the-Actuation-bar and
pick-a-Binding interactions independently for the deep slot:

- **`SetDeepStage { input, layer, binding }`** — creates/edits the deep Binding on `layer`.
  Mirrors `SetBinding` one level deeper. Relies entirely on the trailing
  `config::validate(&next)?` (no inline check) for `DeepStageWithoutPrimary`/`DeepStageMissingConfig`
  — sequencing across primary Binding / deep Binding / `deep_stages` config is the caller's
  (GUI's) job, the same way `SetAxisAssignment` leaves "was there already a Binding here" to
  `validate`'s reachable states.
- **`ClearDeepStage { input, layer }`** — removes the deep Binding. `NotFound` if none exists.
  Does **not** cascade-clear `deep_stages` or force-release a live slot — that's the runtime-behavior
  section below.
- **`SetDeepActuation { input, actuation, release }`** — sets the deep Actuation/Release pair,
  `.entry(input).or_default()`-creating a fresh `DeepStageConfig` (mode defaults to Handoff) if
  none exists. **No `Effect`** — unlike `SetActuationPoint`, nothing needs a live snapshot pushed
  to it: `stage::Engine` lives in dispatch and reads `Config` directly each tick.
- **`SetStagingMode { input, mode }`** — sets the Staging mode, same `.or_default()` creation.
  No `Effect`, same reasoning.

All four `plan` arms rely solely on the trailing `config::validate(&next)?` — no inline checks,
no new `Effect`s beyond what's listed above.

**D-Bus methods**, four thin wrappers on `com.acheron.Daemon` shaped exactly like
`set_axis_assignment`/`set_actuation_point` (parse wire args, build the `Edit`, `self.apply(...)`):
`SetDeepStage(input, layer, binding_dict)`, `ClearDeepStage(input, layer)`,
`SetDeepActuation(input, u8, u8)`, `SetStagingMode(input, string)`.

**Wire encoding** (`dbus/wire.rs`): `StagingMode` marshals as a flat lowercase string
(`handoff`/`no_return`/`additive`/`quick_skip`), the exact convention `axis_target_str` already
uses. `DeepStageConfig` marshals as a flat dict bundling its `ActuationPoint` sub-dict plus the
mode string. `profile_to_dict` gains three entries (`deep_base`, `deep_held`, `deep_stages`)
after `status_leds`.

**Parse test:** `a_pre_dual_stage_config_defaults_deep_fields`, mirroring
`a_pre_status_led_config_defaults_status_leds` — a minimal pre-feature `config.toml` must parse
with `deep_base`/`deep_held`/`deep_stages` all empty.

**No new signal.** `SetDeepStage`/`ClearDeepStage`/`SetDeepActuation`/`SetStagingMode` emit
nothing, following `SetBinding`/`SetActuationPoint`'s existing precedent — the GUI rebuilds from
`GetConfig` after its own calls.

**GUI mirror (ADR-0005) — mechanical, no logic:**

- **`daemon_client.py`** — four new methods + `Protocol` stubs, mechanical mirror of
  `set_axis_assignment`/`set_actuation_point`.
- **`daemon_stub.py`** — matching methods following `set_chord_binding`/`set_actuation_point`'s
  guard-clause-before-mutate shape: a Grid-input check, the deep-pair hysteresis check, the
  disjoint-band check against the resolved primary point, `_validate_binding_action` reused as-is
  for `set_deep_stage`, the two dangling checks, and
  `AnalogRepeatOnDualStageKey`/`ChordMemberDeepStageConflict` as `_reject_if_*`-style helpers.
  Seed Profile dict gains `"deep_base": {}`, `"deep_held": {}`, `"deep_stages": {}`.
- **`wire.py`/`read_model.py`** — surface `deep_base`/`deep_held`/`deep_stages` in the config
  dict the GUI reads.
- **`rules.py` — nothing new.** All seven new rules are whole-`Config`/cross-map checks, not pure
  functions of one Binding — they land in `daemon_stub.py` only. (A deep Binding's own
  payload/Trigger-mode legality is already covered for free via the existing
  `_validate_binding_action`, reused unchanged.)

### GUI binding-editor layout

Settled by [ticket 04](./issues/04-dual-stage-binding-editor-layout.md), prototyped at
[`prototype/04-dual-stage-binding-editor-layout/prototype.py`](../../prototype/04-dual-stage-binding-editor-layout/prototype.py)
(three variants built; "Variant A — swap toggle" won, confirmed live against the real
Key/Controller-button pickers). One flat panel, no disclosure/tabs, top to bottom:

1. **One shared 4-marker Actuation bar** — primary green/amber + deep in a second colour pair
   (blue actuation `#3498db` / purple release), fixed-width, sized to match the real key-picker
   row's own natural width so the bar lines up flush with the picker beneath it rather than
   tracking window width live (a `hexpand` width made markers jump mid-drag). Marker order
   left-to-right matches the enforced hysteresis order exactly: primary release, primary
   actuation, deep release, deep actuation. Legend uses real colour swatches, same order as the
   bar. Greys with a "No depth — analog capture unavailable" note in Digital mode (deep markers
   grey too, since they only exist once a deep stage does).
2. **Primary/Deep toggle row.** A `[Primary — <summary>]` toggle, and in the same slot next to
   it: `+ Add deep stage` until one exists, replaced by `[Deep — <summary>]` plus a square red
   `✕` ("Remove deep stage" in its tooltip, not the label) once it does. Both toggles share one
   mutually-exclusive toggle group, so at most one stage is ever selected for editing.
3. **Staging-mode row** (Handoff / No-Return / Additive / Quick-Skip, one-line tooltip each) sits
   below the Primary/Deep row, rendered only once a deep stage exists. Greys with a "Requires
   analog capture" note in Digital mode.
4. **Editor slot** — the real Trigger-mode dropdown, Action-kind dropdown, and the real,
   unmodified `key_picker`/`controller_picker` widgets for whichever stage is currently selected
   in step 2. **Structurally, only one stage's fields — and so only one picker — are ever mounted
   in the tree at once**, which is what actually satisfies the "never two pickers on screen at
   once" hard constraint, rather than hiding a second picker behind CSS. The deep stage's picker
   highlights its current selection in the same blue as the deep-actuation marker (`#3498db`,
   with `background-image: none` alongside the colour override — the theme's `.suggested-action`
   accent otherwise paints over a plain colour) rather than the theme's generic accent, so which
   stage's picker is on screen is unambiguous at a glance.
5. **Bind-primary-first gate.** With no primary Binding, the whole panel below the Actuation bar
   collapses to one line ("Bind a primary Action first…") — no Add-deep-stage affordance, no
   editor.

The panel sits in its own bounded/scrollable container the way the existing editor's
`actuation_scroller` (`binding_editor.py:992`) already does, so a deep stage's addition growing
the panel stays bounded by the popover's own sizing rather than growing unbounded — carry forward
that *intent*, not the prototype harness's specific pixel numbers (the prototype used its own
fixed bar width and monitor-bound scroll window purely to hold a non-resizable standalone test
window steady; the real popover has its own existing sizing constraints to fit into).

### Runtime behavior across Layer/Profile/connection changes

Settled by [ticket 05](./issues/05-interaction-sweep.md). Two corrections surfaced during this
sweep to what earlier tickets had assumed as precedent: **Layer switch force-releases nothing
today** for a plain key (a live Slot is silently re-bound against whatever sits at that Input on
the new Layer, or dangles) — real force-release-on-switch precedent exists only for
`SwitchProfile` and the Chord retroactive-miss path; and **there is no generic disconnect hook**
anywhere in the daemon today (Analog-repeat has no dropout handling at all).

- **Layer switch mid-press:** force-release **both** stages via `stage::Engine::stop_all()`,
  wired into `handle_layer_switch` — deliberately stricter than the plain-key precedent (which
  live-rebinds instead of releasing). The other Layer's stage for that Input, if it has one,
  always starts fresh from Up on the next physical crossing; it never picks up at the current
  Depth. Chosen because force-release matches what a user watching the Depth bar would actually
  expect, and the plain-key live-rebind quirk is an accepted historical wrinkle of the
  single-stage system, not a model worth propagating into a new feature.
- **Profile switch mid-press:** the same `stop_all()` joins `SwitchProfile`'s effect list as a new
  **`Effect::StopAllStages`**, alongside `Effect::StopAllToggles`/`Effect::StopAllAnalogRepeats`.
  A Profile switch fired by the deep stage's **own** Action completes that firing (via
  `commit_input_edits`, the same path the Chord-member-fires-`ProfileSwitch` precedent already
  uses) before the resulting `Edit::SwitchProfile` applies and tears the stage state down — the
  triggering firing is never itself interrupted by its own consequence.
- **Capture-mode flip to Digital mid-press:** the same `stop_all()` extends the existing
  `handle_capture_mode_change` Digital-transition branch (which already calls
  `analog_repeat.stop_all()` there). After the deep stage releases, the primary is untouched by
  the flip and simply inherits whatever Digital-sourced Down/Repeat/Up the key produces next —
  unchanged from a plain key's existing behavior on this transition.
- **Output suppression:** Chord-style suppression is moot for a dual-stage key — it can never be
  a Chord member (Q11), so it's never subject to Chord suppression to begin with. GUI-focus
  `StopAllToggles` (the manual "attention just moved to the GUI" escape hatch, distinct from
  suppression itself) is extended to also drain the deep stage's `Slots<StageKey>` toggles,
  alongside the individual path's — deliberately more aggressive than the Chord-toggle-survives-
  a-Profile-switch precedent, since it's a manual, not automatic, teardown.
- **Cascade-delete:** a live deep stage force-releases **immediately**, not on the next Up (there
  is no guarantee one ever arrives before the user notices stray output). `edit::apply` pushes a
  new per-key **`Effect::StopStage(input)`** whenever an edit removes a primary Binding that had a
  live `deep_base`/`deep_held` entry; `run_effects` calls the same per-key teardown the other
  cases above use. **No GUI confirmation dialog** — verified that no Binding delete anywhere in
  the editor is confirmed today, primary or otherwise, so a special-case dialog just for this
  would be inconsistent with the rest of the editor.
- **Reconnect / stuck-key reset:** a **new, explicit** disconnect hook in
  `handle_connection_change`'s `connected == false` branch force-releases every live deep slot and
  resets `stage::Engine`'s per-key deep-band `KeyState` + Quick-Skip runtime state — independent of
  capture's own synthetic-Up trick for the primary band (`relay_grid_blocking`, `analog.rs:829`),
  since that trick only ever covers the primary. This is new machinery, not a reuse of an existing
  path — none exists today for anything, including Analog-repeat, whose lack of dropout handling
  is a pre-existing gap this effort does not fix (see Out of Scope).
- **Chord-window proximity:** confirmed a non-issue by inspection, not by design — `chord::feed`
  only diverts an event when the Input is an actual Chord member, and a dual-stage key can never
  be one (Q11), so its events never enter the Chord window machinery regardless of physical
  proximity to a real Chord.

New mechanisms this section introduces: `Effect::StopAllStages` (global per-Layer/Profile/connection
teardown, calls `stage::Engine::stop_all()`), `Effect::StopStage(Input)` (single-key teardown for
cascade-delete), `Command::StopAllToggles`'s handler extended to drain `Slots<StageKey>` too, and
the new disconnect hook described above.

### Domain vocabulary

Six terms added to `CONTEXT.md`'s Configuration section (between **Release point** and
**Status LED**): **Actuation stage** (the `(ActuationPoint, Binding)` pair concept, covering
primary/deep and the disjoint-band/cascade-delete/Digital-inert rules in one entry — "primary
stage" did not earn a separate line, since it's fully covered by contrast inside the Actuation
stage entry) and **Staging mode** plus one short entry per mode (**Handoff**, **No-Return**,
**Additive**, **Quick-Skip**), mirroring the existing Trigger-mode / Fire-once / Hold-to-repeat /
Toggle / Analog-repeat structure exactly — a governing-concept entry followed by one entry per
variant.

## Testing Decisions

- **`stage.rs`'s pure core.** Table-tested per Staging mode against the exact transition tables
  above (`(prev band, new band, mode) -> [emitted ops]`), the same discipline `chord`/`axis`/
  `analog_repeat`'s pure cores already get — including the 1-report-skip rows and the
  Quick-Skip Armed/Skipped/Late runtime-state transitions.
- **`stage::Engine` integration**, exercised through the `rx_depth`/`rx_events` channels the way
  dispatch's other engines are: a same-report double-crossing resolves synchronously and produces
  the mechanically-replayed op sequence, not a short-circuited one; Quick-Skip's buffered primary
  either resolves to Skipped, Late (via the deadline arm), or is cancelled by an early Up or a
  Layer/Profile/capture-mode change while Armed.
- **`config::validate`.** One test per new `ConfigError` variant (seven), following the file's
  existing one-test-per-rule convention; the parse test
  `a_pre_dual_stage_config_defaults_deep_fields`; a `config_to_dict` assertion that
  `profile_to_dict` emits the three new keys.
- **`edit::plan`.** A unit test per new `Edit` variant confirming it mutates the right field and
  relies on `validate` rather than an inline check; a test that `SwitchProfile`'s effect list now
  contains `AssertStatusLeds`'s sibling, `StopAllStages`; a test that clearing a primary Binding
  with a live deep stage pushes `Effect::StopStage`.
- **Runtime teardown.** Tests (against the `stage::Engine`/`DispatchState` seam, following the
  Status-LEDs `led`-task-channel precedent of testing dispatch's decider logic through the
  channel rather than the real ioctl) that Layer switch, Profile switch, the Digital-mode flip,
  and the new disconnect hook each force-release every live deep slot and reset Quick-Skip runtime
  state; that GUI-focus `StopAllToggles` drains deep Toggles too.
- **GUI — `DaemonStub` contract.** The bind-primary-first gate collapses correctly with no primary
  Binding; adding/removing a deep stage toggles the Primary/Deep row and staging-mode row
  correctly; only one stage's picker is ever present in the built widget tree at once; the seven
  new `daemon_stub.py` validation rules raise the right errors, contract-tested the same way the
  existing rules are.
- No end-to-end test against real hardware is specified beyond what ticket 01 already relied on
  (the verified un-throttled `rx_depth` stream) — this is a software-only feature layered on an
  already-validated Depth pipeline.

## Out of Scope

Carried from the map so the implementation effort inherits the boundary:

- **Three or more Actuation stages** — capped at 2; key travel makes more impractical.
- **A deep stage with no primary Binding** — the model is "primary + optional second," never
  representable the other way.
- **Analog-repeat on either stage of a dual-stage key** — a v1 restriction; revisit as a fresh
  effort if wanted.
- **Deep-stage participation in Chord detection** — only the primary stage is Chord-eligible, and
  a Chord member cannot have a deep stage at all in v1.
- **A daemon→GUI "active stage" signal or `GetState()` field** — the 4-marker live Depth bar
  covers it; config stays the single source of truth.
- **Any digital-mode approximation of the deep stage** — inert in Digital mode, full stop, no
  Hold-to-repeat-style degradation the way Analog-repeat gets.
- **Fixing Analog-repeat's pre-existing lack of dropout/disconnect handling** — surfaced while
  resolving the reconnect question (Analog-repeat has no disconnect hook today and would sit stale
  on frozen Depth across a replug, with or without dual-stage keys). Pre-existing and unrelated to
  this feature; the new deep-stage engine gets its own explicit disconnect hook so it doesn't
  inherit the same gap, but the existing gap itself is a separate effort.
- **README / user-facing copy beyond this spec's own prose** — a full "Dual-stage keys" README
  section with a driving-sim framing example is left to the implementation effort to draft
  alongside the feature, the same way most feature README copy is written during implementation
  rather than during spec.
- **The implementation itself** — a separate effort, handed this finished spec.

## Further Notes

- **ADR.** [ADR-0007](../../docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md)
  ("Dual-stage depth interpretation runs in the dispatch task, not the analog capture source") is
  filed alongside this spec. It refines the spirit of the existing "dispatch owns Depth
  interpretation" pattern (Axis, Analog-repeat); it does not supersede or refine ADR-0002.
- **Cross-effort dependency, already folded in.** Post-release-development ticket 15 landed
  `trigger::Slots<K>` (the generic `(firings, toggles)` handle pair with `slot`/`snapshot`/
  `perform`/`force_release`/`stop_toggle`/`stop_all_toggles` as methods) after ticket 01's design
  session named a `firings`/`toggles` pair and a free `perform_trigger<K>` function that no longer
  exist under those names. Every reference to the deep stage's slot storage in this spec already
  uses the current `Slots<StageKey>` surface, not ticket 01's original hand-rolled-pair language.
- **Prior art in the tree.** `prototype/04-dual-stage-binding-editor-layout/prototype.py`
  (standalone GTK4) is the verified reference for the binding-editor layout, on `dev` — the
  release rebuild keeps `prototype/` and `.scratch/` out of `main`, so no separate throwaway
  branch was needed.
- Every ticket on the map (`.scratch/tartarus-dual-stage-keys/map.md`) is resolved as of this
  spec. Implementation is a **fresh effort** — this spec is the hand-off, and this map is done and
  can be archived.
