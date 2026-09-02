# 01 — Per-Profile `status_leds` in the config schema and on the wire

**What to build:** Every Profile gains a **Status LED assignment** — a triple of
independent on/off states for the orange, green, and blue Status LEDs — stored in
`config.toml` as a plain named table and surfaced through `GetConfig`. A user
editing `config.toml` by hand can add `[profiles.<name>.status_leds]` with any of
`orange` / `green` / `blue` and have it round-trip. A user upgrading from a build
without this feature keeps their existing `config.toml` working unchanged, with
every Profile defaulting to all-off. No runtime or hardware behaviour changes in
this ticket — nothing drives the LEDs yet.

Source of truth: [`spec.md`](../../tartarus-status-leds/spec.md) §"Config schema".

**Blocked by:** None — can start immediately.

**Status:** done

- [x] A named `StatusLeds` type sits beside `ActuationPoint` in the daemon config
      module: three `bool` fields `orange` / `green` / `blue`, `Default` deriving
      to all-`false`, each field independently `#[serde(default)]` so a partial
      hand-edited inline table still parses. A named struct, never `[bool; 3]` or
      a tuple — a later brightness byte or sibling sub-struct must be an additive
      change (spec §"Config schema", §"Further Notes").
- [x] `Profile` gains a `status_leds` field, `#[serde(default)]` **without**
      `skip_serializing_if` — always written back in full, like `default_actuation`
      and `mode_key_role`. `toml::to_string_pretty` renders it as a
      `[profiles.<name>.status_leds]` sub-table with all three keys spelled out.
- [x] `config::SCHEMA_VERSION` stays `1`; no migration machinery is added. A
      pre-feature `config.toml` (no `status_leds` key) parses with every Profile's
      `status_leds == StatusLeds::default()` — the serde default *is* the
      migration, exactly as every prior added field.
- [x] No `config::validate` rule — every `(bool, bool, bool)` is structurally
      valid.
- [x] New parse test, mirroring the existing pre-ticket-17 actuation-defaults
      test: a minimal `schema_version = 1` file with a Profile that has only its
      base map set must parse with `profile.status_leds == StatusLeds::default()`.
- [x] The daemon's `profile_to_dict` (D-Bus `wire`) gains a `status_leds` entry —
      a nested dict `{ orange: bool, green: bool, blue: bool }`, following
      `actuation_point_to_dict`. A `config_to_dict` test asserts the three keys,
      mirroring the existing default-actuation serialization test.
- [x] The GUI's `DaemonStub` seed Profile dict gains
      `"status_leds": {"orange": False, "green": False, "blue": False}` so
      stub-backed GUI code sees the same shape a real `GetConfig` returns. (The
      GUI reads `GetConfig` replies via native `GLib.Variant.unpack()`, so no
      encode-direction `wire.py` change is needed; confirm the config dict the GUI
      consumes carries `status_leds` per Profile.)
- [x] `rules.py` gets nothing — "three bools, always valid" has no invariant to
      mirror.
