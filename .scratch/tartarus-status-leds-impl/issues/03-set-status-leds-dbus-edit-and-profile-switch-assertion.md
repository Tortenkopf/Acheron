# 03 — `SetStatusLeds` D-Bus edit and Profile-switch assertion

**What to build:** A caller can set the active Profile's Status LED assignment over
D-Bus in one call, and the change is persisted to `config.toml` and driven to the
hardware immediately. Switching Profile re-asserts the newly active Profile's
assignment, so the physical indicator always follows the active Profile
deterministically. The GUI's D-Bus client and stub gain the matching method so the
next ticket can wire the UI. No GUI widgets in this ticket.

Source of truth: [`spec.md`](../../tartarus-status-leds/spec.md) §"Daemon
architecture" (Decider), §"D-Bus surface".

**Blocked by:** 01, 02

**Status:** ready-for-agent

- [ ] A new unit `Effect::AssertStatusLeds` variant in the daemon's `edit` module,
      alongside `ReconcileStepperCursor` / `AnnounceProfileChange`.
- [ ] A new data-only `Edit::SetStatusLeds { orange: bool, green: bool, blue: bool }`
      variant. `plan`'s arm, modelled on `SetActuationPoint`: set
      `active_profile_mut(&mut next).status_leds` from the triple, push
      `Effect::AssertStatusLeds` **unconditionally**. Whole triple in one call —
      one frame drives all three channels, so no per-channel edit and no
      channel-name enum on the wire. `plan` still returns `Result` for symmetry
      with every persisting `Edit`; it never fails on its own account.
- [ ] `edit::plan`'s `Edit::SwitchProfile` arm appends `Effect::AssertStatusLeds`
      to its existing effect list (order irrelevant — the LEDs are independent of
      Toggles / axes / Analog-repeat).
- [ ] `run_effects` handles `AssertStatusLeds` by calling the
      `push_status_leds(&config)` helper from ticket 02 — which reads
      `config.active_profile().status_leds` and sends it on the `led` watch
      channel. `run_effects` (Profile switch, set-edit) and the `rx_connection`
      arm (connect) are the two call sites of the one helper.
- [ ] There is **no** non-active-Profile write path and **no** `target == active`
      gate: every mutating D-Bus method is Profile-unscoped and `plan` applies it
      to the active Profile; a Status-LEDs edit is structurally always an edit to
      the active Profile.
- [ ] A D-Bus method `SetStatusLeds(bbb) -> ()` on `com.acheron.Daemon`, shaped
      like `set_default_actuation`: build the `Edit` directly and `apply(...)`.
- [ ] **No** `GetState()` addition and **no** new signal. The active Profile's
      stored triple is fully visible via `GetConfig`; there is no
      hardware-divergence path (charting ticket 01 found no on-device keymap
      switch), and siblings like `SetActuationPoint` emit no signal — the GUI
      rebuilds from `GetConfig` after its own calls and on `ActiveProfileChanged`.
- [ ] GUI mirror (ADR-0005), mechanical:
  - `daemon_client.py`: `set_status_leds(self, orange, green, blue)` calling
    `SetStatusLeds` with a `(bbb)` variant, plus the abstract-method stub in the
    `Protocol`.
  - `daemon_stub.py`: same signature — mutates
    `self._profiles[self._active_profile]["status_leds"]` and appends
    `("set_status_leds", orange, green, blue)` to `self.calls`.
- [ ] Tests:
  - `plan`: `Edit::SetStatusLeds { .. }` sets `active_profile().status_leds` and
    returns exactly `[Effect::AssertStatusLeds]`; `Edit::SwitchProfile`'s effect
    list now contains `AssertStatusLeds`.
  - dispatch decider through the `led` watch channel: a `SetStatusLeds` edit and a
    `SwitchProfile` each push the expected triple; a burst of switches coalesces
    to the final triple.
