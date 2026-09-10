# Runtime-engine lifecycle teardown is one `match`, not a `DepthEngine` trait

The dispatch task owns several pieces of ephemeral runtime state — the individual
(`Input`-keyed) firing/toggle pair, the Chord (`ChordKey`-keyed) pair and its pure
`ChordMachine`, `axis::Engine`, `analog_repeat::Engine`, `stage::Engine`. Four situations
tear some subset of that state down: a **Layer switch**, a **Profile switch**, a device
**disconnect**, and a capture-mode flip **to Digital**. Before
`post-release-development` ticket 19 the "what gets released in which situation" matrix
was spread across four structurally different pieces of code — three bespoke
`#[allow(clippy::too_many_arguments)]` leaf functions plus a five-variant `Effect`
fan-out through `run_effects` — with no single place to read it and holes (`axis` /
`analog_repeat` not touched on disconnect; `axis` not reset on the Digital flip) that
nothing marked as decisions rather than bugs.

## Decision

Lifecycle teardown is **one `DispatchState::tear_down(reason: TeardownReason)` method** —
a single `match reason { … }` where every arm names every participant, with an explicit
`//` line for each participant it deliberately leaves alone. `TeardownReason` is
`{ LayerSwitch, ProfileSwitch, Disconnect, CaptureModeToDigital }` — a plain `Copy` data
enum that lives in `edit.rs` next to `Effect` (so `edit` stays a leaf module: `dispatch`
imports `edit`, not the reverse, and `Effect::TearDown` needs the type). Two entry points
feed the one match:

- a **direct call** for the three situations that touch only momentary `DispatchState`
  (`handle_layer_switch`, `handle_connection_change`, `handle_capture_mode_change`, now
  `&mut self` methods on `DispatchState`);
- **`Effect::TearDown(reason)`** for the Profile switch, because a `SwitchProfile` mutates
  `Config` and the `run` loop is the sole commit point — its teardown has to run as data
  through `run_effects`, alongside `RepublishActuation` / `AssertStatusLeds` /
  `AnnounceProfileChange`. This replaced the five separate `StopAllToggles` /
  `ReleaseAllHolds` / `ResetAxisOutputs` / `StopAllAnalogRepeats` / `StopAllStages`
  effect variants; their fan-out order now lives in the `ProfileSwitch` arm.

The deadline-arm wrapper `wait_for_deadline(Option<Instant>)` is likewise single-sourced —
the Chord window and the Quick-Skip timeout kept byte-identical copies (`wait_for_chord_deadline`
/ `wait_for_stage_deadline`); the two `select!` arms keep their own next-deadline source and
share only the wait wrapper.

## Why not a `DepthEngine` trait

Three consecutive architecture reviews (2026-09-03, -03b, -06) escalated "give the
Depth-fed engines a shared interface" — from an axis-emit dedup, to a `reset()` fan-out
method, to a full `trait DepthEngine { wants; observe; next_deadline; on_teardown }`
behind `Vec<dyn DepthEngine>`, with the fallback "even adopting only the
`on_teardown(reason)` half". No review rated it Strong. The 2026-09-09 grilling settled
on the minimal cut — the `match`, no trait — for reasons load-bearing enough to record:

- **The engines are radically heterogeneous.** `axis::Engine` is sync / infallible /
  holds no `&Injector` / returns `Vec<AxisWrite>` the caller emits. `analog_repeat::Engine`
  is async and owns spawned tokio tasks with their own `CancellationToken` timers.
  `stage::Engine` is async / fallible (`io::Result`) with a 7-field `EngineDeps` and
  reaches into `DispatchState::individual` + `stepper`. `chord`'s `ChordMachine` holds
  **no handles at all** — it is a pure free-fn family whose teardown target is a *sibling*
  field (`chord_slots`), and it is not fed from `rx_depth`. A uniform
  `observe(snapshot) -> Ops` / `wants(cfg, layer)` interface fits none of them without a
  lowest-common-denominator that hides more than it reveals.
- **`Vec<Box<dyn DepthEngine>>` with async methods fights edition 2024 and this codebase.**
  Async-fn-in-trait is not `dyn`-compatible without `async-trait` (only a transitive dep
  via `zbus` today) or hand-boxed `Pin<Box<dyn Future>>`; the daemon has **zero** `dyn`
  trait objects — its established pattern is a pure sync core plus one async shell.
- **The `rx_depth` arm is the sub-millisecond hot path** (continuous analog-stick `ABS_*`
  output). A `dyn`-dispatched `observe` there is a real cost. A `dyn`-dispatched
  `on_teardown` would be fine — human-scale events — but is not worth the boilerplate over
  a `match`.
- **Precedent.** `post-release-development` ticket 10 already declined a `depth::` umbrella
  module: the engines "share only the `rx_depth` snapshot value … CONTEXT.md has no such
  concept." Teardown is even less of a domain concept — `tear_down` / `TeardownReason` are
  internal plumbing with no CONTEXT.md term (no domain entry, per the ticket 10 / 14 / 15 /
  17 precedent of declining one for a locus refactor).

## Consequences

- A reviewer reads the whole 4-situation × 6-participant matrix in one `match`; each
  deliberately-skipped cell has a written rationale beside it. A fifth engine, or a new
  `TeardownReason`, is one visible edit per arm.
- The refactor changed **no behaviour** — the four arms were transcribed from the four
  former call sites. Turning one of the `//`-marked skips into a real call
  (`axis`/`analog_repeat` on disconnect, `axis` on the Digital flip, any `chord` teardown)
  is `post-release-development` ticket 20's job, one at a time, each with its own reasoning
  and possibly a `spec.md` change (disconnect handling is currently declared out of scope).
- The Quick-Skip buffer / individual-Toggle / Chord-Toggle survival rules stop living only
  in an `edit.rs` doc comment — they are `//` lines in the arm that owns them, and the
  `dispatch::tests::tear_down_*` matrix tests assert them as executable spec.

Adjacent to **ADR-0007** (where staged Depth is interpreted) — this decision is about
lifecycle, which was always out of ADR-0007's scope; it refines nothing there.

**ADR-0011** is the sibling: the same "one place, not a per-site push" move for the
**config-edit** axis (`edit::reconcile_teardowns` — the `StopToggle` / `StopStage` /
`StopChord` a committed `Edit` orphans, derived by diffing the active Profile). The two
ADRs are the two axes of one story.
