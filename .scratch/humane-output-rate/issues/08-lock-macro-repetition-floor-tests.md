# Lock the trigger-driven Macro-repetition floor with regression tests

Type: task
Status: resolved
Blocked by: 01, 07
Parent: [Humane output rate](../map.md)

> Execution ticket — resolved on this map, not handed off (map Notes: "execution is in
> scope"). Graduated from ticket 03's decision 1. **Test-only — no production logic
> change, no new constant.**
>
> **Rewired `Blocked by: 01, 07` (2026-09-06).** Ticket 07's spec changes what a Toggle
> wrapping a *single-key* Macro does: it now rides the `value=2` held-key path, **not** the
> `run_toggle_loop` / `target_lap` loop. So item 1 below must split — see the notes inline.
> Item 2 (Hold-to-repeat → multi-step Macro) is unaffected. Do this ticket *after* ticket
> 07's implementation effort lands, or the item-1 test will be written against
> soon-to-be-dead behaviour.

## Question

Ticket 03 established that the *inter-run* cadence of a Trigger mode repeating a whole
Macro is **already** floored to `max(live kernel period, MIN_TOGGLE_LAP 20 ms)` on every
path — `executor::run_toggle_loop`'s `target_lap` for Toggle → Macro, and kernel-`Repeat`
arrival plus the `Slot::FiringUnfinished` overlap guard in `trigger::decide` for
Hold-to-repeat → Macro. "Generalise `MIN_TOGGLE_LAP`" therefore adds no floor that isn't
already present. The risk is a future refactor silently removing that guarantee.

Lock it:

1. **Regression test — Toggle → Macro.** Split by ticket 07's spec:
   - **Multi-step Macro** (≥ 2 distinct keys, or a `Delay` between key steps): a Toggle
     wrapping it paces at `target_lap`, not as fast as the injector drains. This is the case
     `run_toggle_loop` + `target_lap` still own. (Extends the existing `run_toggle_loop` /
     `MIN_TOGGLE_LAP` tests in `executor.rs` if none covers a Macro-shaped step vec
     explicitly.)
   - **Single-key Macro** (`single_held_key` returns `Some` — `[KeyDown(k), KeyUp(k)]`,
     optionally modifier-wrapped, optionally one trailing `Delay`): a Toggle wrapping it now
     rides ticket 07's `value=2` held-key path — assert `value=1` … `value=2` at the kernel
     envelope … `value=0`, **not** a `target_lap`-paced `[Down,Up]` loop. Guards against a
     refactor accidentally routing it back through `run_toggle_loop`.

2. **Regression test — Hold-to-repeat → Macro.** Assert that a mid-run kernel `Repeat` is
   dropped by the `FiringUnfinished` overlap guard (so a fast Macro cannot re-fire faster
   than kernel `Repeat` arrival), and that a Macro run longer than one `REP_PERIOD`
   re-fires no faster than one run per completion. Locate the right seam — `trigger`'s
   `decide` truth-table tests already exercise `SpawnFireOnce` under
   `Some(Slot::FiringUnfinished)`; this may only need an assertion sharpening + a comment.
   Note the **chord-keyed** variant (`FireChord{state: Repeat}` → same `decide`/`perform`,
   keyed by `ChordKey`) rides the identical seam — either add a chord-keyed assertion or a
   comment recording that coverage is transitive. Applies only to a *multi-step* Macro
   Action; a single-key chord Action rides ticket 07's `value=2` path.

3. **Comments.** In `run_toggle_loop` (at the `target_lap` floor) and at the overlap-guard
   arm in `trigger::decide` / `Slots::perform`, a line naming ticket 03's decision 1: this
   is the load-bearing mechanism for the humane-output-rate invariant on trigger-driven
   Macro *repetition*; a Macro fired once and the keystroke cadence within one run are
   deliberately unrestricted (ticket 03 decision 2).

Feeds ticket 04 (the ADR cites the mechanism, not a new constant).

## Answer

Resolved 2026-09-06 by the `value=2` implementation effort
[`.scratch/kernel-shaped-repeat-impl/`](../../kernel-shaped-repeat-impl/issues/). Its
[ticket 05](../../kernel-shaped-repeat-impl/issues/05-toggle-sustained-autorepeat.md)
owns the Toggle→Macro regression split this ticket called for; ticket 07 of that effort
(un-gate ADR-0008) closes this one out. Test-only work — no production logic changed, no
new constant, matching this ticket's own scope.

**Item 1 — Toggle → Macro split** (done, `kernel-shaped-repeat-impl` ticket 05):

- Single-key Macro rides ticket 07's `value=2` held-key path:
  `dispatch::toggle_single_key_macro_holds_the_same_autorepeat_as_the_equivalent_keypress`
  asserts the `value=1` … `value=2`-envelope … `value=0` stream, **not** a
  `target_lap`-paced `[Down,Up]` loop.
- Multi-step Macro still paces at `target_lap`:
  `dispatch::toggle_multi_step_macro_still_loops_at_target_lap` (the sole remaining
  `run_toggle_loop` / `D::StartToggleLoop` user, surface 9).
- `trigger::decision_table` locks that a single-key Macro's `decide` output matches the
  equivalent Keypress arm for arm, on every `(mode, state)` — so a refactor cannot
  silently route it back through `run_toggle_loop`.

**Item 2 — Hold-to-repeat → multi-step Macro re-fire** (already covered, rides the same
`decide` / `Slots::perform` seam):

- `dispatch::overlapping_same_input_firings_are_dropped_not_queued` — a mid-run kernel
  `Repeat` on a Hold-to-repeat Macro binding is dropped by the `Slot::FiringUnfinished`
  overlap guard; a `Repeat` after the run completes starts a genuinely new firing. So a
  fast Macro cannot re-fire faster than one run per completion / per kernel `Repeat`
  arrival.
- `trigger::decision_table` exercises `None => guarded(D::SpawnFireOnce)` against every
  slot state, `FiringUnfinished` → `D::Nothing` included.
- The **chord-keyed** variant (`FireChord{state: Repeat}`) routes through the identical
  `decide` / `perform` seam keyed by `ChordKey` — coverage is transitive (spec §3.3).
- A **single-key** chord/Hold-to-repeat Action instead rides the `value=2` path
  (`kernel-shaped-repeat-impl` ticket 04:
  `single_key_chord_hold_to_repeat_emits_one_autorepeat_per_leader_repeat`).

**Item 3 — comments**: the load-bearing floor is documented in
`daemon/src/executor.rs` at `MIN_TOGGLE_LAP` / `combine_toggle_lap_target` /
`resolve_toggle_lap_target` / `run_toggle_loop` (the ticket-26 freeze story, why the
floor is kept, and — post-`value=2` — that only the multi-step Macro Toggle is paced this
way), and in `trigger::decide` at the overlap-guard closure and the `(HoldToRepeat,
Repeat)` `None` arm. A Macro fired once, and the keystroke cadence within one run, stay
deliberately unrestricted (ticket 03 decision 2 / ADR-0008 "The Macro exception").

Full daemon suite green (516 passed) as of `kernel-shaped-repeat-impl` ticket 06.
