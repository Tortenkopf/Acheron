# 05 — Toggle → single key / single-key Macro → sustained autorepeat (surfaces 6, 6b)

**What to build:** A Toggle whose held target is a single keyboard key — a plain
`Keypress` **or** a single-key `Macro` (identical compiled steps ⇒ identical
behaviour) — stops looping `[Down, Up]` through `run_toggle_loop` at `target_lap` and
instead holds a genuine autorepeat: `value=1` on the first press, `value=2` at the
**full** `REP_DELAY`→`REP_PERIOD` kernel envelope (a Toggle-held key *is* a held key
— it should look exactly like one), `value=0` on the second press and on any stop
(Layer/Profile switch, Analog→Digital flip, GUI-focus `StopAllToggles`).

`run_toggle_loop` + `target_lap` + `MIN_TOGGLE_LAP` + `combine_toggle_lap_target`
survive **only** for the multi-step Macro Toggle (surface 9) — the sole remaining
`D::StartToggleLoop` user. Toggle → mouse / gamepad button (surfaces 7/8) is
untouched (`BTN_*` never autorepeats).

Source of truth: [`spec-kernel-shaped-repeat.md`](../../humane-output-rate/spec-kernel-shaped-repeat.md)
§3.2, §4 (surfaces 6, 6b, 7, 8, 9), §5.2 (`run_toggle_autorepeat` pseudocode), §5.4
(missed-deadline clamp — Toggle reuses `RepeatSchedule::advance_fired` as-is), §7.

**Blocked by:** 01 — Injector `repeat_key` primitive; 03 — `&macros` threaded into
`decide`.

**Status:** done

- [x] A `RepeatSchedule` resolved **once at daemon startup** — a `spawn_blocking`
      `analog::read_kernel_auto_repeat()` with the `DEFAULT_REPEAT_DELAY_MS` /
      `DEFAULT_REPEAT_PERIOD_MS` fallback (mirror `capture::analog::read_repeat_schedule`;
      or lift that fn to `pub(crate)`). Threaded down exactly like `toggle_lap_target`:
      `main.rs` resolves it → `dispatch::run` parameter → `DispatchState` field →
      `trigger::PerformDeps`. An inline per-press blocking read breaks the
      `tokio::time::pause()` test harness (ticket 68's finding) — do not do that.
- [x] `TriggerDecision::StartToggleAutorepeat(Modifiers, KeyCode)` (§3.2).
      `decide`'s `(Toggle, Down)` arm: `SustainedNoRepeat(code)` → `StartToggleHeld`
      (unchanged); `AutorepeatKey(mods, code)` → `StartToggleAutorepeat(mods, code)`;
      `None` → `StartToggleLoop` (unchanged, multi-step Macro).
- [x] `ActiveToggle::spawn_autorepeat(injector, mods, code, schedule)` beside
      `spawn` / `spawn_held`, running `run_toggle_autorepeat` per §5.2: press `mods`
      then `code` (`value=1`, tracked in a loop-private `held`); loop emitting
      `injector.repeat_key(code)` (`value=2`) with the next-due time
      `started + delay_ms + fired*period_ms`, `RepeatSchedule::repeat_due` /
      `advance_fired` governing it (the missed-deadline clamp carries for free); on
      `cancel`, `force_release(held)` → `value=0` for `code` + `mods`. Same
      `{cancel, handle}` shape as the other two, so `stop()` and every caller work
      unchanged.
- [x] `Slots::perform`'s `D::StartToggleAutorepeat` arm inserts an
      `ActiveToggle::spawn_autorepeat(...)` into `toggles`, threading the
      `RepeatSchedule` from `PerformDeps`. `spawn_autorepeat` stays synchronous.
- [x] Second-press stop: `handle_event`'s existing "Down on an Input with an active
      Toggle stops it first" path (`stop_toggle`) works unchanged for the new variant.
- [x] `run_toggle_loop` / `target_lap` / `MIN_TOGGLE_LAP` / `combine_toggle_lap_target`
      / `resolve_toggle_lap_target` all stay — now reached only by
      `D::StartToggleLoop` (multi-step Macro). Add a comment at each saying so.
- [x] `executor` unit tests (`start_paused`): a `run_toggle_autorepeat` emits
      `value=1`, then the first `value=2` a full `delay_ms` later, then `value=2` at
      `period_ms`, then `value=0` on `stop()`; a stall past several due times emits
      one `value=2` on resume and re-bases (no catch-up burst); modifier-wrapped
      variant holds the modifier `value=1` and releases it with the key.
- [x] Dispatch integration tests: Toggle → keyboard Keypress and Toggle →
      single-key Macro both produce the identical `value=1` … `value=2` envelope …
      `value=0`-on-second-press stream; Toggle → mouse button still one `value=1` /
      one `value=0`; Layer switch / `StopAllToggles` releases a running autorepeat
      Toggle with `value=0`.
- [x] **Resolves [humane-output-rate ticket 08](../../humane-output-rate/issues/08-lock-macro-repetition-floor-tests.md)**
      (§9): write its Toggle→Macro regression split here — a Toggle wrapping a
      **single-key** Macro asserts the `value=2` envelope (not a `target_lap`-paced
      `[Down,Up]` loop); a Toggle wrapping a **multi-step** Macro still asserts
      `target_lap` pacing. The Hold-to-repeat→Macro re-fire test (ticket 08 item 2)
      is unaffected — note that the chord-keyed variant rides the same
      `decide`/`perform` seam. Ticket 07 of this effort marks ticket 08 resolved
      with a pointer here.
- [x] `cargo fmt --check`, `cargo clippy --all-targets`, full daemon suite green.

## Comments

### Implemented 2026-09-06

**Startup resolution.** `capture::analog::resolve_toggle_autorepeat_schedule()` (new,
`pub async`) — a `spawn_blocking` wrapper around the existing private
`read_repeat_schedule` (kept private; the wrapper made lifting it unnecessary), with
the `DEFAULT_REPEAT_DELAY_MS` / `DEFAULT_REPEAT_PERIOD_MS` fallback on a
`spawn_blocking` join error. `main.rs` resolves it right after
`resolve_toggle_lap_target` and threads it: `dispatch::run` param →
`DispatchState::toggle_autorepeat_schedule` → `trigger::PerformDeps` /
`stage::EngineDeps` → `ActiveToggle::spawn_autorepeat`. `RepeatSchedule` is already
`Copy`, so it threads as a plain value exactly like `toggle_lap_target`.

**`RepeatSchedule::due_offset(fired) -> Duration`** (new, pure, unit-tested) —
`delay_ms + fired*period_ms`, the sleep target `run_toggle_autorepeat` waits to before
consulting the existing `repeat_due` / `advance_fired`. The grid loop doesn't need it
(it's woken by the report stream); the self-driven Toggle emitter does. §5.4's
missed-deadline clamp carries for free — `advance_fired` re-bases `fired` from real
elapsed time, so after a stall exactly one `value=2` fires and the next is a full
period out (locked by `toggle_autorepeat_does_not_burst_after_a_long_stall`).

**`run_toggle_autorepeat`** presses `held_key_down_steps(mods, key)` (reused from
ticket 04 — each modifier then the base key, all `value=1`, tracked in a loop-private
`held`), then `tokio::time::sleep_until(started + due_offset(fired))` /
`injector.repeat_key(key)` per tick, and `force_release(held)` on `cancel`. Same
`{cancel, handle}` shape as `spawn` / `spawn_held`, so `stop_toggle` /
`stop_all_toggles` / the second-press path drive teardown unchanged.

**`stage::fire` refactor.** Adding the schedule pushed `stage::fire` to 8 args
(clippy `too_many_arguments`). Collapsed it to take a pre-built `trigger::PerformDeps`
instead of `injector` + `config` + `cursors` + the two pacing values (4 args total);
it reads `deps.macros` for `decide`. The four call sites build `PerformDeps::new(...)`
inline.

**Ticket 08 split** (`humane-output-rate`) landed as two dispatch tests:
`toggle_single_key_macro_holds_the_same_autorepeat_as_the_equivalent_keypress` (rides
the `value=2` path) and `toggle_multi_step_macro_still_loops_at_target_lap` (surface 9,
`run_toggle_loop` unchanged). Ticket 07 of this effort marks ticket 08 resolved.
The Layer-switch case in the last checklist item: an individual Toggle deliberately
*survives* a Layer switch (`handle_layer_switch` leaves `individual` toggles running —
spec.md), so the teardown assertion is against `StopAllToggles`
(`stop_all_toggles_releases_a_running_single_key_autorepeat_toggle`), which is also the
Profile-switch `Effect::StopAllToggles` path.

Full daemon suite: 508 passed. `cargo fmt --check` clean, `cargo clippy --all-targets`
clean.

### /code-review follow-ups (2026-09-06)

Two-axis review (Standards + Spec). No correctness bugs. Spec axis confirmed §5.2
fidelity (`due_offset` + `sleep_until` is a faithful, drift-resistant equivalent of the
pseudocode's `next_due_after` + `sleep`), `advance_fired` reused as-is per §5.4, and
every threading/routing/teardown point matches.

- **ADDRESSED — Layer-switch teardown coverage / spec-vs-code note.** Spec §3.2/§7
  list "Layer / Profile switch" as a stop path, but `handle_layer_switch` deliberately
  leaves *individual* Toggles running (spec.md's "Toggle behavior across Layer/Profile
  switches"; only `drain_firings` runs). Added
  `single_key_autorepeat_toggle_survives_a_layer_switch` asserting the real behaviour —
  the autorepeat Toggle keeps emitting `value=2` across the switch and is still torn
  down cleanly later. The §7 "Layer/Profile switch" teardown row is really the
  Profile-switch `Effect::StopAllToggles` path, covered by
  `stop_all_toggles_releases_a_running_single_key_autorepeat_toggle`. Ticket 07 should
  tighten the spec wording (individual Toggle survives a *Layer* switch; only a
  *Profile* switch / GUI `StopAllToggles` stops it).
- **ACCEPTED — `toggle_lap_target` + `toggle_autorepeat_schedule` now travel as a
  pair** (Standards: possible Data Clump). They are alternatives — `lap_target` only
  reaches `StartToggleLoop`, the schedule only `StartToggleAutorepeat` — consumed at
  disjoint sites, so a bundle would ride half-unused everywhere. Mirrors the existing
  solo `toggle_lap_target` threading exactly. Left as two fields.
- **ACCEPTED — `PerformDeps::new(...)` literal repeated in `stage.rs`** (Standards:
  Duplicated Code, pre-existing). The `stage::fire` refactor already cut it from a
  7-arg helper call to a 4-arg one; the remaining repetition is `PerformDeps`'s normal
  constructor at each `Engine` method's disjoint borrow site.
- **ADDRESSED (nit) — comments.** Added ticket-05 notes to `combine_toggle_lap_target`
  and `resolve_toggle_lap_target` (the collective block above `MIN_TOGGLE_LAP` already
  named all five names).
- **NOT CHANGED — Toggle→mouse-button assertion.** Existing
  `toggle_mouse_button_holds_a_single_keydown_and_the_same_key_stops_it` already
  asserts exactly one `value=1` / one `value=0` for a `BTN_LEFT` Toggle; still green.

Full daemon suite: 509 passed.
