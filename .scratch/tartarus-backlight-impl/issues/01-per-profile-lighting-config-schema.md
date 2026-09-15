# 01 — Per-Profile Lighting assignment in the config schema and on the wire

**What to build:** Every Profile gains a **Lighting assignment** — a `LightingAssignment`
sum type (`Off | FixedEffect | CustomLayout`) plus a sibling `brightness: u8` byte —
stored in `config.toml` as plain fields and surfaced through `GetConfig`. A user
editing `config.toml` by hand can set `[profiles.<name>.lighting]` (or leave it out
entirely) and have it round-trip. A user upgrading from a build without this
feature keeps their existing `config.toml` working unchanged, with every Profile
defaulting to `Off` at `brightness = 0`. No runtime or hardware behaviour changes
in this ticket — nothing drives the backlight yet.

Source of truth: [`spec.md`](../../tartarus-backlight/spec.md) §"Daemon architecture"
(the `LightingAssignment`/`FixedEffect`/`BreathStyle`/`WaveDirection`/`Colour` types)
and §"Config schema".

**Blocked by:** None — can start immediately.

**Status:** done

- [x] `LightingAssignment { Off | FixedEffect { effect: FixedEffect } | CustomLayout
      { colours: [Colour; 21] } }`, `FixedEffect { Static | Spectrum | Reactive |
      Wave | Breath | Starlight }` (with `speed`/`colour`/`direction`/`style` fields
      per the spec's type listing), `BreathStyle { Random | Single | Dual }`,
      `WaveDirection { Left | Right }`, and `Colour { r: u8, g: u8, b: u8 }` land in
      the daemon config module exactly as typed in the spec — named structs/enums
      throughout, never a tuple or hex string, so a future per-variant field lands
      additively. `WaveDirection` is its own 2-variant type, **not** a reuse of
      `daemon/src/input.rs`'s existing 4-variant `Direction`.
- [x] `#[serde(tag = "type", rename_all = "snake_case")]` on `LightingAssignment` and
      `FixedEffect`, `#[serde(tag = "style", rename_all = "snake_case")]` on
      `BreathStyle` — matching this file's existing tagging precedent for
      data-carrying enums (`Action`).
- [x] `Profile` gains `#[serde(default)] lighting: LightingAssignment` and
      `#[serde(default)] brightness: u8`, both **without** `skip_serializing_if` —
      always written back in full, like `default_actuation` and `mode_key_role`.
      `LightingAssignment` derives `Default` to `Off`.
- [x] `config::SCHEMA_VERSION` stays unchanged; no migration machinery is added. A
      pre-feature `config.toml` (no `lighting`/`brightness` keys) parses with every
      Profile's `lighting == LightingAssignment::Off` and `brightness == 0` — the
      serde default *is* the migration, exactly as every prior additive `Profile`
      field (`status_leds`, `chords_base`, `axis_base`, `deep_stages`).
- [x] No `config::validate` rule beyond type bounds — every `LightingAssignment`
      variant and every `u8` brightness value is structurally valid; speed/colour
      ranges are enforced at the type level (`u8` already bounds them).
- [x] New parse test mirroring `a_pre_status_led_config_defaults_status_leds`: a
      minimal `schema_version = 1` file with a Profile that has only `base` set
      must parse with `profile.lighting == LightingAssignment::Off` and
      `profile.brightness == 0`.
- [x] `dbus/wire.rs::profile_to_dict` gains a `"lighting"` entry (a nested tagged
      dict, following `Action`'s existing convention extended to
      `LightingAssignment`/`FixedEffect`) and a `"brightness"` entry (a plain byte),
      following `status_leds`'s precedent. A `config_to_dict` test asserts both
      entries for each `LightingAssignment` variant.
- [x] The GUI's `DaemonStub` seed Profile dict gains `"lighting": {"type": "off"},
      "brightness": 0` so stub-backed GUI code sees the same shape a real
      `GetConfig` returns.
- [x] `rules.py` gets nothing — no cross-field invariant to mirror.

## Comments

Implemented as specced. `LightingAssignment`/`FixedEffect`/`BreathStyle`/
`WaveDirection`/`Colour` land in `daemon/src/config.rs` beside `StatusLeds`;
`profile_to_dict` (`daemon/src/dbus/wire.rs`) gains `lighting`/`brightness`
following `status_leds_to_dict`'s pattern, encode-only (no `SetLighting`
decode path yet — that's ticket 03). `daemon/src/edit.rs`'s
`reconcile_teardowns` doc comment's "ignored fields" list was extended to
name `lighting`/`brightness` for accuracy (no behaviour change — the
function was already field-selective and never touched them).

Code review (forked, ~93k tokens) flagged one real gap beyond the ticket's
own checklist: `gui/acheron_gui/device_overview.py`'s `PLACEHOLDER_CONFIG`
carries an explicit "mirror `DaemonStub._SEED_PROFILE`" invariant in its own
comment, and was missing the new `lighting`/`brightness` keys — latent today
(nothing reads them yet) but a launch-time `KeyError` waiting for ticket 04's
Lighting tab. Fixed by adding both keys there too.

Daemon: 583 tests green (`cargo test`, `cargo clippy`, `cargo fmt --check`).
GUI: 569 tests green (`gui/.venv/bin/pytest gui/tests`).
