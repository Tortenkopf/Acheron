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

**Status:** ready-for-agent

- [ ] `TriggerDecision::RepeatKey(KeyCode)` added (§2.1 / §3.1). `Slots::perform`'s
      `D::RepeatKey(code)` arm calls `deps.injector.repeat_key(code)` — stateless, no
      `firings` / `held` mutation.
- [ ] `decide`: `(HoldToRepeat, Repeat)` with `AutorepeatKey(_, code)` →
      `D::RepeatKey(code)` (replaces the old `SpawnFireOnce`). `SustainedNoRepeat`
      still → `D::Nothing`; `None` (multi-step Macro) still → `guarded(D::SpawnFireOnce)`.
- [ ] `decide`: `(HoldToRepeat, Down)` with `AutorepeatKey(mods, code)` → a guarded
      decision that presses `mods` (each `value=1`) and `code` (`value=1`) and
      registers all of them in the firing's `held` set. Extend `D::HoldKeyDown` to
      carry the modifier set (or add a sibling variant) so `perform` spawns
      `vec![KeyDown(m0), …, KeyDown(code)]` rather than today's single `KeyDown`.
- [ ] Force-release / stop (§7): every existing teardown path
      (`D::ForceReleaseStuck` → `force_release_stuck` draining `held`, chord
      `ReleaseChordFiring`, `stop_all`, deep-stage teardown) balances the key **and
      its modifiers** with `value=0` — verify `force_release_stuck` drains the whole
      set and needs no change beyond the extra codes now in it. The `value=2` stream
      itself needs no teardown.
- [ ] **Required ordering test** (§7): with a `RepeatKey` queued on the injector
      channel when `Up` arrives, the final event for the key is `value=0` and the key
      is not left logically down — the queued `RepeatKey` is delivered before, or
      dropped in favour of, the `value=0`, never after it. (Send order through the
      one mpsc channel already guarantees this — the test locks it.)
- [ ] Dispatch integration tests:
  - Digital Hold-to-repeat on a plain key: `value=1`, then exactly one `value=2` per
    synthesized `EventState::Repeat`, then `value=0` on `Up` — no `[Down,Up]` pairs;
  - Analog-synth grid Hold-to-repeat: same shape, driven by the existing grid
    `RepeatSchedule` (assert the cadence still matches — `advance_fired` untouched);
  - modifier-wrapped key (`Ctrl`+key): `Ctrl` `value=1` once, base key `value=1`
    then `value=2`…, both released `value=0` on `Up`, `Ctrl` never emits `value=2`;
  - single-key Chord Hold-to-repeat: one `value=2` per leader `Repeat`;
  - single-key **deep-stage** Hold-to-repeat: `value=2` stream through the deep
    keyspace, released on the deep band going Up and on `StopAllStages`;
  - Stepper Hold-to-repeat unchanged — still fires discrete items per `Repeat`.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets`, full daemon suite green.
