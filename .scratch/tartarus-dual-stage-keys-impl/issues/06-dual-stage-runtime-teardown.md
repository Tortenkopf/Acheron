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

**Status:** ready-for-agent

- [ ] **Layer switch mid-press:** `handle_layer_switch` already calls
      `stage::Engine::stop_all()` (from ticket 03) — verify/extend so it
      force-releases **both** stages, deliberately stricter than the plain-key
      precedent (which live-rebinds instead of releasing). The other Layer's stage
      for that Input, if it has one, always starts fresh from Up on the next
      physical crossing.
- [ ] **Profile switch mid-press:** a new `Effect::StopAllStages` (unit variant,
      alongside `Effect::StopAllToggles`/`Effect::StopAllAnalogRepeats`) in the
      `edit` module, calling `stage::Engine::stop_all()` from `run_effects`.
      `Edit::SwitchProfile`'s arm in `plan` appends it to the existing effect list.
      A Profile switch fired by the deep stage's own Action completes that firing
      (via `commit_input_edits`, the same path the Chord-member-fires-
      `ProfileSwitch` precedent already uses) *before* the resulting
      `Edit::SwitchProfile` applies and tears the stage state down.
- [ ] **Capture-mode flip to Digital mid-press:** extend the existing
      `handle_capture_mode_change` Digital-transition branch (which already calls
      `analog_repeat.stop_all()` there) with `state.stage.stop_all()` too. After
      release, the primary is untouched by the flip and simply inherits whatever
      Digital-sourced Down/Repeat/Up the key produces next.
- [ ] **GUI-focus `StopAllToggles`:** the `Command::StopAllToggles` handler is
      extended to also drain `Slots<StageKey>`'s toggles alongside the individual
      path's — deliberately more aggressive than the Chord-toggle-survives-a-
      Profile-switch precedent, since it's a manual, not automatic, teardown.
- [ ] **Cascade-delete:** `edit::apply` pushes a new per-key `Effect::StopStage
      (Input)` whenever an edit removes a primary Binding that had a live
      `deep_base`/`deep_held` entry — check this at the `SetBinding` (overwrite) and
      `ClearBinding` (removal) arms of `plan`, cascading the config-level
      `deep_base`/`deep_held` entry away (but **not** `deep_stages` — per spec.md,
      "legal with no matching `deep_base`/`deep_held` entry" means an unused, inert
      deep-stage config is fine, so only the deep Binding is dropped, never the
      Actuation/mode config). `run_effects` calls the same per-key teardown method
      the other cases above use — force-releases **immediately**, not on the next
      Up. **No GUI confirmation dialog** (verified precedent: no Binding delete
      anywhere in the editor is confirmed today).
- [ ] `daemon_stub.py`'s existing `set_binding`/`clear_binding` mutation mirrors
      this same cascade — clearing/overwriting a primary Binding with a live
      `deep_base`/`deep_held` entry on that Input/Layer drops the deep Binding too,
      so GUI tests against the stub see the same behavior the real daemon does.
- [ ] **Reconnect / stuck-key reset:** a new, explicit disconnect hook in
      `handle_connection_change`'s `connected == false` branch — force-releases
      every live deep slot and resets `stage::Engine`'s per-key deep-band
      `KeyState` + Quick-Skip runtime state. Independent of capture's own
      synthetic-Up trick for the primary band (which only ever covers the primary).
      This is new machinery — no generic disconnect hook exists anywhere in the
      daemon today (confirmed: Analog-repeat has none either; that pre-existing gap
      is out of scope, not fixed here).
- [ ] **Chord-window proximity:** confirm by inspection (no code change expected)
      that `chord::feed` only diverts an event when the Input is an actual Chord
      member, and a dual-stage key can never be one — so a dual-stage key's events
      never enter the Chord window machinery.
- [ ] Tests (against the `stage::Engine`/`DispatchState` seam, following the
      Status-LEDs `led`-task-channel precedent of testing dispatch's decider logic
      through the channel rather than the real ioctl): Layer switch, Profile
      switch, the Digital-mode flip, and the new disconnect hook each force-release
      every live deep slot and reset Quick-Skip runtime state mid-press; a Profile
      switch fired by the deep stage's own Action completes before teardown;
      cascade-delete (`SetBinding` overwrite and `ClearBinding`) pushes
      `Effect::StopStage` and force-releases immediately; `Command::StopAllToggles`
      drains deep Toggles too.
