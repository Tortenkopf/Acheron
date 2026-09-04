# 02 — `stage.rs`'s pure state machine

**What to build:** A new pure `daemon/src/stage.rs` module — no runtime behavior
change yet, nothing wired into dispatch — that decides, for every `(primary band,
deep band, Staging mode)` combination, exactly which ops fire and in what order,
including same-report double-crossings (mechanical replay, never short-circuited) and
Quick-Skip's own Armed/Skipped/Late per-press runtime state. This is the prefactor
ticket 03's dispatch integration builds on; verified entirely through table tests,
the same discipline `chord`/`axis`/`analog_repeat`'s pure cores already get.

Source of truth: [`spec.md`](../../tartarus-dual-stage-keys/spec.md) §"Staging-mode
state machine", and
[ADR-0007](../../../docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md).

**Blocked by:** 01

**Status:** resolved

- [x] `daemon/src/stage.rs` imports nothing from `dispatch`/`edit`/`chord`/
      `config::Config` — the same discipline `chord`/`axis`/`analog_repeat` already
      hold to. It may import lightweight config types (`StagingMode`, `Binding`) the
      way `chord.rs` imports `Binding`/`ChordKey`/`TriggerMode` from `config`.
- [x] `StageKey(Input)` — a newtype distinguishing the deep slot's keyspace from the
      primary's bare `Input` keyspace, the same shape `ChordKey` gives the Chord
      executor, defined here (dispatch-internal, not config-schema-visible — unlike
      `ChordKey`, no `Profile` map is ever keyed by it).
- [x] `StageOp` — a data-only enum, one variant per emitted op: `FirePrimary` /
      `ReleasePrimary` / `RepressPrimary` / `FireDeep` / `ReleaseDeep` /
      `SuppressPrimary` / `Nothing`.
- [x] A pure `advance` function (or equivalent) taking the combined state
      `(primary band, deep band) ∈ {Up, Down}²` transition plus `StagingMode` (and,
      for Quick-Skip, the per-press Armed/Skipped/Late runtime state) and producing
      the ordered `Vec<StageOp>` exactly per spec.md's four transition tables:
  - **Handoff**: the seven-row table (real Primary Down/Up at the outer edges,
    Release-Primary→Fire-Deep and Release-Deep→Repress-Primary at the inner
    crossings, the two 1-report-skip rows, `Nothing` for no crossing).
  - **No-Return**: identical to Handoff except `(Down,Down)→(Down,Up)` emits
    `ReleaseDeep` only (no repress) and its 1-report-skip row drops the repress too.
  - **Additive**: the primary is never touched by the inner crossings — only
    `FireDeep`/`ReleaseDeep` at the inner transitions, real Primary Down/Up at the
    outer edges (including the 1-report-skip rows).
  - **Quick-Skip**: layered on Handoff's mechanics via a per-press Armed→Skipped/
    Late runtime state — Armed transitions to Skipped (`SuppressPrimary`→
    `FireDeep`) on reaching the deep band within the window (including
    synchronously-already-hot resolution), to Late (`RepressPrimary` retroactive→
    ordinary Handoff for the rest of the press) on the deadline elapsing, or
    cancels outright on an early Up or a Layer/Profile/capture-mode change. Once
    Skipped, every subsequent crossing for the rest of *this* press runs as
    Additive-with-no-primary (`FireDeep`/`ReleaseDeep` each crossing, release path
    = No-Return — primary permanently inert). Once Late, the rest of the press
    runs plain Handoff.
- [x] The disjoint-stacked-band invariant (`(Up,Down)` is structurally impossible) is
      asserted or documented as unreachable, not defensively handled —
      `RepressPrimary` fires unconditionally, never re-checking the primary's own
      hysteresis, per spec.md's reasoning.
- [x] A Quick-Skip timeout pair, `next_deadline`/`tick`, mirroring
      `chord::next_deadline`/`chord::tick`'s exact shape (same `Option<Instant>` /
      outcome-producing signature convention) — the seam ticket 04's dispatch-side
      buffer drives.
- [x] Ordering is resolved from the *arriving event's own* depth value where a
      same-report double-crossing needs to pick a direction — this module's job is
      to make that resolution a pure function call, not to depend on channel
      arrival order (that discipline is ticket 03/04's job in the `dispatch` shell;
      this ticket just needs the pure function to accept and correctly interpret a
      `(prev_depth_derived_bands, new_depth_derived_bands)` transition regardless of
      how many bands it crosses in one call).
- [x] Table tests per Staging mode against the exact transition tables in spec.md —
      every row, including the 1-report-skip rows and the Quick-Skip Armed/Skipped/
      Late runtime-state transitions (deep-reached-in-window, deadline-elapsed,
      early-Up-cancels, external-cancel). No tokio, no injector, no `Config` — pure
      `#[test]`s only, mirroring `trigger::tests::decision_table` /
      `analog_repeat::tests::tick_plan_*`.

## Comments

**2026-09-04** — Implemented as specified. `advance(prev, next, mode, quick_skip)`
takes explicit `Bands` (a local `Band { Up, Down }` pair, not `capture::analog::KeyState`
— the checklist's import discipline only allows lightweight `config` types, so `Band`
is defined locally rather than reaching into `capture`) and returns
`(Vec<StageOp>, Option<QuickSkipPhase>)`; `next_deadline`/`tick` take/return
`Option<QuickSkipPhase>` by value rather than mutating an owned machine struct —
`chord::ChordMachine`'s shape didn't fit here since Quick-Skip's whole runtime state is
one small `Copy` enum (`Armed { deadline } | Skipped | Late`), so a stateless
functional shape stayed simpler than a struct with `&mut self` methods while still
matching chord's `Option<Instant>` / outcome-producing signature convention.

One deliberate convention beyond the checklist: `StageOp::Nothing` (the literal
"no crossing" table row) is kept distinct from an *empty* `Vec` (a real crossing whose
designed effect is silence — Quick-Skip's early-Up cancel, and the primary's
permanently-inert final release once Skipped) — see the module's own doc comment.
Collapsing the two would blur "nothing crossed" with "something crossed but is
deliberately silent," which spec.md treats as distinct cases.

The 1-report-skip "already hot" Quick-Skip resolution (ADR-0007: "resolves immediately
to skip if the deep band is already hot") needed no special-cased `begin` function —
it falls out for free as `advance`'s ordinary `(Up,Up)→(Down,Down)` row when
`quick_skip` is `None`, exactly the mechanical-replay discipline spec.md asks for.

`/code-review` caught one real finding: the module was fully unwired (as its own doc
comment says) with no consumer yet, so `cargo clippy --all-targets -- -D warnings`
(CONTRIBUTING.md's documented gate) failed on 15 dead-code errors. Fixed with a
module-level `#![allow(dead_code)]`, commented as temporary and to be removed once
ticket 03's `stage::Engine` wires this module in.

`cargo fmt --check` clean, `cargo clippy --all-targets -- -D warnings` clean, full
daemon suite 430 green (415 prior + 15 new in `stage::tests`).
