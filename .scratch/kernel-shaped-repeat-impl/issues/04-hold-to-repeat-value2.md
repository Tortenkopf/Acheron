# 04 — Hold-to-repeat single key → genuine `value=2` (surfaces 1, 2, 3, deep stage)

**What to build:** The core behaviour change. Every Hold-to-repeat path whose held
target is a single keyboard key stops emitting `[KeyDown, KeyUp]` pairs and instead
presents as real Linux autorepeat: one `value=1` on press, one `value=2` per repeat
the path's existing driver says is due, one `value=0` on release. Modifiers on a
`Ctrl+X`-style binding are held `value=1` alongside and released together; only the
base key autorepeats — exactly what the kernel does for a physically held modified
key.

Converts, all through the shared `decide` + `Slots::perform` seam:
- **Surface 1** Digital-capture Hold-to-repeat — one `value=2` per incoming
  `EventState::Repeat` on the grabbed physical key, cadence unchanged (the real
  kernel envelope, gated 1:1 — no scheduler of ours interposed);
- **Surface 2** Analog-synth Hold-to-repeat on grid keys — same shape;
  `RepeatSchedule` / `repeat_due` / `advance_fired` in `capture/analog.rs` stay
  **untouched** (they decide *when*; only the emit changes);
- **Surface 3** Chord Hold-to-repeat — converts iff the chord's Action is a single
  key; `chord::feed_repeat` already re-fires only the leader member, so one `value=2`
  per leader `Repeat`;
- **Deep stage** — a single-key deep-stage Hold-to-repeat binding rides the same path
  via the deep `Slots<StageKey>` (per the effort decision: let it convert, for
  consistency with a physically held key).

Not converted: Stepper Hold-to-repeat (surface 4 — `Action::Step` ⇒ `HoldKind::None`,
each `Repeat` targets a different item); multi-step Macro (surface 10 — whole macro
re-runs per `Repeat`, unchanged).

Source of truth: [`spec-kernel-shaped-repeat.md`](../../humane-output-rate/spec-kernel-shaped-repeat.md)
§3.1, §3.3, §4 (surfaces 1–4, 10, 11), §5 (the envelope table, surfaces 1 & 2 rows),
§7 (force-release / stop).

**Blocked by:** 01 — Injector `repeat_key` primitive; 03 — `&macros` threaded into
`decide`.

**Status:** done

- [x] `TriggerDecision::RepeatKey(KeyCode)` added (§2.1 / §3.1). `Slots::perform`'s
      `D::RepeatKey(code)` arm calls `deps.injector.repeat_key(code)` — stateless, no
      `firings` / `held` mutation.
- [x] `decide`: `(HoldToRepeat, Repeat)` with `AutorepeatKey(_, code)` →
      `D::RepeatKey(code)` (replaces the old `SpawnFireOnce`). `SustainedNoRepeat`
      still → `D::Nothing`; `None` (multi-step Macro) still → `guarded(D::SpawnFireOnce)`.
- [x] `decide`: `(HoldToRepeat, Down)` with `AutorepeatKey(mods, code)` → a guarded
      decision that presses `mods` (each `value=1`) and `code` (`value=1`) and
      registers all of them in the firing's `held` set. `D::HoldKeyDown` extended to
      `HoldKeyDown(Modifiers, KeyCode)`; `perform` spawns
      `executor::held_key_down_steps(mods, code)` = `vec![KeyDown(m0), …, KeyDown(code)]`.
      The mouse-button / `ControllerButton` `SustainedNoRepeat` arms pass
      `Modifiers::default()` (unchanged output).
- [x] Force-release / stop (§7): `force_release_stuck` drains the whole `held` set, so
      the extra modifier codes it now contains are released with the base key on the
      physical-`Up` / chord-`ReleaseChordFiring` / deep-stage paths — no change there.
      **But** the Layer/Profile-switch / capture-flip / disconnect paths were *not* wired
      to release individual firings (only the deep `Slots<StageKey>`) — see review
      follow-up 1 below: new `Slots::drain_firings` + wiring. The `value=2` stream needs
      no teardown. Locked with
      `trigger::slots::perform_hold_key_down_with_modifiers_presses_and_force_releases_the_whole_chord`,
      `dispatch::…dual_stage_layer_switch_mid_press_force_releases_a_live_deep_hold_to_repeat`,
      `layer_switch_while_holding_a_hold_to_repeat_key_force_releases_it`,
      `profile_switch_while_holding_a_hold_to_repeat_key_force_releases_it`.
- [x] **Required ordering test** (§7):
      `injector::tests::a_queued_repeat_key_never_lands_after_the_force_release_that_follows_it`.
- [x] Dispatch integration tests:
  - `hold_to_repeat_keyboard_key_emits_genuine_autorepeat_not_down_up_pairs` (surface 1);
  - `analog_synth_grid_hold_to_repeat_emits_the_same_autorepeat_shape` (surface 2 — analog-sourced);
  - `hold_to_repeat_modifier_wrapped_key_holds_the_modifier_and_autorepeats_only_the_base`;
  - `single_key_chord_hold_to_repeat_emits_one_autorepeat_per_leader_repeat` (surface 3);
  - `dual_stage_handoff_deep_stage_hold_to_repeat_actually_repeats` (deep stage — value=2
    stream + `ReleaseDeep` teardown) and
    `dual_stage_layer_switch_mid_press_force_releases_a_live_deep_hold_to_repeat` (`stop_all`);
  - `hold_to_repeat_step_binding_advances_the_cursor_on_every_repeat` — unchanged, still green.
- [x] `cargo fmt --check`, `cargo clippy --all-targets`, full daemon suite green (498 passed).

## Comments

### Implemented 2026-09-06

`D::HoldKeyDown` was **extended** to `HoldKeyDown(Modifiers, KeyCode)` rather than adding a
sibling variant — `perform` keeps one arm (`executor::held_key_down_steps`), and the
mouse/gamepad `SustainedNoRepeat` callers pass `Modifiers::default()` for byte-identical
output. `decide`'s `hold_key: Option<KeyCode>` local became `hold_kind: Option<HoldKind>`
so the `HoldToRepeat` / `Toggle` arms see the full classification; `FireOnce` /
`AnalogRepeat` still skip `hold_repeat_kind` entirely.

Toggle is untouched this ticket — `(Toggle, Down)` with `AutorepeatKey` still →
`D::StartToggleLoop` (ticket 05 splits it).

`RepeatKey` is **not** overlap-guarded (spec §3.1) — the `Down`'s `HoldKeyDown` firing is a
delay-free unbalanced-`KeyDown` spawn that completes near-instantly, and a stray `value=2`
while it is somehow still unfinished is harmless (channel order preserved).

Existing tests updated for the behaviour change: `trigger::tests::decision_table`,
`overlap_guard_only_blocks_on_an_unfinished_firing`,
`single_mouse_button_macro_takes_the_sustained_hold_arms_like_the_equivalent_keypress`,
`dispatch::…bound_mode_key_role_routes_the_mode_key_through_full_trigger_mode_dispatch`,
`dual_stage_additive_holds_both_stages_on_independent_untouched_cadences`.

### /code-review follow-ups (2026-09-06)

Four findings; two fixed, two accepted.

1. **FIXED — a keyboard `HoldKeyDown` hold could be stranded at the OS level across a
   Layer/Profile switch, capture-mode flip, or disconnect.** Pre-ticket-04 the individual
   keyboard path emitted balanced `[Down,Up]` pairs and nothing could stick; ticket 04
   widened the pre-existing mouse/`ControllerButton` `HoldKeyDown` exposure to every
   single-key Hold-to-repeat. Spec §7's table lists these teardown paths as balancing the
   held `value=1` via `Slots::stop_all`, but that was only wired for the deep
   `Slots<StageKey>` — `self.individual`'s firings were only ever released on the bound
   Input's own physical `Up` (`decide`'s `Up` arm), which is inert if the key is
   unbound / Toggle-bound on the destination. Fix: new `Slots::drain_firings` (force-release
   **and remove** every firing, leave Toggles running), wired into `handle_layer_switch`,
   `handle_capture_mode_change` (Digital flip), `handle_connection_change` (disconnect), and
   `SwitchProfile` (new `Effect::ReleaseAllHolds`). Tests:
   `layer_switch_while_holding_a_hold_to_repeat_key_force_releases_it`,
   `profile_switch_while_holding_a_hold_to_repeat_key_force_releases_it`.
2. **FIXED — a `Repeat` with no established hold emitted a dangling `value=2`.** A key held
   into a Layer/Profile that freshly binds it, held across daemon startup, or whose firing
   was just drained by fix 1 would `decide` → `RepeatKey` with nothing pressed. `decide`'s
   `(HoldToRepeat, Repeat)` `AutorepeatKey` arm now emits `RepeatKey` only when `slot`
   shows this key's firing (live or just-finished); otherwise it re-presses via
   `guarded(HoldKeyDown(..))`. Test:
   `a_hold_to_repeat_key_held_across_a_layer_switch_re_presses_on_the_new_layer`.
3. **ACCEPTED — `hold_repeat_kind` recompiles an `Action::Macro`'s steps on every `Repeat`
   tick.** Pre-existing since ticket 02/03 and explicitly acknowledged in ticket 03's own
   review notes ("inherent to the pure `decide`/`perform` seam"). Only affects `Action::Macro`
   bindings; a single-key macro now takes the `RepeatKey` arm (no second `compile_action`).
   Not a ticket 04 regression.
4. **ACCEPTED — `HoldKeyDown`'s `value=1` is emitted from a spawned task while `RepeatKey`'s
   `value=2` is awaited inline, so a `Repeat` on the very next event could in principle
   interleave ahead of the initial press.** The window is sub-millisecond right after
   `Down`; every real and synthesized repeat source has a ≥ ~250 ms initial delay
   (`REP_DELAY` / `RepeatSchedule::delay_ms` / leader kernel autorepeat), the dispatch task
   polls the spawned press at its next `await` (immediately), and the end state is correct
   (key down). Matches the existing `HoldKeyDown` architecture (tickets 75/76/79/80) and
   spec §7's "send order through the one mpsc channel already guarantees this". Left as-is.
