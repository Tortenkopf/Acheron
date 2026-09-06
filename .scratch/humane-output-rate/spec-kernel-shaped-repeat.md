# Spec: kernel-shaped repeat output (`value=2`)

Status: **implemented** 2026-09-06 in
[`.scratch/kernel-shaped-repeat-impl/`](../kernel-shaped-repeat-impl/issues/) (7 tickets,
all done). ADR-0008 un-gated; `CONTEXT.md` "Toggle" entry restructured (the
"Physical-plausibility ceiling" term already read present-tense). As-built notes inline
where behaviour was pinned down during implementation (see the "As implemented" blocks
in §3.2 and §7).
Source: [Humane output rate](map.md) ticket [07](issues/07-spec-kernel-shaped-repeat.md)
(grilled + ratified with Charon, 2026-09-06)

Depends on the facts in ticket [01](issues/01-audit-holding-repeating-output-paths.md)'s
audit and ticket [03](issues/03-self-dos-macro-guardrail.md)'s guardrail decision. Feeds
ticket [04](issues/04-record-humane-output-rate-principle.md) (the ADR + glossary term) and
ticket [05](issues/05-spec-macro-editor-safety-guidance.md) (the tips describe the
post-change reality).

---

## 1. Goal

Every path that **holds or repeats a single key** must present downstream as **genuine Linux
kernel autorepeat** — one `value=1` (down), then `value=2` (autorepeat) events at the live
kernel `REP_DELAY`→`REP_PERIOD` envelope, then one `value=0` (up) — instead of today's
stream of `[KeyDown, KeyUp]` (`value=1`,`value=0`) pairs with ~0 ms dwell.

Rate compliance is already met (ticket 01: every surface paces at or below the live kernel
rate). This spec is about **shape and dwell**: the `1,0,1,0,…` pair stream is the single
most concrete synthetic tell in the ticket-02 research (§4.2), and each pair also implies a
~0 ms key-hold time that no physical key produces (§3). After this change, Acheron's
held/repeated keyboard output is timing-indistinguishable from a physically held key.

**Non-goals.** Not disguise — `uinput` origin stays visible and that is accepted (§8). Not a
new rate floor — pacing is unchanged. Not a jitter engine — the kernel's own autorepeat is
timer-regular and so is ours.

---

## 2. Implementation fork — decision: **inject `value=2` ourselves**

Two ways to make a held key emit `value=2`:

- **(a) Inject it ourselves** — a new injector capability that writes `KeyEvent::new(code, 2)`,
  driven by the code that already decides when a repeat is due.
- **(b) Advertise `EV_REP` + set `EVIOCSREP`** on the virtual device and let the kernel
  softrepeat held keys.

**Chosen: (a).** Rationale:

- `evdev` 0.13.2 **cannot do (b)**. `VirtualDeviceBuilder` has no repeat method; the `sys`
  module holding `EVIOCSREP` is private (`evdev-0.13.2/src/lib.rs:177`); and
  `UI_SET_EVBIT(EV_REP)` must be issued *before* `UI_DEV_CREATE`, which the crate calls
  inside `build()` (`evdev-0.13.2/src/uinput.rs:265`). (b) would require replacing
  `injector::build_device`'s builder call with hand-rolled `libc` uinput ioctls.
- (b) also forces a dispatch-layer change: to let the kernel repeat a held key we would stop
  reacting per `EventState::Repeat` and instead hold a bare `KeyDown` from physical Down to
  physical Up — decoupling our output cadence from the physical device's own autorepeat.
- (a) has in-tree precedent: `injector::translate` (`daemon/src/injector.rs:368-384`)
  already emits `value=2` for unbound-key passthrough — our `uinput` device carries
  autorepeat fine, and each `.emit()` frames it with its own `SYN_REPORT`.
- (b)'s only real advantage — the kernel owning the steady period — is not needed. The bar
  is "the live kernel rate", which (a) reads from the same `read_kernel_auto_repeat()`
  source surface 2 already uses.

### 2.1 New primitives

| Primitive | Location | Purpose |
|---|---|---|
| `InjectorMessage::RepeatKey(KeyCode)` | `daemon/src/injector.rs` (enum ~`:180-201`) | one autorepeat event |
| `Injector::repeat_key(&self, KeyCode)` | `daemon/src/injector.rs` (~`:237-263`) | send the message; mirrors `force_release_key` |
| handler arm | `injector_loop` (~`:324-361`) | `sink_for(...).emit(&[*KeyEvent::new(key, 2)])?` — **subject to `suppressed`** (unlike `ForceRelease`); a suppressed hold emits nothing and resumes on unsuppress |
| `TriggerDecision::RepeatKey(KeyCode)` | `daemon/src/trigger.rs` (enum `:72-93`) | "emit one autorepeat for this key" |
| `hold_repeat_kind(&Action, &HashMap<MacroId, MacroDef>) -> Option<HoldKind>` | `daemon/src/trigger.rs` (beside `sustained_hold_key` `:101-107`) | classify a binding's held target |
| `enum HoldKind { SustainedNoRepeat(KeyCode), AutorepeatKey(Modifiers, KeyCode) }` | `daemon/src/trigger.rs` | — |
| `single_held_key(&[MacroStep]) -> Option<(Modifiers, KeyCode)>` | `daemon/src/executor.rs` (beside `keypress_steps` `:82-90`) | pure predicate, §6 |

`HoldKind` replaces the bare `Option<KeyCode>` that `sustained_hold_key` returns today:

- **`SustainedNoRepeat(code)`** — `Action::ControllerButton`, or `Action::Keypress` on a
  mouse-button code. Exactly today's `sustained_hold_key` set. The kernel never autorepeats
  `BTN_*`, so on `Repeat` these still resolve to `D::Nothing`.
- **`AutorepeatKey(mods, code)`** — a single keyboard key: an `Action::Keypress` (with or
  without modifiers) on a non-mouse code, **or** an `Action::Macro` whose compiled steps
  satisfy `single_held_key` (§6). On `Repeat` these resolve to `D::RepeatKey(code)`.
- **`None`** — multi-step Macro, Stepper, Profile switch: unchanged behaviour.

---

## 3. `trigger::decide` changes

`decide` (`daemon/src/trigger.rs:121-172`) gains a `macros: &HashMap<MacroId, MacroDef>`
parameter — both call sites (`dispatch::handle_event` for individual, `dispatch::run_chord_effects`
for chord) already hold `PerformDeps` / the config in scope. It calls `hold_repeat_kind`
instead of `sustained_hold_key`.

### 3.1 Hold-to-repeat

```
(HoldToRepeat, Down):
    SustainedNoRepeat(code)  -> guarded(D::HoldKeyDown(code))     [unchanged]
    AutorepeatKey(mods,code) -> guarded(D::HoldKeyDown(code)) after pressing `mods`   [NEW]
    None                     -> guarded(D::SpawnFireOnce)         [unchanged: multi-step macro]

(HoldToRepeat, Repeat):
    SustainedNoRepeat(_)     -> D::Nothing                        [unchanged]
    AutorepeatKey(_, code)   -> D::RepeatKey(code)                [NEW — replaces SpawnFireOnce]
    None                     -> guarded(D::SpawnFireOnce)         [unchanged: whole macro re-runs]

(HoldToRepeat, Up):
    any                      -> D::ForceReleaseStuck              [unchanged]
```

The modifier case: `D::HoldKeyDown` today spawns `spawn_fire_once(vec![KeyDown(code)])`
(`trigger.rs:300-311`). For `AutorepeatKey(mods, code)` with non-empty `mods`, it must spawn
`vec![KeyDown(m0), …, KeyDown(code)]` — modifiers held `value=1` alongside the key, released
together by `force_release_stuck` on `Up` (it drains the whole `held` set). Only `code`
autorepeats. This is exactly what the kernel does for a physically held `Ctrl+X`.

### 3.2 Toggle (Q8 — surface 6 + single-key Macro)

```
(Toggle, Down):
    SustainedNoRepeat(code)  -> D::StartToggleHeld(code)          [unchanged: mouse/gamepad]
    AutorepeatKey(mods,code) -> D::StartToggleAutorepeat(mods, code)   [NEW]
    None                     -> D::StartToggleLoop               [unchanged: multi-step macro]
```

A Toggle whose held target is a single key — plain `Keypress` **or** a single-key `Macro`
(identical compiled steps ⇒ identical behaviour) — no longer loops `[Down,Up]` through
`run_toggle_loop`. It starts a sustained autorepeat hold and stops it on the second press
(and on the Toggle-teardown paths below).

> **As implemented** (`kernel-shaped-repeat-impl` ticket 05): an *individual* Toggle —
> autorepeat or looping — deliberately **survives a Layer switch and an Analog→Digital
> capture flip** (`handle_layer_switch` / `handle_capture_mode_change` only
> `drain_firings`; see [`tartarus-keybinder/spec.md`](../tartarus-keybinder/spec.md)
> § "Toggle behavior across Layer/Profile switches"). It is stopped only by the **second
> press**, a **Profile switch** (`Effect::StopAllToggles`), or the **GUI-focus
> `Command::StopAllToggles`** — `Slots::stop_all` (firings + toggles) is only ever
> invoked on the deep stage's `Slots<StageKey>`, never on the individual `Slots<Input>`.
> Tests: `single_key_autorepeat_toggle_survives_a_layer_switch`,
> `stop_all_toggles_releases_a_running_single_key_autorepeat_toggle`.

`run_toggle_loop` + `target_lap` (`MIN_TOGGLE_LAP` / `combine_toggle_lap_target`) survive
**only** for `D::StartToggleLoop` — the multi-step Macro loop (surface 9, the declared Macro
exception).

New: `ActiveToggle::spawn_autorepeat(injector, mods, code, schedule)` beside
`spawn` / `spawn_held` (`executor.rs:296-313`), running `run_toggle_autorepeat` — see §5.2.

### 3.3 Chord

Chord bindings route through the same `decide` + `Slots::perform`, keyed by `ChordKey`
(`dispatch::run_chord_effects` `:387-405`). Single-key chord Actions inherit §3.1/§3.2
automatically. `chord::feed_repeat` (`chord.rs:213-228`) already re-fires only the
`BTreeSet`-first "leader" member at that member's Repeat cadence (ticket 67) — so a
single-key chord emits one `value=2` per leader Repeat. A multi-key chord Action is a Macro
(`HoldKind::None`) and re-runs whole per Repeat, overlap-guarded — identical to an
individual Hold-to-repeat→Macro binding, covered by ticket 08.

---

## 4. Which paths convert — the full table

| # | Surface | Today | After this spec |
|---|---------|-------|-----------------|
| 1 | Digital-capture Hold-to-repeat | `[Down,Up]` per kernel `Repeat` on the grabbed physical device | `value=1` on Down; **one `value=2` per incoming `EventState::Repeat`**; `value=0` on Up. Cadence unchanged = the real kernel envelope, gated 1:1 |
| 2 | Analog-synth Hold-to-repeat (grid keys) | `[Down,Up]` per `RepeatSchedule` tick | same shape. `RepeatSchedule` / `repeat_due` / `advance_fired` (`capture/analog.rs:248-292`) **untouched** — they still decide *when*; only the emit changes |
| 3 | Chord Hold-to-repeat | inherits 1/2 | inherits — converts iff the chord's Action is a single key (§3.3) |
| 4 | Stepper Hold-to-repeat | advance cursor + fire the newly-selected item per `Repeat` | **not converted.** There is no single held key to autorepeat — each `Repeat` targets a *different* item. Keeps firing discrete items per `Repeat` (kernel-rate-paced; audit verdict OK). `Action::Step` ⇒ `HoldKind::None` |
| 5 | Analog-repeat hold-solid (Depth ≥ `ANALOG_REPEAT_HOLD_SOLID` 235) | bare unbalanced `KeyDown`, then park on `depth_rx.changed()` | `value=1` on crossing 235, then a **self-contained `value=2` emitter** at the kernel `period_ms`, **no initial delay** (§5.3). Tap band < 235 unchanged (ticket 20) |
| 6 | Toggle → keyboard `Keypress` | `run_toggle_loop` `[Down,Up]` @ `target_lap` | sustained autorepeat: `value=1` on press 1, `value=2` at the **full** `REP_DELAY`→`REP_PERIOD` envelope, `value=0` on press 2 / stop (§3.2, §5.2) |
| 6b | Toggle → single-key `Macro` | `run_toggle_loop` (identical steps) | identical to surface 6 |
| 7 | Toggle → mouse button | `run_toggle_held` — one `value=1`, no repeat | **unchanged** |
| 8 | Toggle → gamepad button | `run_toggle_held` — one `value=1`, no repeat | **unchanged** |
| 9 | Toggle → multi-step `Macro` | `run_toggle_loop` @ `target_lap` | **unchanged** — the sole remaining user of `run_toggle_loop` + `target_lap` |
| 10 | Hold-to-repeat → multi-step `Macro` | whole macro re-runs per `Repeat`, overlap-guarded | **unchanged** — ticket 08 locks it |
| 11 | Controller-button digital pulse-hold | `[Down, Delay(35ms), Up]` | **unchanged** — `BTN_*` doesn't autorepeat |

Analog-repeat + Macro: rejected at the config layer (ticket 09) — not reachable. Analog-repeat
in Digital capture mode falls back to plain Hold-to-repeat ⇒ surface 1.

**Digital 1:1 timing is preserved** (ticket 07 item 2): surface 1 still receives exactly one
`EventState::Repeat` per kernel autorepeat tick on the physical key and emits exactly one
`value=2` per — no scheduler of our own interposed.

---

## 5. The envelope

No sourced envelope of this spec's own. The rule:

- `value=1` emitted at the moment of press (unchanged from today's first `KeyDown`).
- first `value=2` emitted when the path's **existing driver** says a repeat is due.
- steady `value=2` at that driver's period thereafter.

| Path | Driver | First `value=2` | Steady period |
|---|---|---|---|
| Surface 1 | the physical key's own kernel autorepeat | real `REP_DELAY` after physical Down (kernel-produced) | real `REP_PERIOD` |
| Surface 2 | `RepeatSchedule` (`capture/analog.rs`), seeded from `read_kernel_auto_repeat()` off `Node::If01`, fallback `DEFAULT_REPEAT_DELAY_MS` 250 / `DEFAULT_REPEAT_PERIOD_MS` 33 | `held_for >= delay_ms` | `period_ms` |
| Toggle (§3.2) | new `RepeatSchedule` read once at `spawn_autorepeat` | **full `delay_ms`** after press 1 — a Toggle-held key *is* a held key, look exactly like one | `period_ms` |
| Surface 5 hold-solid (§5.3) | new `RepeatSchedule` in `run_analog_repeat_loop` | **immediately** (`period_ms`, no `delay_ms`) — the top of a hand-driven tapping ramp, not a fresh press | `period_ms` |

Grounding for the fallback constants and the "already at kernel rate" claim: the user's own
Tartarus Pro reports delay 250 ms / period 33 ms (ticket 68); surface 2 was measured on
hardware at ~33 ms period, "matches the device's kernel autorepeat" (ticket 95,
`grid_r2c3 → KEY_F13`); Analog-repeat hold-solid ticked at ~34 ms (ticket 73). There is **no
Tartarus hardware autorepeat rate** — the device never autorepeats; Analog capture
synthesises the stream at the kernel rate by design.

### 5.1 Timestamp

Let `evdev`'s `.emit()` default the `input_event` timestamp (it does not set one). The
kernel then stamps at handling time with a monotonic clock — exactly what real
`input_repeat_key` does via `ktime_get()`. **Do not** set an explicit timestamp: current
`uinput` honours a valid userspace timestamp (research §6.2), which would put any regularity
in it into a field a detector reads directly.

### 5.2 `run_toggle_autorepeat` (new, `daemon/src/executor.rs`)

```
async fn run_toggle_autorepeat(injector, mods: Modifiers, key: KeyCode,
                               schedule: RepeatSchedule, cancel: CancellationToken) {
    let mut held = HashSet::new();
    for m in modifier_codes(mods) { execute_step(&injector, &mut held, KeyDown(m)).await?; }
    execute_step(&injector, &mut held, KeyDown(key)).await?;        // value=1
    let started = Instant::now();
    let mut fired = 0u32;
    loop {
        // next due at started + delay_ms + fired*period_ms
        let due = schedule.next_due_after(started.elapsed(), fired);  // see §5.4
        tokio::select! {
            _ = cancel.cancelled() => break,
            _ = sleep(due) => {}
        }
        let held_for = started.elapsed();
        if !schedule.repeat_due(held_for, fired) { continue; }
        fired = schedule.advance_fired(held_for, fired);              // §5.4 missed-tick clamp
        injector.repeat_key(key).await;                              // value=2
    }
    force_release(&injector, held).await;                            // value=0 for key + mods
}
```

`ActiveToggle::spawn_autorepeat` reads the schedule via a `spawn_blocking`
`analog::read_kernel_auto_repeat()` **at daemon startup** and threads the resolved
`RepeatSchedule` down as a plain value — same pattern ticket 68 used for `target_lap` (an
inline per-press read broke the `tokio::time::pause()` test harness; see ticket 68's Answer).
So `dispatch::run` gains one more threaded `RepeatSchedule` parameter alongside
`toggle_lap_target`, and `spawn_autorepeat` stays synchronous.

### 5.3 Surface 5 hold-solid — `run_analog_repeat_loop` changes

`run_analog_repeat_loop` (`daemon/src/analog_repeat.rs:253-318`) gains:

- a `schedule: RepeatSchedule` parameter (read at spawn in `dispatch::update_analog_repeats`
  `:550-591`, same `read_repeat_schedule()` source);
- `solid_since: Option<Instant>` and `solid_fired: u32` loop state.

The `TickPlan::HoldSolid` arm (`analog_repeat.rs:267-280`) changes from "press KeyDowns once,
then `select!` park" to:

```
TickPlan::HoldSolid => {
    if !holding_solid {
        for step in &steps { if KeyDown => execute_step(...).await; }   // value=1
        holding_solid = true;
        solid_since = Some(Instant::now());
        solid_fired = 0;
    }
    let base = solid_since.unwrap();
    // no initial delay: Nth value=2 due at base + N*period_ms  (delay_ms treated as 0 here)
    let due = period_ms.saturating_sub(base.elapsed() % period_ms ...);  // §5.4
    tokio::select! {
        () = cancel.cancelled() => break,
        _  = depth_rx.changed() => {}
        () = sleep(due) => {
            let elapsed = base.elapsed();
            solid_fired = advance_fired_no_delay(elapsed, solid_fired, period_ms);   // §5.4
            injector.repeat_key(solid_key).await;                       // value=2
        }
    }
}
```

`solid_key` is the single non-modifier `KeyDown` in `steps` (Analog-repeat is grid-key-only
and its Action compiles to a keypress or single button). On leaving hold-solid —
`release_solid_first` true, or cancel — the existing `release_solid` / `force_release` fires
the `KeyUp` steps ⇒ `value=0`, and `solid_since`/`solid_fired` reset.

The **tap band** (`TickPlan::Tap`, Depth < 235) is untouched: `fire_analog_repeat_pulse`
still emits balanced `[KeyDown … pulse_hold … KeyUp]` pulses paced by `tap_pace_wait`
(ticket 06). Ticket 20's deliberately-human tapping shape stays.

### 5.4 Missed-deadline clamping (ticket 07 item 5)

- **Surface 2**: nothing to do. `RepeatSchedule::advance_fired` (`capture/analog.rs:270-290`,
  ticket 06) already re-bases `fired` from real elapsed time after a stall — emit one
  `value=2` now, next due a full `period_ms` later, no catch-up burst. It governs *when* a
  repeat fires; this spec changes only *what* is emitted then. Carries for free.
- **Toggle (§5.2)** and **hold-solid (§5.3)**: the new emitters adopt the identical
  discipline. Reuse `RepeatSchedule::advance_fired` for the Toggle case (it has the
  `delay_ms` envelope). For hold-solid (no `delay_ms`), add a sibling
  `RepeatSchedule::advance_fired_steady(elapsed, fired) -> u32` = `max(fired+1, elapsed/period_ms + 1)` —
  or generalise `advance_fired` with a `delay: Duration` argument. Pick one; unit-test it
  the way `advance_fired` is tested.
- **Tap band**: `tap_pace_wait` (`analog_repeat.rs:111-113`, ticket 06) unchanged.

Kernel parity: `input_repeat_key` re-arms from *now* on every tick and never bursts — all
three emitters match this.

---

## 6. The single-key predicate

```rust
/// Some((mods, key)) iff `steps` is a single held keyboard key — the shape a
/// physically held key produces. Used by both the Hold-to-repeat and Toggle
/// compile sites to route through the value=2 path instead of a [Down,Up] loop.
pub(crate) fn single_held_key(steps: &[MacroStep]) -> Option<(Modifiers, KeyCode)>
```

**Qualifies** (`Some`):

- `[KeyDown(k), KeyUp(k)]` — a plain unmodified `Keypress`, or a `Macro` compiling to exactly this.
- `[KeyDown(m0)…KeyDown(mN), KeyDown(k), KeyUp(k), KeyUp(mN)…KeyUp(m0)]` — a modifier-wrapped
  single key (`Ctrl+X` etc.), `m*` being modifier codes in `keypress_steps`' fixed order
  (`executor.rs:43-58`). Modifiers are held `value=1`; only `k` autorepeats with `value=2`;
  all released on `Up`. This is precisely how the kernel repeats a physically held modified key.
- either of the above followed by **at most one trailing `MacroStep::Delay`** — ignored
  (ticket 03's "turbo macro" written with a trailing pause).

**Disqualifies** (`None`):

- more than one distinct non-modifier key;
- any `MacroStep::Delay` *between* the key steps (an authored cadence — treat as a real macro);
- any other shape.

A `Macro` that returns `None` keeps today's behaviour: whole macro re-runs per `Repeat`
(Hold-to-repeat) or loops at `target_lap` (Toggle). `Action::Step` and `Action::ProfileSwitch`
never reach this predicate (`HoldKind::None` / handled earlier).

### 6.1 Detection site

In `Slots::perform` / `decide`, not `executor`. `decide` gains `&macros` and calls
`hold_repeat_kind(&binding.action, macros)`, which for an `Action::Macro` resolves the
`MacroDef`, compiles its steps once (or reuses `executor::compile`), and runs
`single_held_key` on the result. For the common `Action::Keypress` case no macro map access
is needed — `hold_repeat_kind` reads `mods` + `key` straight off the action.

`single_held_key` itself stays pure over `&[MacroStep]` and is unit-tested standalone
(qualifying shapes, each disqualifying shape, the trailing-`Delay` boundary, the
modifier-order match).

---

## 7. Force-release / stop semantics (ticket 07 item 7)

The held `value=1` is registered in a `held: HashSet<KeyCode>` exactly as today's
`D::HoldKeyDown` does — via `spawn_fire_once(vec![KeyDown(code), …mods])`
(`trigger.rs:300-311`) for Hold-to-repeat, via the loop-private `held` in
`run_toggle_autorepeat` / `run_analog_repeat_loop` for the others. **`value=2` events are
stateless** — `Injector::repeat_key` never touches any `held` set.

Every teardown path already emits the terminating `value=0`:

| Trigger | Path | Balances the held `value=1`? |
|---|---|---|
| physical `Up` (individual / chord member) | `decide` → `D::ForceReleaseStuck` → `Slots::force_release` → `FiringHandle::force_release_stuck` drains `held` (`executor.rs:229-241`) | ✓ (key + any modifiers) |
| chord dissolves (any member released) | `ChordEffect::ReleaseChordFiring` → `Slots::force_release` | ✓ |
| Layer switch, Profile switch, Analog→Digital flip — **Hold-to-repeat firing** | `Slots::drain_firings` (force-release + remove every firing; `handle_layer_switch` / `handle_capture_mode_change` / `SwitchProfile`'s `Effect::ReleaseAllHolds`) | ✓ (key + any modifiers) |
| **Profile switch** or **GUI-focus `Command::StopAllToggles`** — **autorepeat Toggle** | `Slots::stop_all_toggles` → each `ActiveToggle` `cancel` → `force_release(held)` | ✓ |
| Toggle second press / stop | `run_toggle_autorepeat` `cancel` → `force_release(held)` | ✓ |
| Analog-repeat cancel, or Depth leaves hold-solid | `run_analog_repeat_loop` → `release_solid` / `executor::force_release` | ✓ |
| dropped device connection | capture layer synthesises `EventState::Up` (`analog.rs:860-870`) | ✓ |

Ticket 33's `force_release_stuck` is unchanged — it still balances the one bare `KeyDown`
(now plus modifiers). The `value=2` stream needs no teardown of its own.

`InjectorMessage` ordering: all key writes go through the one mpsc channel in send order
(`injector.rs:324-361`), so a queued `RepeatKey` cannot land after the `ForceRelease` /
`KeyState(up)` that follows it.

**Required test**: a `value=2` in flight (queued `RepeatKey`) when `Up` arrives must be
delivered before, or dropped in favour of, the `value=0` — never after it. Assert the final
event for the key is `value=0` and the key is not left logically down.

---

## 8. Heuristics-report rationale

*(Written free-standing here; ticket 04 lifts it into ADR-0008 — see §9. Do not cite
ADR-0002: it is about "direct evdev/uinput instead of OpenRazer" and says nothing about
detectability.)*

1. **`uinput` origin is always visible, and that is accepted — not fought.** Acheron's
   device is named "Acheron Virtual Tartarus Pro" and is enumerable under
   `/sys/devices/virtual/input/` with a blank `phys` (research §0, §6.3). Nothing in this
   change hides it, and nothing should.
2. **What the change buys: held and repeated keyboard output is now timing-indistinguishable
   from a physically held key.** Correct event value (`2`, not a `1,0` pair); correct
   envelope (`REP_DELAY` then the shorter `REP_PERIOD`, from the machine's own live config);
   correct dwell (the actual physical hold duration, not the ~0 ms dwell every `[Down,Up]`
   pair implies — research §3, §4.2).
3. **The one variable input — the Analog-repeat depth ramp — is continuously hand-driven.**
   A person is modulating grid-key Depth in real time; the repeat rate is a live human
   choice moment to moment, not a scripted curve. The hold-solid `value=2` stream is reached
   only by pressing a physical key to ≥ 92% travel and holding it there.
4. **Conclusion for any future heuristics report:** Acheron's keyboard output stays within
   the rate and shape a person with this keyboard could produce, and its one dynamic mode is
   human-in-the-loop. Plausibility of *rate*, not disguise of *origin*.

---

## 9. Cross-ticket effects (apply when ticket 07 resolves)

- **Ticket 08** → rewire `Blocked by: 01, 07`. Its Toggle→Macro regression test splits:
  a Toggle wrapping a **single-key** Macro now rides this spec's `value=2` path (assert
  `value=1` … `value=2` at the kernel envelope … `value=0`, *not* a `target_lap`-paced
  `[Down,Up]` loop); a Toggle wrapping a **multi-step** Macro still paces at `target_lap`.
  The Hold-to-repeat→Macro re-fire test (item 2) is unaffected but should note the
  chord-keyed variant rides the same `decide`/`perform` seam.
- **Ticket 04** → the glossary term and ADR cite **ADR-0008** (new, written by ticket 04),
  not ADR-0002. ADR-0008 is the *first* place the "uinput origin is always detectable"
  premise is recorded — §8 above is its rationale block. The ticket-04 note that says
  "cross-check against ADR-0001/0002 for tone" stands; the `(ADR-0002)` citation in its
  glossary draft is wrong and must become `(ADR-0008)`.
- **Ticket 05** → unblocks (was `Blocked by: 01, 06, 07`). The tips now describe: built-in
  Hold-to-repeat / Toggle of a single key produce genuine autorepeat; only a multi-step
  Macro loops as discrete pairs.
- **Map** → Decisions-so-far gains this ticket's gist; add a line under the ADR-0002
  correction.

---

## 10. Out of scope for the implementation effort

- Surfaces 7/8/9/10/11 — unchanged (see §4).
- Any change to pacing / rate floors — `RepeatSchedule`, `repeat_due`, `advance_fired`,
  `tap_pace_wait`, `MIN_TOGGLE_LAP`, `combine_toggle_lap_target` keep their current values
  and semantics.
- Stepper Hold-to-repeat (surface 4) — stays discrete-item-per-Repeat.
- The Analog-repeat tap band < 235 — stays pulsed (ticket 20).
- `EV_REP` / `EVIOCSREP` on the virtual device — rejected (§2); revisit only if the `evdev`
  dependency is replaced for unrelated reasons.
- Jitter / anti-regularity injection — out (map "Out of scope").
