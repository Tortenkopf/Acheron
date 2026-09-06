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

**Status:** ready-for-agent

- [ ] A `RepeatSchedule` resolved **once at daemon startup** — a `spawn_blocking`
      `analog::read_kernel_auto_repeat()` with the `DEFAULT_REPEAT_DELAY_MS` /
      `DEFAULT_REPEAT_PERIOD_MS` fallback (mirror `capture::analog::read_repeat_schedule`;
      or lift that fn to `pub(crate)`). Threaded down exactly like `toggle_lap_target`:
      `main.rs` resolves it → `dispatch::run` parameter → `DispatchState` field →
      `trigger::PerformDeps`. An inline per-press blocking read breaks the
      `tokio::time::pause()` test harness (ticket 68's finding) — do not do that.
- [ ] `TriggerDecision::StartToggleAutorepeat(Modifiers, KeyCode)` (§3.2).
      `decide`'s `(Toggle, Down)` arm: `SustainedNoRepeat(code)` → `StartToggleHeld`
      (unchanged); `AutorepeatKey(mods, code)` → `StartToggleAutorepeat(mods, code)`;
      `None` → `StartToggleLoop` (unchanged, multi-step Macro).
- [ ] `ActiveToggle::spawn_autorepeat(injector, mods, code, schedule)` beside
      `spawn` / `spawn_held`, running `run_toggle_autorepeat` per §5.2: press `mods`
      then `code` (`value=1`, tracked in a loop-private `held`); loop emitting
      `injector.repeat_key(code)` (`value=2`) with the next-due time
      `started + delay_ms + fired*period_ms`, `RepeatSchedule::repeat_due` /
      `advance_fired` governing it (the missed-deadline clamp carries for free); on
      `cancel`, `force_release(held)` → `value=0` for `code` + `mods`. Same
      `{cancel, handle}` shape as the other two, so `stop()` and every caller work
      unchanged.
- [ ] `Slots::perform`'s `D::StartToggleAutorepeat` arm inserts an
      `ActiveToggle::spawn_autorepeat(...)` into `toggles`, threading the
      `RepeatSchedule` from `PerformDeps`. `spawn_autorepeat` stays synchronous.
- [ ] Second-press stop: `handle_event`'s existing "Down on an Input with an active
      Toggle stops it first" path (`stop_toggle`) works unchanged for the new variant.
- [ ] `run_toggle_loop` / `target_lap` / `MIN_TOGGLE_LAP` / `combine_toggle_lap_target`
      / `resolve_toggle_lap_target` all stay — now reached only by
      `D::StartToggleLoop` (multi-step Macro). Add a comment at each saying so.
- [ ] `executor` unit tests (`start_paused`): a `run_toggle_autorepeat` emits
      `value=1`, then the first `value=2` a full `delay_ms` later, then `value=2` at
      `period_ms`, then `value=0` on `stop()`; a stall past several due times emits
      one `value=2` on resume and re-bases (no catch-up burst); modifier-wrapped
      variant holds the modifier `value=1` and releases it with the key.
- [ ] Dispatch integration tests: Toggle → keyboard Keypress and Toggle →
      single-key Macro both produce the identical `value=1` … `value=2` envelope …
      `value=0`-on-second-press stream; Toggle → mouse button still one `value=1` /
      one `value=0`; Layer switch / `StopAllToggles` releases a running autorepeat
      Toggle with `value=0`.
- [ ] **Resolves [humane-output-rate ticket 08](../../humane-output-rate/issues/08-lock-macro-repetition-floor-tests.md)**
      (§9): write its Toggle→Macro regression split here — a Toggle wrapping a
      **single-key** Macro asserts the `value=2` envelope (not a `target_lap`-paced
      `[Down,Up]` loop); a Toggle wrapping a **multi-step** Macro still asserts
      `target_lap` pacing. The Hold-to-repeat→Macro re-fire test (ticket 08 item 2)
      is unaffected — note that the chord-keyed variant rides the same
      `decide`/`perform` seam. Ticket 07 of this effort marks ticket 08 resolved
      with a pointer here.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets`, full daemon suite green.
