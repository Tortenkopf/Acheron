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

**Status:** done

- [x] A new unit `Effect::AssertLighting` variant in the daemon's `edit` module,
      alongside `AssertStatusLeds`.
- [x] A new data-only `Edit::SetLighting { assignment: LightingAssignment,
      brightness: u8 }` variant. `plan`'s arm, modelled on `SetStatusLeds`: write
      both fields onto `active_profile_mut`, push `Effect::AssertLighting`
      **unconditionally** — no `target == active` gate, since every mutating
      D-Bus method is Profile-unscoped and the GUI always edits the active
      Profile.
- [x] `edit::plan`'s `Edit::SwitchProfile` arm appends `Effect::AssertLighting` to
      its existing effect list, alongside `AssertStatusLeds` (order between the
      two is irrelevant — independent writes serialised by the shared `led`
      task).
- [x] `run_effects` gets a sibling arm handling `AssertLighting` by calling the
      `push_lighting(&config)` helper from ticket 02 — `run_effects` (Profile
      switch, set-edit) and the `rx_connection` arm (connect) become the two call
      sites of that one helper, mirroring `push_status_leds`.
- [x] A D-Bus method `SetLighting(a{sv}, y) -> ()` on `com.acheron.Daemon` —
      the `a{sv}` is the tagged-dict encoding of `LightingAssignment`/
      `FixedEffect` (extending `action_to_dict`'s existing sum-type convention;
      `Colour` rides as a nested `(yyy)` byte-triple), the `y` is brightness.
      Built directly and `apply`'d, shaped like `set_default_actuation`.
- [x] **No** `GetState()` addition and **no** new signal — `GetConfig` already
      carries everything, and there is no on-device control that could change
      Lighting behind the Daemon's back (mirrors Status LEDs' own finding).
- [x] **No** `rules.py` change — no cross-field validation beyond what each future
      GUI widget's own range will enforce.
- [x] GUI mirror (ADR-0005), mechanical:
  - `daemon_client.py`: `set_lighting(self, assignment: dict, brightness: int)`
    calling `SetLighting` with an `(a{sv}y)` variant, plus the abstract-method
    stub in the `Protocol`.
  - `daemon_stub.py`: same signature — mutates
    `self._profiles[self._active_profile]`'s `lighting`/`brightness` and appends
    `("set_lighting", assignment, brightness)` to `self.calls`.
- [x] Tests:
  - `plan`: `Edit::SetLighting { .. }` sets `active_profile().lighting`/
    `.brightness` and returns exactly `[Effect::AssertLighting]`;
    `Edit::SwitchProfile`'s effect list now contains both `AssertStatusLeds` and
    `AssertLighting`.
  - dispatch decider through the `led` watch channel: a `SetLighting` edit and a
    `SwitchProfile` each push the expected `LightingState`; a burst of switches
    coalesces to the final state.
  - GUI: `daemon_stub` records `set_lighting` calls and updates the active
    Profile's stored state.

## Comments

Implemented as specced. `Edit::SetLighting`/`Effect::AssertLighting` land in
`daemon/src/edit.rs` beside `SetStatusLeds`/`AssertStatusLeds`; `SwitchProfile`
now pushes both Assert effects. `run_effects` routes `AssertLighting` to the
existing `push_lighting` helper from ticket 02. `SetLighting`'s D-Bus decode
side is new: `dbus/wire.rs` gains `lighting_assignment_from_dict`/
`fixed_effect_from_dict`/`breath_style_from_dict`/`colour_from_field` (plus
`get_u8`/`get_dict` helpers) — the decode counterparts of ticket 01's
encode-only `*_to_dict` functions. Per the ticket's explicit `(yyy)`
byte-triple call-out, `Colour` decodes from a raw 3-tuple on this one path,
deliberately asymmetric with `colour_to_dict`'s `a{sv}` shape on `GetConfig`'s
encode side — verified end-to-end by a real-D-Bus test that sends a `(yyy)`
colour and reads it back via `GetConfig`'s `a{sv}` shape.

GUI: `daemon_client.py`/`daemon_stub.py` gained `set_lighting` per spec.
Forked code review (~92k tokens) caught a real bug beyond the ticket's own
checklist: the literal `self._call("SetLighting", GLib.Variant("(a{sv}y)",
(assignment, brightness)))` the ticket specified crashes on any real call —
`GLib.Variant`'s format-string constructor requires every `v` (variant) slot
to already hold a `GLib.Variant`, not a plain dict/str (confirmed by direct
repro). Fixed by adding `wire.py::lighting_assignment_to_variant` (plus
`_fixed_effect_to_variant`/`_breath_style_to_variant`/`_colour_to_variant`
helpers), mirroring `action_to_variant`/`binding_to_variant`'s existing
boxing convention, and wiring it into `daemon_client.set_lighting`. The
Python-side in-memory Colour shape stays `{"r","g","b"}` (matching what
`GetConfig()` hands back, so "copy from Profile" needs no translation);
`_colour_to_variant` is the one place that translates to the wire's `(yyy)`
shape, right before the D-Bus call.

Declined: the reviewer's second finding, that `DaemonStub.set_lighting`
should mirror a decode/encode Colour-shape asymmetry and validate unknown
`"type"` tags / wrong-length `colours` arrays. Once the `wire.py` fix above
lands, the stub never sees `(yyy)` tuples at all — it only ever receives the
same `{"r","g","b"}`-shaped dicts `GetConfig()` returns, so there is no
asymmetry left to mirror. Wire-shape validation (as opposed to the
business-rule validation `_validate_binding_action`/`rules.py` already
mirror) has no precedent anywhere else in `daemon_stub.py` either — every
existing check mirrors a `config::validate` invariant, never a decode-layer
"unknown tag" rejection.

Daemon: 612 tests green (`cargo test`, `cargo clippy --all-targets -- -D
warnings`, `cargo fmt --check`). GUI: 575 tests green (`gui/.venv/bin/pytest
gui/tests`), up from 569 at the start of this ticket.
