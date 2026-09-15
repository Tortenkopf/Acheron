# 03 — `SetLighting` D-Bus edit and Profile-switch assertion

**What to build:** A caller can set the active Profile's Lighting assignment and
brightness over D-Bus in one call, and the change is persisted to `config.toml`
and driven to the hardware immediately. Switching Profile re-asserts the newly
active Profile's Lighting alongside its Status LEDs, so the backlight always
follows the active Profile deterministically. The GUI's D-Bus client and stub
gain the matching method so the next tickets can wire the UI. No GUI widgets in
this ticket.

Source of truth: [`spec.md`](../../tartarus-backlight/spec.md) §"Daemon
architecture" (Dispatch wiring) and §"D-Bus surface".

**Blocked by:** 01, 02

**Status:** ready-for-agent

- [ ] A new unit `Effect::AssertLighting` variant in the daemon's `edit` module,
      alongside `AssertStatusLeds`.
- [ ] A new data-only `Edit::SetLighting { assignment: LightingAssignment,
      brightness: u8 }` variant. `plan`'s arm, modelled on `SetStatusLeds`: write
      both fields onto `active_profile_mut`, push `Effect::AssertLighting`
      **unconditionally** — no `target == active` gate, since every mutating
      D-Bus method is Profile-unscoped and the GUI always edits the active
      Profile.
- [ ] `edit::plan`'s `Edit::SwitchProfile` arm appends `Effect::AssertLighting` to
      its existing effect list, alongside `AssertStatusLeds` (order between the
      two is irrelevant — independent writes serialised by the shared `led`
      task).
- [ ] `run_effects` gets a sibling arm handling `AssertLighting` by calling the
      `push_lighting(&config)` helper from ticket 02 — `run_effects` (Profile
      switch, set-edit) and the `rx_connection` arm (connect) become the two call
      sites of that one helper, mirroring `push_status_leds`.
- [ ] A D-Bus method `SetLighting(a{sv}, y) -> ()` on `com.acheron.Daemon` —
      the `a{sv}` is the tagged-dict encoding of `LightingAssignment`/
      `FixedEffect` (extending `action_to_dict`'s existing sum-type convention;
      `Colour` rides as a nested `(yyy)` byte-triple), the `y` is brightness.
      Built directly and `apply`'d, shaped like `set_default_actuation`.
- [ ] **No** `GetState()` addition and **no** new signal — `GetConfig` already
      carries everything, and there is no on-device control that could change
      Lighting behind the Daemon's back (mirrors Status LEDs' own finding).
- [ ] **No** `rules.py` change — no cross-field validation beyond what each future
      GUI widget's own range will enforce.
- [ ] GUI mirror (ADR-0005), mechanical:
  - `daemon_client.py`: `set_lighting(self, assignment: dict, brightness: int)`
    calling `SetLighting` with an `(a{sv}y)` variant, plus the abstract-method
    stub in the `Protocol`.
  - `daemon_stub.py`: same signature — mutates
    `self._profiles[self._active_profile]`'s `lighting`/`brightness` and appends
    `("set_lighting", assignment, brightness)` to `self.calls`.
- [ ] Tests:
  - `plan`: `Edit::SetLighting { .. }` sets `active_profile().lighting`/
    `.brightness` and returns exactly `[Effect::AssertLighting]`;
    `Edit::SwitchProfile`'s effect list now contains both `AssertStatusLeds` and
    `AssertLighting`.
  - dispatch decider through the `led` watch channel: a `SetLighting` edit and a
    `SwitchProfile` each push the expected `LightingState`; a burst of switches
    coalesces to the final state.
  - GUI: `daemon_stub` records `set_lighting` calls and updates the active
    Profile's stored state.
