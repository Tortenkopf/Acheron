# Acheron — Either-Or Staging mode

Status: approved (2026-09-25) — implementation tickets in `issues/`

Extends [`tartarus-dual-stage-keys/spec.md`](../tartarus-dual-stage-keys/spec.md)
§"Staging-mode state machine". Settled in one grilling session (a wayfinder charting
that found no fog, so no map): Q1–Q7, recorded under "Decisions" below.

## Problem Statement

A dual-stage key under every existing Staging mode lets a press reach the deep stage
*after* the primary has already fired: Handoff and No-Return hand off whenever the deep
band is crossed, and Quick-Skip falls back to plain Handoff once its ~50 ms window
elapses. A user who wants each press to be *either* the primary *or* the deep binding —
never both — has to be careful not to push through into the deep band while holding the
primary. There is no mode that makes that accident impossible.

Separately, Quick-Skip silently drops a quick, shallow tap: a press that crosses the
primary point and releases within the window, never reaching the deep band, emits
nothing at all. The only rationale ever recorded
([ticket 02](../tartarus-dual-stage-keys/issues/02-dual-stage-state-machine.md): "never
held long enough") treats a sub-50 ms press as noise, which the Actuation/Release
hysteresis already filters. It is a bug, and it is fatal to the new mode's promise.

## Solution

A fourth Staging mode, **Either-Or**: Quick-Skip's window, with the deep stage locked
out once the primary fires.

- Reach the deep band within 50 ms of crossing the primary Actuation point → **deep
  only**. The primary never fires this press (identical to Quick-Skip's Skipped).
- The 50 ms elapse without reaching the deep band → **primary only**. The primary fires
  (up to 50 ms late) and the deep band is **inert** for the rest of the press: the
  primary stays held straight through any excursion into and out of the deep band and
  releases only on the real Up.
- Release within 50 ms without reaching the deep band → **primary, as a tap** (see
  "Early-Up flush" below).

The early-Up flush is also applied to **Quick-Skip**, fixing its dropped-tap bug.

Wire / config name: `either_or`. GUI label: "Either-Or". Uses the same fixed 50 ms
`QUICK_SKIP_WINDOW` constant, measured from the same moment (the real primary `Down`
edge, armed in `begin_quick_skip`); not configurable.

## User Stories

1. As a user, I want a dual-stage key where a fast full press fires only the deep
   binding and a normal press fires only the primary, so I can pick one of two actions
   per keystroke by how fast I press.
2. As a user holding the primary on such a key, I want pushing further down to do
   nothing, so I can't accidentally fire the deep binding mid-hold.
3. As a user, I want a quick tap on an Either-Or or Quick-Skip key to still fire the
   primary, not vanish.
4. As a user binding a Controller button (or any Hold-to-repeat key) as the primary, I
   want that quick tap to be held long enough for a game to see it.

## Decisions

| # | Question | Answer |
|---|---|---|
| Q1 | Late: what does "deep unreachable" mean? | Deep band fully inert once Late; primary held through any depth excursion; released only by the real Up. No feedback. |
| Q2 | Skipped behaviour | Identical to Quick-Skip's Skipped: primary permanently inert, deep fires on every dip into the deep band and releases on every rise out of it. |
| Q3 | Early Up inside the window | Flush the buffered primary as a tap — for Either-Or *and* Quick-Skip (the drop has no load-bearing reason). |
| Q4 | Window | Reuse `QUICK_SKIP_WINDOW` (50 ms), same start point; not configurable. |
| Q5 | Name | Either-Or (`either_or`). |
| Q6 | Destination | No map — this spec, then a test-first implementation. |
| Q7 | Flushed-tap dwell | Down immediately, Up deferred by `executor::FIRE_ONCE_KEY_DWELL` (40 ms). |

## Implementation Decisions

### State machine (`daemon/src/stage.rs`)

Either-Or shares Quick-Skip's per-press phase machine (`None` → Armed → Skipped / Late),
its dispatch-side buffer (`begin_quick_skip` / `end_quick_skip`), its deadline
(`next_deadline` / `tick`), and its `feed` routing rows. Every `deep_cfg.mode ==
StagingMode::QuickSkip` check in `Engine` becomes "is a windowed mode" (Quick-Skip or
Either-Or). Renaming `QuickSkipPhase` / `quick_skip` to something mode-neutral is left
to the implementer.

The two modes differ **only in the Late phase**:

**Either-Or, from Late**

| Transition | Emitted ops |
|---|---|
| (Down,Up)→(Down,Down) | Nothing |
| (Down,Down)→(Down,Up) | Nothing |
| (Down,Up)→(Up,Up) | Primary Up (real) |
| (Down,Down)→(Up,Up), 1-report skip | Primary Up (real) |
| no crossing | Nothing |

Late is entered only by `tick` (the deadline), which always happens with the key at
`(Down, Up)`; the phase returns to `None` when the key reaches `(Up, Up)`. The lone
`ReleasePrimary` rows are left to the real `Up` event, exactly as Quick-Skip Late's
Handoff rows already are (`update`'s "lone FirePrimary/ReleasePrimary" skip).

Consequences the implementation must honour:

- **`deep_repeat` must be a no-op for an Either-Or key in Late.** Today it re-fires a
  Hold-to-repeat deep stage off every primary `Repeat` whenever `rt.deep == Down`; with
  an empty deep slot, `trigger::decide`'s "`Repeat` with no firing re-presses first"
  rule would *fire the deep stage* — precisely what Either-Or forbids.
- `primary_handed_off` is never set in Either-Or Late (no `ReleasePrimary` op), so the
  primary's own `Repeat`s keep reaching the ordinary path — a Hold-to-repeat primary
  keeps repeating while the key sits in the deep band. This is intended.
- The `just_reset` re-adoption's `primary_handed_off` re-confirmation (`update`, the
  `matches!(deep_cfg.mode, Handoff | NoReturn)` line) stays as-is: Either-Or never
  hands off.

### Early-Up flush (Either-Or and Quick-Skip)

Replaces the Quick-Skip table row "Up arrives first → cancelled: buffered Down dropped,
nothing emitted" in the dual-stage spec.

| From Armed | Result |
|---|---|
| Up arrives first (deadline not elapsed, deep never reached) | **Flush**: the buffered primary Down is performed now (`fire` against `individual`, exactly as `tick`'s `RepressPrimary`); its Up is performed `FIRE_ONCE_KEY_DWELL` (40 ms) later as a *real-Up-shaped* release (`decide(binding, Up, slot)` + `perform`, not `ReleasePrimary`'s `stop_toggle` + `force_release`) |

- **Real-Up semantics, per Trigger mode** — the flushed tap behaves as a physical tap on
  a single-stage key would, just shifted: Fire-once fires once (its own dwell applies);
  Toggle starts and keeps running (a Toggle outlives its release); Hold-to-repeat,
  mouse and Controller button primaries are held for 40 ms then released; a Macro runs;
  a ProfileSwitch primary returns its `Edit` from the Down.
- **The deferred release is a timed event on the engine**, not a sleep in the dispatch
  loop. It rides the same `next_deadline` / `tick` mechanism the window already uses
  (e.g. a per-key pending-release deadline), so the `run` loop's existing
  `wait_for_stage_deadline` arm fires it.
- **Re-press before the deferred release lands**: a new primary `Down` on that key
  performs the pending release immediately, then begins the new press normally.
- **`stop_all` / `stop_stage` with a release pending**: the pending release is cancelled
  and the primary force-released through the existing teardown paths (the flushed
  primary lives in `individual`, like a Late primary does).
- The flush only replaces the *early-Up* cancellation. A Layer/Profile switch or
  capture-mode flip while Armed still cancels silently via `stop_all` — nothing fires.
- Physical-plausibility ceiling: the 40 ms is the ceiling's own canned-tap dwell, and
  clears the ~35 ms per-frame controller poll noted under Macro in `CONTEXT.md`.

### Config, wire, GUI

- `config::StagingMode` gains `EitherOr` (serde `either_or`). No `schema_version` bump
  and no new `config::validate` rule (additive enum variant; every existing deep-stage
  rule applies unchanged).
- `dbus/wire.rs` `staging_mode` to/from string gains `"either_or"`.
- `gui/acheron_gui/daemon_stub.py` `_STAGING_MODES` gains `"either_or"`.
- `gui/acheron_gui/binding_editor.py` `STAGING_MODES` gains an entry after Quick-Skip:
  `("either_or", "Either-Or", "Reaching the deep band within ~50 ms of the primary
  point fires only the deep stage; otherwise the primary fires (up to 50 ms late) and
  the deep stage is locked out until the key is released.")`. The Quick-Skip tooltip
  text is unaffected by the flush.
- `SetStagingMode` teardown (`edit.rs`, `Effect::StopStage` on any mode change) needs no
  change — flipping into or out of Either-Or strands the shared phase machine exactly
  as Quick-Skip does, and the same reset covers it.
- README "Dual-stage keys" section gains an **Either-Or** bullet; the Quick-Skip bullet
  is fine as written.
- `CONTEXT.md`: **Staging mode** lists four modes, **Quick-Skip** describes the flush,
  new **Either-Or** entry (done alongside this spec).
- `tartarus-dual-stage-keys/spec.md`: a one-line supersession note on the Quick-Skip
  "Up arrives first" row pointing here (done alongside this spec).

## Testing Decisions

Test-first, in the same two layers the existing Quick-Skip coverage uses: pure-core
table tests on `advance` / `tick` in `stage.rs`, and dispatch-level scenarios in
`dispatch.rs` (the `dual_stage_quick_skip_*` family is the template).

- **Changes an existing test**: `dual_stage_quick_skip_early_up_cancels_with_nothing_emitted`
  asserts the bug being fixed; it becomes the flush test (Down emitted, Up emitted 40 ms
  later under paused time).
- Either-Or, pure core: every Late row in the table above; Skipped and Armed rows
  mirror Quick-Skip's.
- Either-Or, dispatch:
  - fast full press → deep only, primary never emitted (Skipped);
  - slow press to full depth → primary only, no deep emission on the way down *or*
    back up; primary released on the real Up;
  - Late with a Hold-to-repeat deep and Hold-to-repeat primary held in the deep band →
    primary keeps repeating, deep never fires (the `deep_repeat` guard);
  - Late, repeated excursions into and out of the deep band → nothing emitted for any
    of them;
  - quick shallow tap → flushed primary, 40 ms dwell.
- Flush edge cases (both modes): re-press inside the 40 ms pending release; `stop_all`
  with a release pending; Toggle primary tapped (loop starts and keeps running);
  Controller button primary tapped (Down, 40 ms, Up on the gamepad device).
- Wire round-trip for `either_or`; GUI stub accepts it; the GUI picker lists it.

## Out of Scope

- A configurable window length (per key, per Profile, or global).
- Any feedback (sound, LED, Toast label) when the deep band is reached while locked out.
- Changing Handoff or No-Return.
