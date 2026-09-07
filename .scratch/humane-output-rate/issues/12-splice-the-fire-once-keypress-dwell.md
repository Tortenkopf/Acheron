# Splice the Fire-once keypress dwell

Type: task
Status: resolved
Blocked by: 10
Parent: [Humane output rate](../map.md)

> Execution ticket — resolved on this map, not handed off (map Notes: "execution is in
> scope"). Graduated from [ticket 10](10-fire-once-keyboard-keystroke-dwell.md)'s decision.
> A **rate-plausibility consistency follow-up** to the `value=2` rebuild
> ([`kernel-shaped-repeat-impl/`](../../kernel-shaped-repeat-impl/issues/)), not a detection
> defense: a canned one-shot keyboard press today emits `[KeyDown, KeyUp]` at ~0 ms dwell —
> the exact shape ADR-0008 calls "the clearest synthetic tell," erased everywhere else the
> audit flagged it.

## Question

Give a **canned one-shot keyboard press** a fixed dwell between its Down and Up, so it stops
emitting the near-zero-dwell `[KeyDown, KeyUp]` pair. All parameters are settled by
ticket 10 — this ticket is the implementation.

1. **New constant `FIRE_ONCE_KEY_DWELL = Duration::from_millis(40)`** (name at your
   discretion), in `daemon/src/executor.rs` near `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`.
   Its doc comment must state it is **deliberately not shared** with
   `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD` — that constant targets single-poll-frame
   coverage (35 ms ≈ 2 frames at 60 fps); this one targets a human-plausible keystroke
   dwell. 40 ms rationale: ~2.5 frames at 60 Hz, clears the CS2 "0 ms overlap/neutral"
   shape, inside the ~30–500 ms keystroke-dynamics typing band
   ([`docs/anti-cheat-input-heuristics.md`](../../../docs/anti-cheat-input-heuristics.md)),
   and ~2× headroom under the ~80 ms human same-key double-tap floor so `decide`'s
   `FiringUnfinished` overlap guard never drops a real user's second press.

2. **Splice the dwell where `executor::single_held_key` matches, under Fire-once only.**
   The compiled step sequence becomes
   `[KeyDown(mods…), KeyDown(key), Delay(FIRE_ONCE_KEY_DWELL), KeyUp(key), KeyUp(mods…)]`.
   `single_held_key` is the same bright-line predicate the `value=2` work uses, so this
   covers — with no new content inspection:
   - Fire-once `Action::Keypress`, plain **and** modifier-wrapped;
   - Fire-once **single-key** `Macro` (`[KeyDown, KeyUp]`, no `Delay`);
   - Fire-once Stepper `Key` step;
   - Fire-once Chord whose Action is a single key.

   And leaves untouched:
   - Fire-once **multi-step** `Macro` — `single_held_key` returns `None`; the Macro
     exception holds by shape;
   - **Analog-repeat**, including the Digital-Capture-mode fallback (`(AnalogRepeat, Down |
     Repeat)` in `trigger::decide`) — a fixed 40 ms dwell would break its 20 Hz (50 ms
     period) fast end, and it is an already-audited tap stream;
   - `Action::ControllerButton` (locked out of Fire-once by ticket 78, already carries its
     own 35 ms).

3. **Mechanism — constraints binding, variant your call.** Binding: (a) must not affect the
   Analog-repeat path; (b) reuse `single_held_key`; (c) Fire-once only.
   `trigger::decide` already has `(FireOnce, Down)` and `(AnalogRepeat, Down | Repeat)` as
   separate match arms that merely both return `guarded(D::SpawnFireOnce)` — splitting the
   Fire-once arm to carry the classified `(Modifiers, KeyCode)` (mirroring `D::HoldKeyDown`
   / `D::RepeatKey`) keeps `perform` mechanical and is the *preferred* shape, but a
   `perform`-local branch that runs `single_held_key` on the compiled steps is acceptable
   if it reads cleaner against the real code. Whichever: the AnalogRepeat arm must be
   provably unaffected (assert it in a `decide` table test).

4. **`single_held_key` interaction check.** Confirm that splicing a `Delay` *between* the
   key edges does not feed back into any `value=2` classification path — `single_held_key`
   rejects a `Delay` between edges, so a re-classification of the *spliced* output returns
   `None` (correct). Verify the classifier is only ever run on the *pre-splice* steps.

5. **Docs — amend ADR-0008, no new ADR.**
   - **`docs/adr/0008-physical-plausibility-ceiling-for-synthetic-output.md`, "The ceiling"
     paragraph** — add:
     > A canned one-shot keyboard press — a Fire-once Keypress, and its single-key Macro /
     > Stepper-step / Chord equivalents — carries a fixed 40 ms dwell between Down and Up
     > (`FIRE_ONCE_KEY_DWELL`), rather than the near-zero-dwell pair the research names the
     > clearest synthetic tell. This is a rate-plausibility consistency follow-up, not a
     > detection defense: a Macro fired once still keeps its author's cadence, and
     > Analog-repeat's tap pulses are unchanged.
   - **`CONTEXT.md` "Physical-plausibility ceiling"** — after the `value=1`/`value=2`
     sentence:
     > A canned one-shot keyboard press carries a fixed ~40 ms Down→Up dwell rather than a
     > zero-dwell pair.
   - **`CONTEXT.md` "Fire-once"** — leave as-is.
   - Add the effort/ticket ref (`ticket 12`) the way ticket 09 did.

6. **Tests.**
   - `decide` table: `(FireOnce, Down)` on a single-key Binding produces the new
     dwelled decision / marker; `(AnalogRepeat, Down)` and `(AnalogRepeat, Repeat)` are
     **unchanged**; a multi-step-Macro Fire-once Binding still produces the plain
     `D::SpawnFireOnce`.
   - executor / `perform`: the spliced sequence for a plain key, a modifier-wrapped key, a
     single-key Macro, a Stepper `Key` step, and a single-key Chord all carry exactly one
     `Delay(FIRE_ONCE_KEY_DWELL)` between the key edges; a multi-step Macro carries none.
   - Regression: a Fire-once press still force-releases cleanly on the physical `Up`
     (`force_release_stuck`), and back-to-back Fire-once presses > 40 ms apart both fire.
   - Full suite green (daemon + GUI + clippy), as ticket 09 did.

7. **GUI.** No change expected — this is a Daemon-internal timing splice with no config
   surface, no new `ConfigError`, no `rules.py` mirror. Confirm and note it.

## Answer

Implemented on `dev` (2026-09-07). All parameters were settled by ticket 10; this records
the mechanism chosen and the surprises.

### The constant

`executor::FIRE_ONCE_KEY_DWELL = Duration::from_millis(40)`, in `daemon/src/executor.rs`
right below `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`. Its doc comment states it is
**deliberately not shared** with that constant (35 ms → single-poll-frame coverage on a
receiving game; 40 ms → a human-plausible keystroke dwell) and that this is a
rate-plausibility *consistency* follow-up, not a detection defense.

### Mechanism — `perform`-local branch, not a `decide` split

Ticket 10 named the `decide`-arm split as *preferred*. I went with the **`perform`-local
branch** instead, because the `decide` split cannot cover the Fire-once **Stepper `Key`
step**: `decide` deliberately stays abstract for `D::SpawnFireOnce` (no compiled steps —
so a dropped firing never advances a Stepper cursor), and `hold_repeat_kind` returns
`None` for `Action::Step`. A `perform`-local branch runs `executor::single_held_key` on
the *already-compiled* steps (inside `Slots::perform`'s `D::SpawnFireOnce` arm, after
`compile_action`), so it covers all four in-scope shapes uniformly:

```rust
if deps.fire_once_key_dwell
    && binding.trigger == TriggerMode::FireOnce
    && let Some((mods, code)) = executor::single_held_key(&steps)
{
    steps = executor::fire_once_key_steps(mods, code);
}
```

`decide` is **entirely untouched** — a Fire-once single key still resolves to the plain
`D::SpawnFireOnce`, and the `(FireOnce, Down) | (AnalogRepeat, Down | Repeat)` arm is
unchanged, so the Analog-repeat path (including the Digital-Capture fallback that shares
the arm) is provably unaffected. Locked by
`trigger::tests::decide_is_unchanged_by_the_fire_once_dwell_splice`.

New helper `executor::fire_once_key_steps(mods, key)` = `keypress_steps` with one
`Delay(FIRE_ONCE_KEY_DWELL)` between the base key's edges.

### `single_held_key` re-classification (point 4) — safe

The spliced sequence has a `Delay` *between* the key edges, so `single_held_key` rejects
it (`None`). The classifier only ever runs on the *pre-splice* steps
(`hold_repeat_kind` in `decide` re-compiles from the Action; `perform`'s branch runs it
before the swap). Locked by
`executor::tests::fire_once_key_steps_output_is_not_re_classified_as_a_single_held_key`.

### The one design decision ticket 10/12 did not anticipate: the `stage::Engine` path

`Slots::perform` is shared by the individual path, the Chord path, **and** the
`stage::Engine` (dual-stage) path. A dual-stage primary is a plain Fire-once single key,
so the dwell would apply to a `RepressPrimary` / deep fire too — and because a dwelled
firing stays `FiringUnfinished` for 40 ms, `decide`'s ordinary same-key overlap guard
would then **drop a `RepressPrimary`** on a fast (< 40 ms) deep-band wiggle. A
depth-driven re-press is machine-sequenced, not a canned press the user made once, so it
is out of ticket 10's stated scope ("a canned `[KeyDown, KeyUp]` the user has no timing
control over").

Resolved with a **`fire_once_key_dwell: bool` on `trigger::PerformDeps`** (threaded
through `PerformDeps::new`): `true` at the 3 dispatch call sites (individual `Down`,
individual `Repeat`/`Up`, Chord `FireChord`), `false` at the 5 `stage::Engine` call sites
(FirePrimary/RepressPrimary, FireDeep ×2, retroactive RepressPrimary, deep repeat). So
the dwell lands only on a user-initiated one-shot.

### Redundant trailing `value=0` on a sub-dwell physical tap — accepted

If the physical key is released (or Handoff hands off) *inside* the 40 ms dwell,
`ForceReleaseStuck` / `ReleasePrimary` drains the firing's `held` and force-releases the
key, then the dwell task later emits its own `KeyUp` — a redundant `value=0` for an
already-released key. The kernel deduplicates a redundant key-up, and the shape is
identical to the pre-existing `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD` firing on a
sub-35 ms Analog-repeat `Up`. Not engineered around here (ticket 12: "reuse ticket 33's
force-release path rather than inventing new architecture" — a cancel token on
`FiringHandle` would also truncate a running multi-step Fire-once Macro on physical
release, which is not wanted).

**Follow-up (2026-09-07):** the clean fix — a dual-stage key's primary press is
machine-sequenced, so it should not carry the dwell at all — needs the "is this a
dual-stage key?" predicate that `post-release-development` **ticket 17**
(`fold-stage-routing-behind-engine-feed`) centralises in `stage::Engine::feed`.
Folded into ticket 17's scope; see that ticket's Addendum. Until then this residual
stands as described above (benign — kernel-deduplicated).

### Docs

- **ADR-0008** "The ceiling" — the ticket-12 sentence added verbatim, plus
  `FIRE_ONCE_KEY_DWELL` appended to the "enforced by existing mechanism" list.
- **`CONTEXT.md` "Physical-plausibility ceiling"** — the ~40 ms Down→Up dwell sentence
  added after the `value=1`/`value=2` sentence. "Fire-once" entry left as-is.
- No new ADR.

### Tests (all names new unless noted)

- `executor`: `fire_once_key_steps_splices_one_dwell_between_the_edges`,
  `fire_once_key_steps_output_is_not_re_classified_as_a_single_held_key`,
  `fire_once_key_dwell_actually_elapses_before_the_up_write` (start_paused — the dwell is
  a genuine blocking sleep).
- `trigger`: `decide_is_unchanged_by_the_fire_once_dwell_splice`,
  `perform_fire_once_single_key_holds_the_key_for_the_spliced_dwell`,
  `perform_fire_once_single_key_skips_the_dwell_when_disabled` (the `stage` path),
  `perform_fire_once_multi_step_macro_carries_no_dwell`.
- `dispatch`: `fire_once_keypress_holds_the_dwell_then_back_to_back_presses_both_fire`
  (Seam, start_paused — the regression case: force-release clean on `Up`, two presses
  > 40 ms apart both fire, no stray events).
- Updated: `fire_once_step_binding_produces_no_extra_output_on_physical_release` and
  `set_stepper_items_clamps_a_cursor_left_stranded_by_a_shrink` now hold / space their
  presses past the dwell (they pressed faster than any hand); the two dual-stage
  Handoff/No-Return walk tests gained a `settle_past_dwell()` helper so the primary
  firing clears before the deep excursion.

### Suite

- `cargo test` (daemon): **525 passed**, 0 failed (was 517 pre-change; +8 net new tests).
- `cargo clippy --all-targets`: clean.
- `cargo fmt --check`: clean for every file this ticket touched. (Pre-existing, **not
  from this ticket**: `daemon/src/config/binding.rs:189` is fmt-dirty on `dev` — it was
  committed unformatted by ticket 09's `afed7ce`. Worth a separate sweep before `main`
  is rebuilt.)
- GUI `pytest`: **503 passed**. No GUI change — Daemon-internal timing splice, no config
  surface, no new `ConfigError`, no `rules.py` mirror (point 7 confirmed).

### Files

`daemon/src/executor.rs`, `daemon/src/trigger.rs`, `daemon/src/stage.rs`,
`daemon/src/dispatch.rs`, `docs/adr/0008-physical-plausibility-ceiling-for-synthetic-output.md`,
`CONTEXT.md`.
