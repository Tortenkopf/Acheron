Type: task
Status: resolved
Blocked by: 18

## Question

Apply [ticket 17](./17-decide-analog-data-model.md)'s decided data model to the actual code.
Ticket 17 was a pure design decision — nothing in `config.rs`/`command.rs`/`dbus/mod.rs`
reflects it yet. This ticket is purely mechanical/AFK: no physical device is needed, and the
existing 72+28 test suites (daemon + GUI) plus new unit tests are the complete verification —
there is nothing here that requires live hardware. Read ticket 17's `## Answer` in full before
starting; it's the spec this ticket implements.

Ticket [22](./22-task-build-analog-capture-source.md) (the real analog `CaptureSource`) and
ticket [23](./23-task-wire-analog-supervisor-and-install.md) (live source-swap in `main.rs`)
both build on top of what this ticket lands — do this one first.

## Do this

**`daemon/src/config.rs`**:

- Add `ActuationPoint { actuation: u8, release: u8 }` (`Debug, Clone, Copy, PartialEq, Eq,
  Serialize, Deserialize`), `Default` impl returning `{ actuation: 128, release: 112 }`
  (ticket 17 §5's explicit placeholder, not tuned).
- Add to `Profile`: `#[serde(default)] pub default_actuation: ActuationPoint` and
  `#[serde(default, skip_serializing_if = "HashMap::is_empty")] pub actuation_overrides:
  HashMap<Input, ActuationPoint>`.
- Add to `Config`: `#[serde(default)] pub force_digital: bool`.
- No `SCHEMA_VERSION` bump (ticket 17 §6 — everything here is additive/`#[serde(default)]`).
  Add a config.rs test mirroring `a_profile_missing_held_and_mode_key_role_defaults_both` /
  `held_layer_bindings_survive_a_full_write_and_reparse_round_trip` for the new fields, so a
  pre-ticket-17 `config.toml` is proven to still load unchanged.

**`daemon/src/capture/mod.rs`**:

- Widen `PhysicalEvent` with `pub depth: Option<u8>`. This breaks every existing
  `PhysicalEvent { .. }` construction site — fix each one with `depth: None` (evdev-sourced
  events always carry `None`, per ticket 17 §1; there is no analog source yet for this ticket
  to set `Some`). Expect to touch `capture/fake.rs`, `capture/evdev_source.rs`,
  `dispatch.rs`'s tests, and `injector.rs`'s tests — run `cargo build` and `cargo test` and fix
  every resulting error/failure rather than guessing the site list up front.

**`daemon/src/command.rs`**:

- Add `capture_mode: &'static str` to `State` (`"analog"` / `"digital"`). For *this* ticket,
  dispatch has no real analog source to report on yet — hardcode it to always report
  `"digital"` in `handle_command`'s `GetState` arm, with a doc comment pointing at ticket 22/23
  as where this becomes genuinely live. Don't build any live-tracking plumbing for it now —
  that's ticket 22 §5's watch-channel/capture-side job, not this ticket's.
- Add five new `Command` variants, each following `SetBinding`'s existing shape (a payload
  plus `reply: oneshot::Sender<Result<(), CommandError>>`):
  - `SetActuationPoint { input: Input, actuation: u8, release: u8, reply }`
  - `ClearActuationPoint { input: Input, reply }`
  - `SetDefaultActuation { actuation: u8, release: u8, reply }`
  - `ResetActuationPoints { reply }`
  - `SetForceDigital { force: bool, reply }`

**`daemon/src/dispatch.rs`**:

- `handle_command` grows a match arm per new `Command`, all active-Profile-scoped and
  persisted via the existing `persist()` helper with the same rollback-on-write-failure
  pattern `SetBinding`/`SetModeKeyRole` already use.
- `SetActuationPoint`/`ClearActuationPoint`: reject a non-`Grid` `input` — return
  `CommandError::InvalidRequest("...")` (ticket 17 §3 — `Daemon::parse_input`'s
  `dbus/mod.rs:98` is the existing precedent for this class of validation, but the *input
  variant* check belongs here since it's Command-level, not wire-parsing). Also reject
  `release > actuation` the same way.
- `SetForceDigital`: for this ticket, just persists the flag. Actually swapping the live
  capture source on this call is ticket 23's job — leave a doc comment saying so rather than
  building a stub.
- `ResetActuationPoints`: clears the active Profile's `actuation_overrides` in one call/one
  `config.toml` rewrite (ticket 17 §7 — the GUI's "reset all keys" button, not 20 individual
  `ClearActuationPoint` calls).

**`daemon/src/dbus/mod.rs`**:

- Five new D-Bus methods mirroring `set_binding`/`set_output_suppressed`'s existing shape
  exactly (parse wire args via `wire::` helpers where needed — `Input`'s `FromStr` already
  exists in `input.rs` and doubles as the wire parser, see `Daemon::parse_input`), each
  building the matching `Command` and awaiting its `oneshot` reply.
- Add `capture_mode: String` to `GetState()`'s returned dict.
- Add a `CaptureModeChanged(mode: String)` signal, mirroring `device_connection_changed`
  (`dbus/mod.rs:435`) exactly. Nothing fires it yet in this ticket (dispatch's `capture_mode`
  is hardcoded) — the signal only needs to exist and be wired into the `#[interface]`
  trait/proxy blocks (mirroring lines ~410-481's pattern) so ticket 23 can call it without
  touching this file again.
- Add tests mirroring the existing `set_binding_over_real_dbus_...` family: each new method
  succeeds over a real D-Bus round-trip, persists to `config.toml`, and the
  non-`Grid`-input/`release > actuation` rejections surface as
  `com.acheron.Daemon.Error.InvalidBinding` (same pattern as
  `set_binding_over_real_dbus_with_an_invalid_input_string_is_rejected`, `dbus/mod.rs:715`).

**`CONTEXT.md`**: already has Depth/Actuation point/Release point/Capture mode entries from
ticket 17's own resolution — check they still match what actually got built (they should; flag
here if not) rather than assuming.

## Answer

Landed exactly as ticket 17's `## Answer` specified, mechanically, no hardware involved.
Daemon test count went from 109 to 128 (19 new tests); `cargo build`/`cargo test`/`cargo
clippy --all-targets`/`cargo fmt` all clean.

### `daemon/src/config.rs`

`ActuationPoint { actuation: u8, release: u8 }` with its `Default` (128/112), plus
`Profile::default_actuation`/`actuation_overrides` and `Config::force_digital`, all
`#[serde(default)]` — no `SCHEMA_VERSION` bump. New test
`a_pre_ticket_17_config_defaults_actuation_fields_and_force_digital` proves a config.toml
with none of the three fields still parses, defaulting all of them.

### `daemon/src/capture/mod.rs`

`PhysicalEvent` widened with `pub depth: Option<u8>`. Rather than hand-hunting every
construction site, followed the ticket's own advice: added the field, ran `cargo build`/
`cargo test`, and fixed each of the resulting 33 compile errors as they surfaced —
`evdev_source.rs`'s two real sites got `depth: None`; the other 31 (test-only, across
`fake.rs`, `injector.rs`, `dispatch.rs`, `dbus/mod.rs`) got the same via a scripted edit,
since they were all one of two uniform shapes.

### `daemon/src/command.rs`

`State.capture_mode: &'static str` added. Five new `Command` variants
(`SetActuationPoint`/`ClearActuationPoint`/`SetDefaultActuation`/`ResetActuationPoints`/
`SetForceDigital`), each following `SetBinding`'s `reply: oneshot::Sender<...>` shape.

### `daemon/src/dispatch.rs`

`GetState`'s arm hardcodes `capture_mode: "digital"` with a doc comment pointing at tickets
22/23. Each new `Command` gets a match arm using the existing rollback-on-persist-failure
pattern. `SetActuationPoint`/`ClearActuationPoint` reject a non-`Grid` `input`;
`SetActuationPoint` and `SetDefaultActuation` both reject `release > actuation` — extended
past ticket 21's literal text (which only spells the check out for `SetActuationPoint`) to
`SetDefaultActuation` too, since it's the same hysteresis invariant on the same
`ActuationPoint` shape and leaving it unchecked there would let the invalid state back in
through the one door `SetActuationPoint` closes. `ResetActuationPoints` clears
`actuation_overrides` in one call; `SetForceDigital` persists only, per its doc comment
pointing at ticket 23.

### `daemon/src/dbus/mod.rs`

Five new D-Bus methods mirroring `SetBinding`/`SetOutputSuppressed`'s shape, `capture_mode:
String` added to `GetState()`'s returned tuple, and `CaptureModeChanged(mode: String)` added
to the `#[interface]` trait/proxy blocks (unfired, mirroring `DeviceConnectionChanged`
exactly) so ticket 23 doesn't need to touch this file again. New D-Bus round-trip tests
mirror the `set_binding_over_real_dbus_...` family — each method succeeds over a real
connection and persists, and the non-`Grid`/`release > actuation` rejections surface as
`com.acheron.Daemon.Error.InvalidBinding`. `GetConfig()`'s wire dict was deliberately left
untouched (out of this ticket's stated scope — only `dbus/mod.rs`'s bullet was named, not
`wire.rs`), so the clear/reset tests assert against `config.toml` on disk instead of
`GetConfig()`'s dict, which doesn't carry actuation fields.

### `CONTEXT.md`

Checked against what actually got built: matches (Depth/Actuation point/Release point/
Capture mode entries were already accurate from ticket 17's own resolution).

### Deliberately out of scope

`GetConfig()`'s wire dict does not yet surface `default_actuation`/`actuation_overrides`;
whichever ticket wires the GUI's actuation-point editor will need to add that.

### Code review findings, all fixed

`/code-review` caught one real regression and two quality nits, all addressed before
landing:

- **`GetState()`'s arity change broke the real GUI.** `gui/acheron_gui/app.py`'s
  `rebuild()` unpacked a hardcoded 4-tuple; against the now-5-tuple `GetState()` this raises
  an uncaught `ValueError` on every refresh — not a `DaemonError`/`GLib.Error`, so the
  existing `except` clause doesn't catch it. Fixed by threading `capture_mode` through
  `daemon_client.py`'s `Protocol`/`DBusDaemonClient`, `daemon_stub.py` (hardcoded
  `"digital"`, mirroring the daemon's own ticket 21 stand-in), `app.py`'s unpacking, and the
  two GUI test files' `get_state()` call sites. This was flagged as out of scope above
  before review; keeping GUI callers from crashing against a wire-contract change this
  ticket itself makes isn't optional the way GUI *feature* work is.
- **Duplicated validation.** The non-`Grid`-input and `release > actuation` checks were
  copy-pasted across `SetActuationPoint`/`ClearActuationPoint`/`SetDefaultActuation`.
  Extracted `reject_non_grid_input`/`reject_release_above_actuation` in `dispatch.rs`.
- **Misleading `ResetActuationPoints` doc comment.** Said "Never fails," copied from
  `StopAllToggles`'s genuinely-infallible doc — but `ResetActuationPoints` does call
  `persist()` and can return `IoError` on a disk-write failure. Corrected in both
  `command.rs` and `dbus/mod.rs`.

Daemon: still 128/128 passing, `cargo clippy --all-targets`/`cargo fmt` clean on every
touched file. GUI: no test runner available in this environment to execute
`gui/tests/`, but every edited file passes `python3 -m py_compile`; the changes are
narrow (tuple arity + one hardcoded field) and mirror the existing stub/client pattern
exactly.
