# 06 — Dual-stage runtime teardown across Layer/Profile/connection changes

**What to build:** A live deep stage never survives a Layer switch, a Profile
switch, a flip to Digital Capture mode, deleting its primary Binding, or a device
replug — closing every gap spec.md's "Runtime behavior across Layer/Profile/
connection changes" section identifies. This is the last correctness ticket before
the GUI (07); everything here is invisible without deliberately provoking one of
these five transitions mid-press, verified by dispatch-harness tests.

Source of truth: [`spec.md`](../../tartarus-dual-stage-keys/spec.md) §"Runtime
behavior across Layer/Profile/connection changes".

**Blocked by:** 03, 04

**Status:** resolved

- [X] **Layer switch mid-press:** `handle_layer_switch` already calls
  `stage::Engine::stop_all()` (from ticket 03) — verify/extend so it
  force-releases **both** stages, deliberately stricter than the plain-key
  precedent (which live-rebinds instead of releasing). The other Layer's stage
  for that Input, if it has one, always starts fresh from Up on the next
  physical crossing.
- [X] **Profile switch mid-press:** a new `Effect::StopAllStages` (unit variant,
  alongside `Effect::StopAllToggles`/`Effect::StopAllAnalogRepeats`) in the
  `edit` module, calling `stage::Engine::stop_all()` from `run_effects`.
  `Edit::SwitchProfile`'s arm in `plan` appends it to the existing effect list.
  A Profile switch fired by the deep stage's own Action completes that firing
  (via `commit_input_edits`, the same path the Chord-member-fires-
  `ProfileSwitch` precedent already uses) *before* the resulting
  `Edit::SwitchProfile` applies and tears the stage state down.
- [X] **Capture-mode flip to Digital mid-press:** extend the existing
  `handle_capture_mode_change` Digital-transition branch (which already calls
  `analog_repeat.stop_all()` there) with `state.stage.stop_all()` too. After
  release, the primary is untouched by the flip and simply inherits whatever
  Digital-sourced Down/Repeat/Up the key produces next.
- [X] **GUI-focus `StopAllToggles`:** the `Command::StopAllToggles` handler is
  extended to also drain `Slots<StageKey>`'s toggles alongside the individual
  path's — deliberately more aggressive than the Chord-toggle-survives-a-
  Profile-switch precedent, since it's a manual, not automatic, teardown.
- [X] **Cascade-delete:** `edit::apply` pushes a new per-key `Effect::StopStage (Input)` whenever an edit removes a primary Binding that had a live
  `deep_base`/`deep_held` entry — check this at the `SetBinding` (overwrite) and
  `ClearBinding` (removal) arms of `plan`, cascading the config-level
  `deep_base`/`deep_held` entry away (but **not** `deep_stages` — per spec.md,
  "legal with no matching `deep_base`/`deep_held` entry" means an unused, inert
  deep-stage config is fine, so only the deep Binding is dropped, never the
  Actuation/mode config). `run_effects` calls the same per-key teardown method
  the other cases above use — force-releases **immediately**, not on the next
  Up. **No GUI confirmation dialog** (verified precedent: no Binding delete
  anywhere in the editor is confirmed today).
- [X] `daemon_stub.py`'s existing `set_binding`/`clear_binding` mutation mirrors
  this same cascade — clearing/overwriting a primary Binding with a live
  `deep_base`/`deep_held` entry on that Input/Layer drops the deep Binding too,
  so GUI tests against the stub see the same behavior the real daemon does.
- [X] **Reconnect / stuck-key reset:** a new, explicit disconnect hook in
  `handle_connection_change`'s `connected == false` branch — force-releases
  every live deep slot and resets `stage::Engine`'s per-key deep-band
  `KeyState` + Quick-Skip runtime state. Independent of capture's own
  synthetic-Up trick for the primary band (which only ever covers the primary).
  This is new machinery — no generic disconnect hook exists anywhere in the
  daemon today (confirmed: Analog-repeat has none either; that pre-existing gap
  is out of scope, not fixed here).
- [X] **Chord-window proximity:** confirm by inspection (no code change expected)
  that `chord::feed` only diverts an event when the Input is an actual Chord
  member, and a dual-stage key can never be one — so a dual-stage key's events
  never enter the Chord window machinery.
- [X] Tests (against the `stage::Engine`/`DispatchState` seam, following the
  Status-LEDs `led`-task-channel precedent of testing dispatch's decider logic
  through the channel rather than the real ioctl): Layer switch, Profile
  switch, the Digital-mode flip, and the new disconnect hook each force-release
  every live deep slot and reset Quick-Skip runtime state mid-press; a Profile
  switch fired by the deep stage's own Action completes before teardown;
  cascade-delete (`SetBinding` overwrite and `ClearBinding`) pushes
  `Effect::StopStage` and force-releases immediately; `Command::StopAllToggles`
  drains deep Toggles too.

## Answer

Every bullet above was either already wired (Layer switch / capture-mode flip,
from ticket 03) or added fresh:

- `edit::Effect::StopAllStages` joins `SwitchProfile`'s effect list
  (`daemon/src/edit.rs`); `run_effects` runs it via `stage::Engine::stop_all()`.
  Ordering is safe for free: `update_stages`/`begin_quick_skip` fully complete
  (and return the `Edit::SwitchProfile`) before `commit_input_edits` ever reaches
  `edit::apply`, so a deep stage's own `ProfileSwitch` Action always finishes
  firing before its own teardown runs.
- `edit::Effect::StopStage(Input)` is pushed by `SetBinding`'s overwrite arm and
  `ClearBinding`'s removal arm whenever `deep_layer_mut(layer).remove(&input)`
  finds a live entry — `deep_stages` is never touched. `run_effects` routes it to
  a new `stage::Engine::stop_stage(input, injector)`, scoped to one key (unlike
  `stop_all`'s whole-engine sweep) and removing the runtime entry outright rather
  than resetting it, since `Engine::update`'s own `deep_layer(...).contains_key`
  guard already skips this Input for good.
- `stage::Engine::stop_all_toggles()` (drains `Slots<StageKey>` only — no firing
  force-release, no `KeyState`/Quick-Skip reset) is wired into
  `Command::StopAllToggles`'s handler alongside `individual.stop_all_toggles()`.
- `handle_connection_change` gained `stage`/`injector` parameters and calls
  `stage.stop_all(injector)` on the `connected == false` transition only —
  independent of, and untouched by, `capture::analog`'s existing synthetic-Up
  trick for the primary band.
- `daemon_stub.py`'s `set_binding`/`clear_binding` both gained an unconditional
  `deep_{layer}.pop(input_str, None)` mirroring the cascade — harmless when
  nothing was there, matching the real Daemon's "presence is proof of an
  overwrite" reasoning (a deep Binding can't exist without a primary).
- Chord-window proximity: confirmed by inspection —
  `ConfigError::ChordMemberDeepStageConflict` (ticket 01) already makes a
  dual-stage key and a Chord member mutually exclusive at `validate` time, so
  `chord::feed` structurally never diverts one. No code change.

Tests: `daemon/src/edit.rs` gained unit tests for both cascade-delete arms
(`SetBinding` overwrite, `ClearBinding` removal) plus the widened
`switch_profile_sets_active_and_emits_its_ordered_effect_chain` assertion.
`daemon/src/dispatch.rs` gained a "ticket 06: runtime teardown" block: Profile
switch force-releasing a live deep Toggle, Quick-Skip cancellation on Profile
switch and on disconnect (alongside the pre-existing Layer-switch/capture-mode-
flip cases), a deep stage's own `ProfileSwitch` Action completing before its
teardown, cascade-delete via both `SetBinding` and `ClearBinding` force-releasing
immediately, `Command::StopAllToggles` draining a deep Toggle, and the disconnect
hook force-releasing a live deep Toggle. `gui/tests/test_daemon_stub.py` gained
matching cascade-delete coverage for the stub. 463 daemon + 434 GUI tests pass;
`cargo clippy --all-targets` and `cargo fmt --check` are clean.
