# 14 — Config file + minimal domain model + Base-layer Fire-once Keypress remapping

**What to build:** The Daemon reads a TOML config file and applies real remaps for simple, single-press bindings — the first user-visible "I typed X and got Y" behavior. Scope is deliberately narrow: one Profile (`Default`), Base Layer only, `Action::Keypress` only, `TriggerMode::FireOnce` only. See `.scratch/tartarus-keybinder/spec.md` ("Domain model (Daemon, Rust)" and "Config lifecycle") for the full design — this ticket implements a subset of it, not the whole domain model.

**Blocked by:** 13

**Status:** ready-for-agent

- [ ] `Input` composite enum (`ModeKey`, `Grid(row, col)`, `Thumbstick(Direction)`, `Wheel(WheelEvent)`) exists with a custom `Display`/`FromStr` serializing to the flat snake_case strings (`mode_key`, `grid_r1c1`, `thumbstick_up`, `wheel_scroll_up`, `wheel_middle`, …) matching ticket 01's table exactly.
- [ ] `Binding`, `Action::Keypress { modifiers, key }`, and `TriggerMode::FireOnce` types exist (other `Action`/`TriggerMode` variants can exist as stubs but don't need to function yet).
- [ ] Config is a single TOML file at `~/.config/acheron/config.toml` with a top-level `schema_version = 1`.
- [ ] A Layer's Bindings are a sparse map keyed by `Input`; an absent entry means passthrough (unchanged from ticket 13's behavior).
- [ ] If `config.toml` is missing, the Daemon creates `~/.config/acheron/` and the file itself on startup, writing it immediately (not lazily) — seed content: one Profile named `Default`, `schema_version = 1`, empty Base-layer Binding map, set active.
- [ ] If `config.toml` fails to parse or has an unsupported `schema_version`, the Daemon refuses to start: exits non-zero with a clear parse error to the journal/stderr, and does not overwrite or back up the file.
- [ ] On startup with a valid config, the dispatch task resolves each `PhysicalEvent`'s `Input` against the `Default` Profile's Base Layer: a configured Binding fires its `Keypress` action on `Down` only (ignoring `Repeat`/`Up`); an absent Binding passes through unchanged (ticket 13 behavior).
- [ ] Live-hardware demo: hand-edit `config.toml` to bind one grid key to a different Keypress, restart the Daemon, press the physical key, and the remapped key (not the original) appears in a real text field.
- [ ] Automated tests (via ticket 13's fake `CaptureSource`) cover: passthrough when unbound, remap when bound, seed-on-missing-file, and refuse-to-start-on-corrupt-file.
