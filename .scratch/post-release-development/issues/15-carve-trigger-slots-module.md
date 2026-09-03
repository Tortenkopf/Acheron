<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 15 — Concentrate the `(firings, toggles)` handle pair into one `trigger::Slots<K>` deep module

**What to build:** The `(HashMap<K, FiringHandle>, HashMap<K, ActiveToggle>)`
pair and the operations always performed on it — read one key's liveness,
snapshot every key's liveness, perform a `trigger::TriggerDecision`, tear a
key (or every key) down — move behind one generic struct
`trigger::Slots<K>` in `daemon/src/trigger.rs`, holding both maps private:

```rust
impl<K: Eq + Hash + Clone> Slots<K> {
    /// Firing-wins liveness of one key — the `trigger::decide` overlap guard.
    fn slot(&self, key: &K) -> Option<Slot>;
    /// Toggle-wins liveness of every key — `chord::feed`'s completion input.
    fn snapshot(&self) -> HashMap<K, Slot>;
    /// Perform one decision against the pair (`compile_action` +
    /// `executor::spawn_*` + map insert, or `force_release`).
    async fn perform(&mut self, decision, key: K, binding: &Binding, deps) -> io::Result<()>;
    /// Release (never remove) one key's stuck firing — ticket 33's path.
    async fn force_release(&mut self, key: &K, injector: &Injector);
    /// Stop + remove one key's Toggle; returns whether one was present.
    async fn stop_toggle(&mut self, key: &K, injector: &Injector) -> bool;
    /// Drain + stop every Toggle (the `StopAllToggles` effect / command).
    async fn stop_all_toggles(&mut self);
    /// Toggle keys currently active — `GetState`'s `active_toggles`.
    fn active_toggle_keys(&self) -> impl Iterator<Item = &K>;
}
```

`DispatchState` holds two: `individual: Slots<Input>` (was `in_flight` +
`toggles`) and `chord_slots: Slots<ChordKey>` (was the `ChordRuntime`
struct). The coming `stage::Engine` (dual-stage ticket 01) will hold a third,
`Slots<StageKey>`, instead of hand-rolling a fourth copy.

**No behaviour change.** Every firing, toggle, overlap-guard verdict, chord
completion and teardown resolves identically for every input sequence. The
two deliberately-inverse tie-breaks (`slot()` firing-wins, `snapshot()`
toggle-wins) are **preserved exactly**, not unified — they are genuinely
non-equivalent in the "a live re-bind left both maps populated for one key"
edge case (`chord.rs:180` branches on `Some(Slot::Toggle)`; `trigger.rs:118`
blocks only on `Some(Slot::FiringUnfinished)`), and unifying either is a
behaviour change out of scope for this carve.

## The friction

The pair is open-coded **2½ times** and the protocol around it is scattered:

- **`DispatchState.{in_flight, toggles}`** (`dispatch.rs:100–101`), keyed by
  `Input`.
- **`ChordRuntime.{firings, toggles}`** (`dispatch.rs:85–88`), keyed by
  `ChordKey` — a struct whose whole body *is* the pair, whose doc comment
  already says it mirrors "how `axis::Engine` bundles its own two maps."
- **`stage::Engine.{firings, toggles}`** — `tartarus-dual-stage-keys` ticket
  01 §2/§4: "exactly `ChordRuntime`'s shape", fired via "the existing generic
  `perform_trigger<K>` … keyed by a new `StageKey(Input)`", with its own
  `slot_for` call and its own teardown fan-out. Charting, not built.

The three operations on the pair are each a free function or a macro:

- **`chord_slots(runtime: &ChordRuntime)`** (`dispatch.rs:870–884`) —
  **toggle-wins**: writes `Slot::Toggle` over any firing entry for a key.
  Rebuilt on every `PhysicalEvent` (`dispatch.rs:240`).
- **`slot_for<K>(firings, toggles, key)`** (`dispatch.rs:895–908`) —
  **firing-wins**: returns the firing slot first, `Toggle` only as fallback.
  Called at 3 sites (`~290`, `~332`, `~446`). Its doc comment
  (`dispatch.rs:886–894`) is the **only** thing that documents the divergence
  from `chord_slots` — a real bug surface guarded by prose.
- **`trigger_ctx!`** (`dispatch.rs:64–76`) + **`TriggerCtx<'a, K>`**
  (`dispatch.rs:918–934`) + **`perform_trigger<K>`** (`dispatch.rs:935–974`)
  — a per-call-site borrow struct built from a macro because, per the
  rationale comment at `dispatch.rs:60–62`, *"a `&mut self` method can't be
  generic over which map type `K` selects, so `perform_trigger<K>` stays a
  free function."* 17 mentions of `perform_trigger` / `trigger_ctx!` in the
  file.
- **Teardown** is three more hand-rolled shapes: `trigger::force_release_stuck`
  / `trigger::stop_toggle` (`trigger.rs:168–193`, an impure annex the module
  doc apologises for — *"the one place this module touches `executor` /
  `injector` types … deliberately not part of the pure core"*) and the free
  `dispatch::stop_all_toggles` (`dispatch.rs:1122`).

**Deletion test.** Deleting `Slots<K>` scatters the two-map bookkeeping and
both tie-break rules back across three structs, restores the `trigger_ctx!`
macro that exists only to dodge `&mut self`, and forces `stage::Engine` to
hand-roll copy #4. Concentrating it is the win: one precedence rule lives
next to its state, the `slot_for`/`chord_slots` divergence becomes two
clearly-named methods with a table test in `trigger.rs` instead of a
dispatch integration path, `trigger.rs` loses its impure annex, and
`stage::Engine` becomes `Slots<StageKey>` for free.

## The module

```rust
// daemon/src/trigger.rs — alongside the pure `decide` matrix and `Slot`.
//
// `Slots<K>` is the runtime handle pair `decide`'s output is performed
// against. It owns `FiringHandle` / `ActiveToggle` and has an async
// `perform` — so it is NOT part of the pure core, the same way
// `force_release_stuck` / `stop_toggle` were not. Those two free functions
// are absorbed as methods; nothing else in `trigger.rs` changes its purity.
// The module doc is reworded: "the pure `decide` matrix + the `Slots<K>`
// handle pair it is performed against."

pub(crate) struct Slots<K> {
    firings: HashMap<K, FiringHandle>,
    toggles: HashMap<K, ActiveToggle>,
}
```

- **`slot(&K) -> Option<Slot>`** — the exact body of today's `slot_for`:
  `firings.get(key)` → `FiringUnfinished` / `FiringFinished` by
  `is_finished()`; else `toggles.contains_key(key).then_some(Slot::Toggle)`.
  **Firing-wins.** Feeds `trigger::decide`'s overlap guard, which only ever
  blocks on `Some(Slot::FiringUnfinished)`.
- **`snapshot() -> HashMap<K, Slot>`** — the exact body of today's
  `chord_slots`: insert every firing first, then overwrite with `Slot::Toggle`
  for every key in `toggles`. **Toggle-wins.** No cache — rebuilt per call,
  as today (chord maps are ≤ ~12 keys; caching would need invalidation on
  every mutation).
- **`perform(decision, key, &binding, deps)`** — today's `perform_trigger<K>`
  body verbatim (`compile_action` behind the guard `decide` already cleared →
  `executor::spawn_fire_once` / `ActiveToggle::spawn{,_held}` + insert into
  `self.firings` / `self.toggles`, or `self.force_release(&key, injector)`).
  `deps` carries what `TriggerCtx` bundled minus the two maps: `injector`,
  `&macros`, `&steppers`, `&mut cursors`, `toggle_lap_target`. Passed as
  positional args, or an inline `perform::Deps<'a>` borrow struct built at the
  call site (no macro) if the tail reads badly — an implementation call, not
  a design one.
- **`force_release(&K, &Injector)`** / **`stop_toggle(&K, &Injector) -> bool`**
  — `trigger::force_release_stuck` / `trigger::stop_toggle` bodies verbatim,
  now `&mut self` methods.
- **`stop_all_toggles(&mut self)`** — `dispatch::stop_all_toggles` body
  verbatim (`for (_, toggle) in self.toggles.drain() { toggle.stop().await }`).
- **`active_toggle_keys() -> impl Iterator<Item = &K>`** — `self.toggles.keys()`,
  for `GetState`'s `active_toggles` (`dispatch.rs:650`).

`Slots<K>` derives nothing it can't — `Default` where `K` allows (both maps
`HashMap::new()`), used by `DispatchState::new` and the dual-stage engine.

## Integration in `dispatch.rs`

- **`DispatchState`**: `in_flight` + `toggles` → one `individual: Slots<Input>`
  field; `chord_runtime: ChordRuntime` → `chord_slots: Slots<ChordKey>`. The
  `ChordRuntime` struct + its `#[derive(Default)]` are **deleted**; its doc
  comment (the `axis::Engine` parallel, "adding a new piece of dispatch
  runtime state means a field here") moves onto the `chord_slots` field or is
  folded into `DispatchState`'s own doc. `DispatchState::new` builds both with
  `Slots::default()`.
- **The Down-stops-Toggle guard** (`dispatch.rs:209–213`):
  `trigger::stop_toggle(&mut self.toggles, &event.input)` →
  `self.individual.stop_toggle(&event.input, &self.injector)`.
- **The chord snapshot** (`dispatch.rs:240`): `let live =
  chord_slots(&self.chord_runtime);` → `let live = self.chord_slots.snapshot();`.
  **`chord::feed`'s signature is unchanged** — it keeps taking `live:
  &HashMap<ChordKey, Slot>` so `chord` stays a pure, data-in/data-out state
  machine (ticket 07). It does **not** take `&Slots<ChordKey>`.
- **The 3 `slot_for` + `trigger_ctx!` + `perform_trigger` sites**
  (`~290`, `~332`, `~446`) each collapse from `slot_for(…)` + `trigger::decide`
  + `trigger_ctx!(…)` + `perform_trigger(…)` to:

  ```rust
  let slot = self.individual.slot(&event.input);            // or self.chord_slots
  let decision = trigger::decide(&binding, event.state, slot);
  self.individual.perform(decision, event.input, &binding, deps).await?;
  ```

- **`run_chord_effects`** (`dispatch.rs:319–363`): `slot_for(&self.chord_runtime
  .firings, …)` → `self.chord_slots.slot(&key)`; `perform_trigger(…, &mut
  self.chord_runtime.firings, &mut self.chord_runtime.toggles, …)` →
  `self.chord_slots.perform(…)`; `trigger::force_release_stuck(&self
  .chord_runtime.firings, &key, …)` → `self.chord_slots.force_release(&key,
  &self.injector)`; `trigger::stop_toggle(&mut self.chord_runtime.toggles,
  &key)` → `self.chord_slots.stop_toggle(&key, &self.injector)`;
  `trigger::force_release_stuck(&self.in_flight, &input, …)` →
  `self.individual.force_release(&input, &self.injector)`.
- **`run_effects`** (`dispatch.rs:569–574`): `Effect::StopToggle(input)` →
  `self.individual.stop_toggle(&input, &self.injector)`;
  `Effect::StopAllToggles` → `self.individual.stop_all_toggles()`.
- **`Command::StopAllToggles`** (`dispatch.rs:665–666`) →
  `self.individual.stop_all_toggles()`.
- **`GetState`** (`dispatch.rs:650`): `self.toggles.keys().copied().collect()`
  → `self.individual.active_toggle_keys().copied().collect()`.
- **The test harness `finish()`** (`dispatch.rs:1394–1396`):
  `stop_all_toggles(&mut self.state.toggles)` →
  `self.state.individual.stop_all_toggles()`.

## What moves, what stays

- **Into `trigger::Slots<K>`:** the bodies of `dispatch::chord_slots`,
  `dispatch::slot_for`, `dispatch::perform_trigger`, `dispatch::stop_all_toggles`,
  `trigger::force_release_stuck`, `trigger::stop_toggle`. **Deleted after the
  move:** `trigger_ctx!`, `TriggerCtx<'a, K>`, the `ChordRuntime` struct.
- **Stays in `trigger.rs` unchanged:** `Slot` (pure vocabulary — `decide`'s
  parameter, `chord::feed`'s parameter, `Slots`' output), `TriggerDecision`,
  `decide`, `sustained_hold_key`. The module doc's second paragraph
  (`force_release_stuck` / `stop_toggle` "at the bottom") is rewritten to
  describe `Slots<K>` instead.
- **Stays in `dispatch.rs`:** everything else — `handle_event`,
  `run_chord_effects`, `dispatch_individual_down`, the `select!` loop, the
  `axis` / `analog_repeat` engines, every `handle_*`.
- **`chord.rs` / `executor.rs` / `analog_repeat.rs` / `edit.rs` doc comments**
  that name `dispatch::perform_trigger`, `dispatch::chord_slots`,
  `dispatch::slot_for`, `ChordRuntime`, or `force_release_stuck` as a free
  function (`chord.rs:51/75/116/317`, `trigger.rs:11/19/40`,
  `executor.rs:225/234`, `analog_repeat.rs:169/344`, `edit.rs:406`) are
  updated to the new names. No code in those files changes.
- **`FiringHandle` / `ActiveToggle` / `Injector` / `executor::spawn_*`** stay
  in `executor.rs` / `injector.rs`; `Slots<K>` imports them exactly as
  `perform_trigger` did.

## Landing in one pass

Add `Slots<K>` to `trigger.rs` with the six free-function / macro bodies
moved in, retarget every `dispatch.rs` call site, delete `ChordRuntime` /
`TriggerCtx` / `trigger_ctx!`, update the cross-file doc comments, then the
test sweep — one PR (ticket 03–13 precedent). A half-migrated `DispatchState`
with `individual: Slots<Input>` but `chord_runtime` still a bare struct, or
`perform` a method on one path and a free function on the other, is harder to
read than either end state.

## Behaviour-preservation protocol

Not the config gatekeeper, but this *is* the latency-critical input path and
every firing/toggle/teardown transition runs through it:

- **Diff each moved body line-by-line against `HEAD`.** Load-bearing:
  `slot()` checks `firings` **before** `toggles` and returns
  `FiringUnfinished` / `FiringFinished` by `handle.is_finished()`;
  `snapshot()` inserts firings **first** then overwrites with `Toggle`;
  `perform`'s `SpawnFireOnce` / `HoldKeyDown` arms insert into `firings`,
  `StartToggleLoop` / `StartToggleHeld` into `toggles`, `ForceReleaseStuck`
  calls `force_release`; `compile_action` still runs **inside** `perform`
  (behind the guard `decide` cleared) so a dropped Step firing never advances
  a Stepper cursor; `stop_toggle` still `remove`s (not just stops);
  `force_release` still `get`s (never removes — the lingering entry keeps
  `FiringFinished` distinct from `None`); `stop_all_toggles` still `drain`s.
- **`cargo test -p acheron-daemon` fully green** — the dispatch-harness firing
  / toggle / chord / overlap-guard / stuck-key / `StopAllToggles` tests —
  before any test moves.
- **The two-tie-break divergence is exercised directly** (see Tests) — the
  `dispatch.rs:886–894` doc comment's claim becomes an assertion.
- **`/code-review` on both the Standards and Spec axes**, as tickets 05–14
  did.

## Tests: replace, don't layer

- **New synchronous `trigger::slots` unit tests** — the primary surface:
  - **The tie-break table.** A `Slots<K>` with one key present in **both**
    maps (a `FiringUnfinished` firing *and* a `Toggle`), plus the
    firing-only, toggle-only, `FiringFinished`-only and absent cases →
    `slot(&k)` returns the firing-wins verdict, `snapshot()[&k]` returns the
    toggle-wins verdict, for every combination. This is the assertion the
    lone doc comment at `dispatch.rs:892` never was.
  - **`perform`** over `TriggerDecision × slot state` → the exact map
    mutation (`firings` gains an entry / `toggles` gains an entry / a stuck
    firing is released / nothing), mirroring `trigger::decide`'s own
    `decision_table` shape. Async, but no D-Bus, no `Config` — a stub
    `Injector` + inline `Binding`, as `perform_trigger`'s current coverage
    uses.
  - **`stop_toggle` / `force_release` / `stop_all_toggles`** → entry removed
    / entry retained-but-released / all drained.
- **Kept in the dispatch harness as integration smoke** — the existing
  end-to-end firing / overlap-guard / chord-completion / `StopAllToggles`
  tests prove `handle_event` and `run_chord_effects` actually route through
  `Slots<K>`. Any test that existed *only* to reach `perform_trigger` /
  `slot_for` / `chord_slots` through a synthetic dispatch path and assert on
  the map directly is deleted — covered at the unit now.
- **Unchanged** — `chord.rs`'s `feed` / `tick` decision table (its `live:
  &HashMap<ChordKey, Slot>` input is untouched), `trigger.rs`'s
  `decision_table`, `edit.rs`'s `Effect` assertions.
- Net: the daemon suite gains the `trigger::slots` module, loses a handful of
  synthetic-dispatch map-assertion tests; coverage strictly increases (the
  both-maps-populated tie-break case is untested today).

## Decisions from the grilling

- **Interface shape: the conservative struct (a), not the invasive push (b).**
  `Slots<K>` is a field on `DispatchState` / `chord_slots` / `stage::Engine`;
  `chord::feed` keeps its snapshot parameter. Fork (b) — pushing the maps
  inside `chord` / `trigger` so `feed` loses its `live` arg — was rejected: it
  reverses what post-release tickets 07 and 08 built (`chord.rs` "does no
  I/O"; `trigger::decide` "spawns no task, takes no `&Injector`"), for the
  sole gain of one dropped parameter. (Q1)
- **Filed as standalone ticket 15** in `post-release-development`, not folded
  into dual-stage ticket 01 — keeps dual-stage's blast radius smaller,
  matches the tickets 03–14 one-carve-per-PR pattern, and lands the seam
  *before* dual-stage implementation so `stage::Engine` inherits
  `Slots<StageKey>` cleanly. (Q2, §4)
- **No `CONTEXT.md` entry.** `Slots<K>` is dispatch-internal runtime plumbing,
  like `DispatchState` and `ChordRuntime` before it — ticket 14's precedent of
  deliberately declining an entry for a locus refactor. (Q3)
- **`perform` becomes a method; `trigger_ctx!` + `TriggerCtx` + the free
  `perform_trigger<K>` are deleted.** The macro's entire reason for existing
  (`dispatch.rs:60–62`: "a `&mut self` method can't be generic over which map
  type `K` selects") evaporates once `Slots<K>` *is* the `self`. (Q5)
- **The two tie-breaks stay as two named methods** — `slot()` firing-wins,
  `snapshot()` toggle-wins — **not unified.** Verified non-equivalent in the
  both-maps-populated edge case (`chord.rs:180` vs `trigger.rs:118`);
  unifying is a behaviour change out of scope. The divergence moves from a
  doc comment to a table test in `trigger.rs`. (Q6)
- **`snapshot()` rebuilds per call, no cache** — as `chord_slots` does today;
  maps are tiny, caching would need mutation-time invalidation. (Q7)
- **`ChordRuntime` is deleted** — it *is* `Slots<ChordKey>`;
  `DispatchState.{in_flight, toggles}` become one `Slots<Input>` field. (Q8)
- **`Slots<K>` absorbs the teardown operations** — `trigger::force_release_stuck`,
  `trigger::stop_toggle`, `dispatch::stop_all_toggles` become methods.
  `trigger.rs` loses its impure free-function annex entirely. (Q14)
- **`Slots<K>` lives in `trigger.rs`** as `trigger::Slots<K>`, not a new
  `slots.rs` — `Slot` / `decide` are already there, `chord.rs` already imports
  `crate::trigger::Slot`, and `slots::Slots` would stutter. `trigger.rs`'s net
  purity profile is unchanged (annex functions become `Slots` methods, not
  removed). (Q4)
- **`chord::feed` signature is unchanged** — keeps `live: &HashMap<ChordKey,
  Slot>`; does not take `&Slots<ChordKey>`. Keeps `chord` data-in/data-out.
  (Q9)
- **Pointer note into dual-stage only** — a line in
  `tartarus-dual-stage-keys/map.md` Decisions-so-far and one in ticket 01
  noting ticket 15 lands `trigger::Slots<K>` and `stage::Engine` should hold
  `Slots<StageKey>`. Ticket 01's §2/§4 design prose is **not** rewritten —
  that effort's spec ticket 06 integrates it. (Q13)
- **Single PR** — design fully settled here, tickets 03–13 precedent, no
  spec/impl split. (Q12)

## Facts dug from the code during the grilling (not asked of the user)

- `trigger::decide` distinguishes **only** `Some(Slot::FiringUnfinished)` —
  that is the sole value that returns `D::Nothing` from the overlap guard
  (`trigger.rs:117–123`). `Toggle`, `FiringFinished` and `None` are treated
  identically. So `slot()`'s firing-wins rule only changes an observable
  outcome versus toggle-wins when a key simultaneously holds a
  `FiringUnfinished` firing **and** a `Toggle`.
- `chord::feed` reads `Slot` three ways: `!matches!(…, Some(Slot::Toggle |
  Slot::FiringUnfinished))` gates a completion (`chord.rs:168`);
  `matches!(…, Some(Slot::Toggle))` alone drives the "second completion stops
  the Toggle" path (`chord.rs:180`); `slot_is_firing` =
  `FiringUnfinished | FiringFinished`, Toggle excluded (`chord.rs:320`). The
  `chord.rs:180` branch is where `snapshot()`'s toggle-wins rule is
  load-bearing.
- `DispatchState` is entirely dispatch-internal — `main.rs` only calls
  `dispatch::run(…)` (`main.rs:143`); `DispatchState::new` takes the owned
  collaborators. Field renames ripple **only** inside `dispatch.rs` (plus one
  test-harness line at `dispatch.rs:1395`).
- `perform_trigger` / `trigger_ctx!` appear 17× in `dispatch.rs`; `slot_for`
  at 3 call sites (`~290`, `~332`, `~446`); `chord_slots` at 1 (`~240`).
- `handle_layer_switch` (`dispatch.rs:1001`) and `handle_capture_mode_change`
  (`dispatch.rs:1057`) tear down **only** `axis` + `analog_repeat`, never the
  firing/toggle maps — a held key keeps firing across a Layer switch. So
  `Slots<K>` needs **no** `stop_all` fan-out into those handlers; the only
  teardown callers are the `StopToggle` / `StopAllToggles` effect + command
  sites and `run_chord_effects`.
- `trigger::force_release_stuck` / `stop_toggle` are already generic
  (`<K: Eq + Hash>`) and already the shared home for what were "three
  hand-rolled copies of the same two lines" (`trigger.rs:161–167`) — folding
  them into `Slots<K>` is the same consolidation one level up.
- `executor::FiringHandle::force_release_stuck` (`executor.rs:248`) is the
  method `trigger::force_release_stuck` wraps — it stays put; `Slots::force_release`
  calls it exactly as the free function does.
- `trigger.rs` is 311 lines, `chord.rs` 846, `dispatch.rs` 4473
  (implementation ends ~1127).

**Blocked by:** None — post-release tickets 07 (`chord` state machine), 08
(`trigger` module), 09 (`DispatchState`) are all resolved; this builds
directly on their seams.

**Status:** resolved

- [x] `trigger::Slots<K>` added to `daemon/src/trigger.rs`: private `firings`
      / `toggles`; exposes `slot`, `snapshot`, `perform`, `force_release`,
      `stop_toggle`, `stop_all_toggles`, `active_toggle_keys` `pub(crate)` and
      nothing else; hand-written `impl<K> Default` (both maps `HashMap::new()`,
      no spurious `K: Default` bound).
- [x] `slot()` = today's `slot_for` body (firing-wins); `snapshot()` = today's
      `chord_slots` body (toggle-wins, no cache); `perform()` = today's
      `perform_trigger` body with `compile_action` still inside it. `perform`'s
      tail is a `trigger::PerformDeps<'a>` borrow struct built at each call site
      (no macro).
- [x] `force_release` / `stop_toggle` / `stop_all_toggles` = the
      `trigger::force_release_stuck` / `trigger::stop_toggle` /
      `dispatch::stop_all_toggles` bodies verbatim, now methods. `stop_toggle`
      drops the never-needed `&Injector` the ticket sketch showed (`ActiveToggle::stop`
      carries its own); `force_release` is `&self` (it never removes).
- [x] `trigger_ctx!`, `TriggerCtx<'a, K>`, the free `perform_trigger<K>`, the
      free `chord_slots` / `slot_for`, the `ChordRuntime` struct, the free
      `trigger::force_release_stuck` / `trigger::stop_toggle`, the free
      `dispatch::stop_all_toggles` — all deleted. `compile_action` moved
      `dispatch` → `trigger` (a `trigger` → `dispatch` dep would be backwards;
      it needs only `executor` + `stepper`, both already `trigger` deps).
- [x] `DispatchState`: `individual: Slots<Input>` + `chord_slots:
      Slots<ChordKey>`; `DispatchState::new` builds both with
      `Slots::default()`; the `axis::Engine`-parallel doc comment relocated
      onto the `chord_slots` field.
- [x] All `dispatch.rs` call sites retargeted (`handle_event` toggle-stop,
      the chord snapshot, the 3 decide+perform sites, `run_chord_effects`,
      `run_effects` `StopToggle`/`StopAllToggles`, `Command::StopAllToggles`,
      `GetState` `active_toggles`, the test-harness `finish()`).
- [x] `chord::feed` signature unchanged; the one changed line is
      `let live = self.chord_slots.snapshot();`.
- [x] Cross-file doc comments naming the deleted free functions / struct
      updated (`chord.rs` ×4, `trigger.rs` module doc + `Slot` doc,
      `executor.rs` ×3, `analog_repeat.rs` ×4, `edit.rs`).
- [x] New `trigger::slots` unit tests (6): the both-maps-populated tie-break
      table (`slot` vs `snapshot`, all five slot states), `perform` over the
      decision table (asserted through `slot()` / `snapshot()`, not the private
      maps), `perform` is unconditional (fires on top of a Toggle), `perform`
      force-release keeps the entry, `stop_toggle` presence/removal,
      `stop_all_toggles` drain. No synthetic-dispatch map-assertion tests
      existed to delete (only the harness `finish()`, retargeted).
- [x] `tartarus-dual-stage-keys/map.md` Decisions-so-far + ticket 01: pointer
      note to ticket 15 / `Slots<StageKey>`.
- [x] `.scratch/README.md` `post-release-development` line updated.
- [x] `CONTRIBUTING.md:183–185` — retargeted to `trigger::Slots::perform`;
      the two `compile_action` mentions at 206 / 218 retargeted to
      `trigger::compile_action`.
- [x] `cargo fmt --check`, `cargo clippy --all-targets -D warnings`, full
      daemon suite (405) and GUI suite (410) green.

## Comments

**2026-09-03** — Filed from the second-pass architecture review
(`research/architecture-review-2026-09-03b.html`, candidate 1 of 7 — the
carried-forward first-pass #3). Design tree settled over three grilling
rounds; see "Decisions from the grilling". The one force that moved this to
#1 in the second pass: `tartarus-dual-stage-keys` (charting) will add
`stage::Engine` as a third copy of the pair, so landing the seam first turns
a hand-rolled copy #4 into `Slots<StageKey>`. Not yet implemented.

**2026-09-04** — Implemented in one pass on `dev`. `trigger::Slots<K>` +
`PerformDeps` + `compile_action` now live in `trigger.rs` (net −143 lines in
`dispatch.rs`, the `trigger_ctx!` macro / `TriggerCtx` / `ChordRuntime` /
`slot_for` / `chord_slots` / `perform_trigger` / free `stop_all_toggles` all
gone). `DispatchState` holds `individual: Slots<Input>` + `chord_slots:
Slots<ChordKey>`. Two deviations from the ticket sketch, both toward
correctness: `compile_action` moved *into* `trigger` (calling
`crate::dispatch::compile_action` from `trigger` would be a backwards module
dep; `compile_action` needs only `executor` + `stepper`), and
`Slots::stop_toggle` takes no `&Injector` (the sketch showed one, but
`ActiveToggle::stop` needs none and an unused param fails `clippy -D
warnings`). Behaviour preserved: each moved body diffed line-for-line against
HEAD; both tie-breaks kept as two named methods; `compile_action` still runs
inside `perform` behind the cleared guard. Daemon suite 405 green (6 new
`trigger::slots` tests, the both-maps-populated tie-break now asserted rather
than prose), GUI suite 410 green, `fmt` + `clippy --all-targets -D warnings`
clean.

`/code-review` (Standards + Spec, parallel sub-agents) — no behaviour-change
findings; the moved bodies verified verbatim against HEAD and both deviations
judged "correct, not just acceptable". Two review nits fixed in a follow-up
pass: (1) the three retargeted `perform` call sites collapsed from a repeated
six-field `PerformDeps { … }` literal to a one-line `PerformDeps::new(&injector,
config, &mut stepper, lap)` constructor (Duplicated Code — the regression from
the deleted `trigger_ctx!` macro); (2) the `perform` decision-table test
re-pointed off the private `firings` / `toggles` maps onto the public `slot()`
/ `snapshot()` reads (CONTRIBUTING "never assert on private fields"), plus a
stale `perform_trigger` doc mention at `dispatch_individual_down` and a
too-narrow test noted by the Spec axis.
