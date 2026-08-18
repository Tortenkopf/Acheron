Type: grilling
Status: resolved

## Question

Decide whether `GetState()`'s D-Bus surface should move off a positional tuple onto a keyed
dict (matching how `GetConfig()` already returns a dict), or stay positional with a
discipline to prevent the failure mode below from recurring.

Surfaced by [ticket 22](./22-task-build-analog-capture-source.md)'s code review, not acted on
there as a drive-by: the positional tuple already broke a real client once. When
[ticket 21](./21-task-apply-analog-data-model-to-code.md) added `capture_mode` as a fifth
tuple element, `gui/acheron_gui/app.py`'s `rebuild()` unpacked it positionally and raised an
uncaught `ValueError` on every refresh — caught only by that ticket's own code review, not by
the type system or a test that would fail the same way in CI. Every remaining
Binding-editor-facing ticket on this map (Chord, mouse-button output, Stepper, Profile Switch,
reusable Macro entities, plus [ticket 19](./19-prototype-trigger-point-ux-and-live-depth.md)'s
live-depth work) is a candidate to grow `GetState()` again the same way.

## Settle at least

- Keyed dict vs. positional tuple for `GetState()` — weigh against `GetConfig()`'s existing
  dict precedent and whatever `SetBinding`/the rest of the D-Bus surface already establishes
  as this codebase's convention.
- If staying positional: what discipline (a shared test, a doc comment, a changelog note)
  would have caught the ticket 21 regression before code review did, rather than after.
- Migration cost: does changing shape now mean touching every existing `GetState()` call site
  (`daemon_client.py`, `daemon_stub.py`, `app.py`, GUI tests, daemon-side D-Bus tests) in one
  ticket, or can it land incrementally.
- Whether this is worth doing before v1.0 given more growth is already expected, or an
  acceptable fast-follow given the required floor doesn't block on it.

## Answer

**`GetState()` moves to a keyed dict** (`HashMap<String, OwnedValue>` / D-Bus
`a{sv}`), matching `GetConfig()`'s existing wire convention, and built now —
the same session, not a follow-up task ticket.

- **Why a dict, not a discipline on the tuple**: the ticket-21 regression
  happened because Python's positional unpack assumed a fixed arity; only a
  keyed shape actually survives a new field being added, since old code
  reading `state["profile"]` doesn't care what else showed up. A tuple can
  only be made *less likely* to break this way (a test, a comment); a dict
  makes the whole bug class structurally impossible. Every remaining
  Binding-editor-facing ticket (Chord, mouse-button output, Stepper, Profile
  Switch, reusable Macros, ticket 19's live-depth work) is a plausible
  candidate to grow `GetState()` again, so this was worth fixing at the root
  rather than re-litigating per ticket.
- **Built now, not deferred**: unlike most remaining tickets, this needed no
  hardware and no further judgment calls — a small, mechanical migration
  directly mirroring `config_to_dict`'s already-proven pattern. Waiting would
  only let the tuple (and the migration) keep growing.
- **What shipped**: a new `wire::state_to_dict()` (`daemon/src/dbus/wire.rs`)
  mirrors `config_to_dict`'s shape for `command::State`'s five flat scalar
  fields; `Daemon::get_state()` (`daemon/src/dbus/mod.rs`) now returns
  `HashMap<String, OwnedValue>` via it. `command::State` and `dispatch.rs`
  needed zero changes — they already used named fields internally, since the
  positional-ness was purely a wire-encoding concern. Python:
  `daemon_client.py`'s `DBusDaemonClient.get_state()` and the `DaemonClient`
  Protocol now return `dict`; `daemon_stub.py`'s stub returns the same keyed
  shape; `app.py`'s `rebuild()` reads `state["profile"]`/`["layer"]`/
  `["device_connected"]` instead of positional-unpacking five values.
  `gui/acheron_gui/wire.py` needed no changes — `GLib.Variant.unpack()`
  already turns a dict reply into a plain Python dict for free.
- **Test migration**: 12 sites in `daemon/src/dbus/mod.rs`'s own D-Bus test
  module (the `DaemonProxy` trait signature plus 11 tuple-destructures
  across 7 tests) plus 14 sites across `gui/tests/test_daemon_stub.py` and
  `gui/tests/test_device_overview.py`, all the same mechanical swap
  (positional index/unpack → dict key). Added one new unit test,
  `state_to_dict_keys_every_field_by_name`, mirroring
  `config_to_dict_nests_profiles_layers_and_bindings`'s coverage style.
- **Verified**: all 171 Rust tests and 70 Python tests pass; `cargo clippy`
  clean; `cargo fmt --check` shows no new drift in any touched file.
