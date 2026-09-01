<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 09 — Concentrate the dispatch task's runtime state into one `DispatchState`

**What to build:** The ~11 loose `let mut` runtime locals in `dispatch::run` —
`toggles`, `in_flight`, `stepper_cursors`, `active_layer`, `chord_machine`,
`chord_runtime`, `axis_state`, `analog_repeats`, `device_connected`,
`capture_mode`, `device_info` — plus the five task-lifetime handles the helpers
thread by hand (`injector`, `signal_emitter`, `actuation_tx`,
`capture_control_tx`, `toggle_lap_target`) move into one `DispatchState` struct.
`handle_event`, `handle_command`, `run_chord_effects`, `run_effects`,
`commit_input_edits`, `update_analog_repeats`, `handle_depth_update`, and
`dispatch_individual_down` become `&mut self` methods on it. `run` stays a free
function: receive the 13 startup parameters, build one `DispatchState`, then
`loop { select! { … => state.handle_*(&config, …).await } }`.

**No behaviour changes and no logic moves.** Every method body is its current
function body with `param` rewritten to `self.field`. This is the last
structural weight in `dispatch.rs` after tickets 03–08.

## The friction

`dispatch.rs` is the repo's single biggest file (5,162 lines) and its top churn
hot-spot — 37 commits touch it, and unlike the archived `tartarus-input-expansion`
work the churn is *ongoing* post-1.0 bug-fixing (tickets 80–84, 96, 105, plus
the recurring "verify on hardware" fixes). Every one of those edits pays the
same tax:

- `run` holds its runtime state as **~11 bare `let mut` bindings**. Every helper
  re-declares the subset it needs: `handle_event` takes **13 parameters** under
  `#[allow(clippy::too_many_arguments)]`, `run_chord_effects` 9,
  `update_analog_repeats` 7. Five `#[allow(clippy::too_many_arguments)]` sit in
  the file.
- Two `macro_rules!` — `trigger_ctx!` (top of file) and `effect_ctx!` (inside
  `run`) — exist **only** to rebuild a borrow struct at each call site because
  the state they bundle is 9–11 loose locals. `EffectCtx` and `TriggerCtx` are
  already partial, hand-rolled bundles of subsets of that same state.
- `dispatch.rs:525–528` is a standing comment refusing to bundle `run`'s
  *channel* parameters ("not something a struct would meaningfully group") —
  correct for the channels, but the *internal runtime locals* are a different
  set, and they are exactly what a struct groups well.

Tickets 05 / 07 / 08 each extracted a *concept* (`edit`, `chord`, `trigger`)
behind a pure interface. **This candidate is a different kind of move** — it
reshapes the orchestrator's own state, not a new module. It buys locality (one
owner for dispatch runtime state) and interface (`&mut self` replaces the 13-arg
lists), not depth.

## The shape

A throwaway compile-spike (reverted) confirmed the following borrows all hold.

```rust
// dispatch.rs — dispatch-internal, never part of any module's interface.

/// Every piece of ephemeral runtime state the dispatch task owns — reset
/// fresh on every task start, the same lifetime as today's loose `run`
/// locals. NOT `Config` (committed state; stays a `run` local so the input
/// path keeps only `&Config` — ticket 05). NOT the `rx_*` receivers or
/// their `*_open` liveness flags (pure `select!`-loop plumbing; no handler
/// touches them). No lifetime parameter — every handle below is owned and
/// `'static`.
struct DispatchState {
    toggles: HashMap<Input, ActiveToggle>,
    in_flight: HashMap<Input, FiringHandle>,
    stepper_cursors: HashMap<StepperId, usize>,
    active_layer: Layer,
    chord_machine: chord::ChordMachine,
    chord_runtime: ChordRuntime,      // retained as-is
    axis_state: AxisState,            // retained as-is
    analog_repeats: HashMap<Input, ActiveAnalogRepeat>,
    device_connected: bool,
    capture_mode: CaptureMode,
    device_info: Option<DeviceInfo>,
    injector: Injector,
    signal_emitter: Option<SignalEmitter<'static>>,
    actuation_tx: watch::Sender<HashMap<Input, ActuationPoint>>,
    capture_control_tx: mpsc::Sender<bool>,
    toggle_lap_target: Duration,
}

async fn handle_event(
    &mut self,
    config: &Config,
    event: PhysicalEvent,
) -> io::Result<Vec<edit::Edit>>;   // 13 params -> 3
```

- **`select!` borrow-checks.** The `rx_*` receivers stay `run` locals, so the
  branch expressions (`rx_events.recv()`, `rx_depth.changed()`, …) borrow only
  those locals, never `state` — no collision with the `&mut self` handler in the
  winning arm. The chord-deadline arm is fine: `chord::next_deadline(&state.chord_machine)`
  in the branch expression returns a `Copy` `Option<Instant>` and retains no
  borrow when `chord::tick(&mut state.chord_machine, …)` runs in the handler.
  The `rx_depth` arm's two sequential `&mut self` calls
  (`handle_depth_update` then `update_analog_repeats`, the latter also taking
  `&rx_depth`) are fine.
- **`EffectCtx` + `effect_ctx!` delete.** All nine `EffectCtx` fields are
  `DispatchState` fields; `run_effects` / `handle_command` / `commit_input_edits`
  touch `self.*` directly.
- **`TriggerCtx<'a, K>` + `trigger_ctx!` + `perform_trigger<K>` survive,
  unchanged.** A `&mut self` method cannot be generic over which map (`self.in_flight`
  vs `self.chord_runtime.firings`) type `K` selects. `perform_trigger<K>` stays
  a free function; each `trigger_ctx!` site builds from a disjoint
  `&mut self.<map>` borrow plus `&config`. This is an accepted, load-bearing
  residual — the shared two-key-type executor ticket 08 built deliberately, not
  loose-local threading.
- **`#[allow(clippy::too_many_arguments)]`: 5 → 1.** Only `run` keeps it (it
  still receives all 13 startup params to build the struct; the struct literal
  itself trips no lint). Removed from `handle_event`, `run_chord_effects`,
  `update_analog_repeats`, and `dispatch_individual_down` (itself carved in
  ticket 07, now also a method).

## What moves, what stays

- **Into `DispatchState` (as fields):** the 11 runtime locals; `injector`,
  `signal_emitter`, `actuation_tx`, `capture_control_tx`, `toggle_lap_target`.
- **Becomes `&mut self` methods:** `handle_event`, `handle_command`,
  `run_chord_effects`, `run_effects`, `commit_input_edits`,
  `update_analog_repeats`, `handle_depth_update`, `dispatch_individual_down`.
  `config` / `config_path` are passed as `&Config` / `&Path` to the methods that
  need them.
- **Stays a `run` local:** `config`, `config_path`, every `rx_*` receiver, every
  `*_open` flag. `run` remains a free fn — construct-then-`select!`, not folded
  into a `DispatchState::run` method.
- **Unchanged:** `TriggerCtx<'a, K>`, `trigger_ctx!`, `perform_trigger<K>`,
  `compile_action`, `slot_for`, `chord_slots`, `wait_for_chord_deadline`, every
  pure helper, and every `chord::` / `trigger::` / `edit::` call.
- **Deleted:** `EffectCtx`, `effect_ctx!`.

## Landing in one pass

Introduce the struct, convert all eight helpers, and rewire `run` in one pass —
a half-migrated `run` carrying both loose locals and `self.` is less readable
than either alone (03–08 precedent). The test-partition sweep may stage behind
the implementation within the same PR.

**Risk profile: mechanical reshape — lighter than 07 / 08.** No `match` arm is
rewritten, no decision logic is ported. The check is a `param` → `self.field`
diff, plus confirming the `select!` and `trigger_ctx!` borrows. The full daemon
suite and every `run_scripted` end-to-end test must pass against the new code
**before** any test is deleted. `/code-review` on the Standards and Spec axes,
as 05 / 07 / 08.

## Tests: replace, don't layer

- **New `DispatchState` unit seam** — a tokio runtime + a `RecordingSink`
  injector + a fake `Config`; **no channels, no tempfile**. Feed `PhysicalEvent`s
  and `Command`s directly, assert injector writes + returned `Edit`s. The
  per-handler-logic tests currently routed through `run_scripted` /
  `CommandHarness` move here.
- **Stays full-rig** (`run_scripted` / `CommandHarness` / the D-Bus
  `TestServer`): channel-close behaviour (the `*_open` flags),
  `spawn_with_failing_persist` rollback, `select!` arm interleaving, and the
  D-Bus round trip. These genuinely need the plumbing.
- Net test count holds or drops. The exact test-by-test partition is decided
  during the work.

## Out of scope

- **Carving the `axis` and `analog_repeat` engines out of the `rx_depth` arm**
  (review candidate 2) — a *concept* extraction behind a pure interface, its own
  ticket. It touches `handle_depth_update` / `update_analog_repeats` again; the
  double-touch is accepted. This ticket only turns `axis_state` / `analog_repeats`
  into `self.` fields.
- **Folding `run` into a `DispatchState::run` method** — `run` stays the
  constructor-plus-loop.
- **Removing `TriggerCtx` / `trigger_ctx!` / `perform_trigger<K>`** — the
  generic-`K` executor is deliberate (ticket 08).
- **Moving `config` under `DispatchState`** — degrades ticket 05's
  compiler-enforced "input path holds only `&Config`" seam.
- Any change to dispatch behaviour, effect ordering, or the wire.

## Recording the decision

No `CONTEXT.md` term — `DispatchState` is an implementation artifact, not domain
vocabulary (ticket 06's `rules` call). No ADR — the candidate is accepted, not
rejected. One `CONTRIBUTING.md` bullet, in the house style of the "new mutating
`Command`" / "Changing Chord-detection behaviour" entries: *adding a new piece
of dispatch runtime state means a `DispatchState` field, not a fresh `run` local
or a new `handle_*` parameter.*

**Blocked by:** None — tickets 07 (`9b88763`) and 08 (`7061ea3`) are resolved.

**Status:** resolved

- [x] `DispatchState` exists in `dispatch.rs` with no lifetime parameter, owns
      the 11 runtime fields (with `ChordRuntime` / `AxisState` retained as
      nested structs) plus `injector`, `signal_emitter`, `actuation_tx`,
      `capture_control_tx`, `toggle_lap_target`, and owns **none** of `config`,
      `config_path`, any `rx_*` receiver, or any `*_open` flag.
- [x] `handle_event` is `async fn handle_event(&mut self, config: &Config,
      event: PhysicalEvent) -> io::Result<Vec<edit::Edit>>` — no other
      parameters.
- [x] `handle_command`, `run_chord_effects`, `run_effects`, `commit_input_edits`,
      `update_analog_repeats`, `handle_depth_update`, and `dispatch_individual_down`
      are `&mut self` methods; `config` / `config_path` reach them as `&Config` /
      `&Path` arguments where needed.
- [x] `EffectCtx` and `effect_ctx!` no longer exist.
- [x] `TriggerCtx<'a, K>`, `trigger_ctx!`, and `perform_trigger<K>` remain,
      unchanged in shape; each `trigger_ctx!` site builds from a disjoint
      `&mut self.<map>` borrow.
- [x] `run` is a free fn that takes the 13 startup parameters, builds one
      `DispatchState` (via `DispatchState::new`), and runs the `select!` loop;
      the `rx_*` receivers and the `*_open` flags are `run` locals.
- [x] `#[allow(clippy::too_many_arguments)]` appears exactly once, on `run`;
      it is gone from `handle_event`, `run_chord_effects`, `update_analog_repeats`,
      and `dispatch_individual_down`.
- [x] No behavioural diff: the full daemon suite (345) passed against the new
      code before any test was migrated.
- [x] The new `DispatchState` unit seam (`Seam`) exists; 13 per-handler-logic
      tests moved to it (passthrough, keypress remap, Hold-to-repeat
      refire/force-release/carve-outs, Toggle hold-vs-loop/start-stop, the
      three Layer-switch tests); `run_scripted` + the `FakeCaptureSource`
      import deleted. `CommandHarness` keeps the persist-failure,
      invariant-rollback, D-Bus, actuation-snapshot, and remaining
      command/persistence coverage. Net daemon test count unchanged at 345
      (≤ current). A further sweep of pure command-translation tests onto
      `Seam` is possible but left as follow-up — most `CommandHarness`
      command tests also assert the on-disk `config.toml`, which needs the rig.
- [x] `CONTRIBUTING.md` has the "Adding a new piece of dispatch runtime state"
      bullet (names the 8 `&mut self` handlers and the leaf helpers that stay
      free).
- [x] Full daemon suite green (345); `cargo fmt --check` clean; `cargo clippy
      --all-targets -- -D warnings` clean. GUI and packaging suites untouched
      (no wire / D-Bus / catalog change).
- [x] `/code-review` on the Standards and Spec axes — both run, findings applied
      (see Comments).

## Comments

**2026-09-01** — Filed from an architecture-review grilling session (candidate 1
of the four-candidate 2026-09-01 review; candidate 3 was declined the same day →
`docs/adr/0005-gui-test-stub-mirrors-daemon-validation-by-hand.md`; candidates 2
and 4 remain open). Design tree settled over four rounds:

- **Worth doing at all** — weighed directly against ADR 0005, filed hours
  earlier, which declined candidate 3 (which *had* a user-facing correctness
  stake) as "payoff all in tests, low churn, stable surface". Candidate 1 has
  **zero correctness stake** and is accepted as pure ergonomics, on a different
  basis: `dispatch.rs` is the repo's #1 churn hot-spot and the churn is
  *ongoing* post-1.0 bug-fixing, not archived feature work — deep structure here
  pays back on every future fix. Ticket 07's "Out of scope" section had already
  named this candidate ("A `DispatchState` struct for the `run` task's ~18
  locals … Separate candidate"); the name is taken from there.
- **Threshold, set before the spike:** "real win" = `handle_event` 13 → ~3,
  both macros gone, no lifetime parameter, `#[allow]`s mostly gone; "cosmetic" =
  `trigger_ctx!` survives, 13 → ~7, `EffectCtx` lingers. The spike landed ~80%
  of "real": 13 → 3, `EffectCtx` + `effect_ctx!` gone, no lifetime parameter,
  4-of-5 `#[allow]`s gone — with `trigger_ctx!` / `TriggerCtx<K>` /
  `perform_trigger<K>` surviving. Judged to clear the bar: the surviving macro
  is the deliberate two-key-type (`Input` / `ChordKey`) executor from ticket 08,
  not loose-local threading — the thing this candidate targets is the latter,
  and it is removed.
- **`config` stays a `run` local, out of `DispatchState`** — preserves ticket
  05's compiler-enforced seam (input path holds only `&Config` and returns
  `Edit`s; `run` is the sole commit point). `DispatchState` is the *ephemeral*
  half (reset every task start); `config` is the *committed* half.
- **`*_open` flags stay `run` locals** with their receivers — pure `select!`-loop
  plumbing, read by no handler.
- **Name `DispatchState`** (from ticket 07's forward-reference). `run` stays a
  free constructor-plus-loop fn, not a method.
- **Scope: state-reshape only** — review candidate 2 (the `axis` / `analog_repeat`
  engine carve on the `rx_depth` arm) stays its own ticket; the double-touch of
  `handle_depth_update` / `update_analog_repeats` is accepted. "A different kind
  of move", in the review's own words.
- **Tests: replace, don't layer** — a `DispatchState` unit seam absorbs the
  per-handler-logic tests; the full rigs keep only what needs channels,
  persistence, or D-Bus.
- **One pass** (03–08 precedent), mechanical-reshape risk profile (lighter than
  07 / 08 — no logic ported). `/code-review` Standards + Spec. No ADR, no
  `CONTEXT.md` term.

Facts dug from the code during the grilling (not asked of the user):

- `dispatch.rs` is 5,162 lines; `git log --follow` shows 37 commits touching it.
- `dispatch.rs:525–528` already refuses to bundle `run`'s *channel* parameters
  into a struct — a different set from the internal runtime locals this ticket
  bundles.
- `Injector` is `#[derive(Clone)]` and owned; `SignalEmitter<'static>`,
  `watch::Sender`, `mpsc::Sender`, `Duration`, and `PathBuf` are all owned and
  `'static` — so `DispatchState` needs no lifetime parameter.
- `EffectCtx`'s nine fields are all prospective `DispatchState` fields;
  `TriggerCtx<'a, K>` is generic over the slot-key type, which no `&mut self`
  method can be — hence the asymmetric outcome (one macro deletes, one stays).
- `dispatch_individual_down` (carved from `handle_event`'s tail in ticket 07) is
  shared by the ordinary Down path and the chord `FireIndividual` executor —
  becomes a method, both call sites unchanged.
- Compile-spike (throwaway, reverted; `git status` left clean) built a
  `DispatchState` skeleton with real field types and a real `tokio::select!` in
  a `run_spike()`; `cargo build` and `cargo test --no-run` both passed.

**2026-09-01 — Resolved.** The reshape landed in one pass as specced:

- `DispatchState` (no lifetime param) holds the 11 ephemeral fields plus the 5
  owned collaborators; `config` / `config_path` / `rx_*` / `*_open` stay `run`
  locals. `run` builds it via `DispatchState::new(injector, signal_emitter,
  actuation_tx, capture_control_tx, toggle_lap_target)` and then only drives
  `select!`. The 8 handlers are `&mut self` methods in **one** `impl
  DispatchState` block (one-inherent-impl-per-type, matching `config.rs`).
- `EffectCtx` + `effect_ctx!` deleted. `TriggerCtx<'a, K>` / `trigger_ctx!` /
  `perform_trigger<K>` unchanged; each site now builds from a disjoint
  `&mut self.<map>` borrow. `#[allow(clippy::too_many_arguments)]`: 5 → 1
  (only `run`). The `select!` and `trigger_ctx!` borrows all hold, as the
  spike predicted.
- Tests: new `Seam` direct-`DispatchState` harness (RecordingSink injector,
  in-memory `Config`, stub channels). 13 per-handler tests migrated off
  `run_scripted` / `CommandHarness`; `run_scripted` + `FakeCaptureSource`
  deleted. Net daemon count unchanged (345). `CommandHarness` retains the
  persist-failure / invariant-rollback / actuation-snapshot / D-Bus / channel
  coverage plus the command-translation tests that also assert on-disk
  `config.toml`.
- `CONTRIBUTING.md`: "Adding a new piece of dispatch runtime state" bullet
  (lists the 8 `&mut self` handlers; notes the 4 leaf helpers —
  `handle_layer_switch`, `handle_connection_change`,
  `handle_capture_mode_change`, `handle_axis_edge_event` — deliberately stay
  free fns taking a narrow `&mut` borrow).

`/code-review` (Standards + Spec, parallel):

- **Standards** — flagged (hard) that the first draft of the CONTRIBUTING
  bullet overclaimed "every `handle_*` helper is a method" → reworded to name
  the 8 and carve out the leaf helpers. Flagged the 5 scattered `impl` blocks
  → consolidated to one. Flagged the 17-field struct literal duplicated in
  `run` and `Seam` → added `DispatchState::new`. Data-clump on
  `&Option<SignalEmitter>` + flag left as-is (those helpers are out of scope
  per "what stays").
- **Spec** — all checklist items verified against the diff. Noted the test
  partition is partial (pure command-translation tests still on the rig) —
  accepted, hedged by the ticket, net count holds. Flagged stale doc comments
  (`ChordRuntime` "one `run`-local", `TriggerCtx` "same discipline as
  `EffectCtx`") → corrected.

Full daemon suite green (345); `cargo fmt --check` + `cargo clippy
--all-targets -D warnings` clean.
