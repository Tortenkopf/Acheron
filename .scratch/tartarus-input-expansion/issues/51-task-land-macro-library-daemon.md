Type: task
Status: resolved
Blocked by: None — can start immediately

## Question

Land [Design reusable Macro entities](./15-decide-reusable-macro-entities.md)'s settled shape into the real Daemon/config, Daemon-side only — split off [Build the Stepper/Macro library UX for real](./41-task-build-stepper-macro-library-ux.md) per its own "split during resolution" allowance, following the same data-model-first pattern [ticket 18's split](./18-rework-capture-path-for-analog.md) used for [ticket 21](./21-task-apply-analog-data-model-to-code.md) (AFK, no hardware needed). This ticket is Daemon/config/D-Bus only — no GUI widgets. [Build the Macro library GUI](./52-task-build-macro-library-gui.md) is the paired follow-up that consumes this shape.

Scope:

- **Config**: `Config.macros: HashMap<MacroId, MacroDef>` (new top-level field, alongside `profiles`), `MacroDef { name: String, steps: Vec<MacroStepDto> }` — the step-sequence content moves here unchanged from today's inline `Action::Macro { steps }`. `MacroId` is a slug derived from `name` at creation (lowercase, hyphenated, numeric suffix on collision), frozen at creation and never rewritten by a rename.
- **Action cutover**: `Action::Macro { steps: Vec<MacroStepDto> }` becomes `Action::Macro { macro_id: MacroId }` — a Binding no longer carries step content directly, only a reference. No inline/unnamed Macro survives as a second representation. This is a breaking schema change, not an additive one (unlike the analog data model's `#[serde(default)]` precedent) — existing `config.toml` files with inline Macro Bindings are out of this ticket's scope to migrate; that gap is pre-existing in ticket 41's text and not resolved here.
- **D-Bus**: `CreateMacro`, `RenameMacro`, `DeleteMacro` (refuses with `CommandError::InvalidRequest` while any Binding anywhere — `base`/`held`/`chords_base`/`chords_held`, any Profile — still references its `MacroId`, mirroring `DeleteProfile`).
- **Cross-module plumbing, not GUI widgets**: per [ticket 21](./21-task-apply-analog-data-model-to-code.md)'s own precedent for exactly this kind of split (its "missed at the time" retroactive bullet: `GetState()`'s new arity broke `app.py`'s positional unpacking, fixed by threading the new field through `daemon_client.py`/`daemon_stub.py`/`app.py`), thread the new wire shape (`macros` in `GetConfig()`, the new D-Bus methods) through `daemon_client.py`/`daemon_stub.py` so the Rust+Python test suites stay green. Additionally, since the Action cutover invalidates `binding_editor.py`'s existing inline Macro step editor's data path outright (it reads/writes the now-gone `steps` field directly on the Binding), temporarily gate or stub that editor — e.g. disable "Macro" in the Action dropdown, or reduce it to a bare read-only `macro_id` display — just enough that the GUI doesn't crash on a Macro Binding. This is explicitly *not* building the real picker (that's ticket 52's job) — the narrowest change that keeps the app running and the test suites passing.

Verified via the Rust + Python test suites (per this map's stability bar — no live hardware needed for a Daemon/config-only ticket, matching ticket 21's own precedent).

## Answer

Landed exactly as scoped, no hardware involved. Daemon test count went from 199 to 217 (18 new tests); GUI test count went from 155 to 165 (10 new tests). `cargo build`/`cargo test`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` all clean; `.venv/bin/python -m pytest gui/tests` all green.

### `daemon/src/config.rs`

`MacroId(String)` (`#[serde(transparent)]`, `Display`, `From<String>`/`From<&str>`, no `FromStr`/validation — a lookup miss just becomes `CommandError::NotFound`) and `MacroDef { name, steps }` added. `Config.macros: HashMap<MacroId, MacroDef>` added as `#[serde(default, skip_serializing_if = "HashMap::is_empty")]`, alongside `profiles`. `Action::Macro { steps }` became `Action::Macro { macro_id: MacroId }` — the breaking change ticket 15's Answer called for, no migration. `slug_base`/`unique_macro_id` implement the lowercase/hyphenated/collision-suffixed slug algorithm (`screenshot-combo`, `screenshot-combo-2`, ...), unit-tested directly. `parse()` gained a fifth validation pass, `ConfigError::UnknownMacro`, mirroring `InvalidControllerButton`'s shape exactly — a `config.toml` whose `Action::Macro` names a `macro_id` absent from `[macros]` refuses to start, file on disk untouched.

### `daemon/src/executor.rs`

`compile`'s signature grew a `macros: &HashMap<MacroId, MacroDef>` parameter; the `Action::Macro` arm resolves `macro_id` against it and `.expect()`s the entry exists, with a doc comment pointing at the two enforcement points (`SetBinding`, `config::parse`) that make a miss structurally impossible.

### `daemon/src/command.rs` / `daemon/src/dispatch.rs`

Three new `Command` variants — `CreateMacro { name, steps }` (returns the derived `MacroId`, `InvalidRequest` on an empty/whitespace name, no `AlreadyExists` since a slug collision is resolved automatically), `RenameMacro { macro_id, new_name }` (pure `MacroDef.name` write, `NotFound`/`InvalidRequest`), `DeleteMacro { macro_id }` (`NotFound`, or `InvalidRequest` while referenced) — each following the existing persist/rollback-on-failure pattern. `fire()` grew a `macros` parameter threaded from `handle_event`'s `&config.macros`. `SetBinding`'s handler grew a validation block rejecting an `Action::Macro` naming an unknown `macro_id`, mirroring the existing `ControllerButton` block. Added `macro_references`, mirroring `profile_switch_references` exactly (scans `base`/`held` across every Profile — chords don't exist in code yet, same caveat `profile_switch_references` already documents, so the ticket text's `chords_base`/`chords_held` mention is not yet applicable). Every pre-existing test that used an inline `Action::Macro { steps }` as a convenient multi-step Trigger-mode fixture (a dozen or so, across the Toggle/HoldToRepeat/actuation-point test groups) now registers a `MacroDef` via a new `macro_action`/`config_with_bindings_and_macros`/`config_with_profile_and_macros` test-helper trio and references it by `macro_id` — assertions on injected output are unchanged. New `CommandHarness` tests cover slug derivation + collision suffixing, rename-not-id, delete-while-referenced refusing then succeeding once cleared, and `SetBinding` rejecting an unknown `macro_id`.

### `daemon/src/dbus/wire.rs`

`action_to_dict`/`action_from_dict`'s `"macro"` arm carries `"macro_id"` instead of `"steps"`. `macro_step_to_dict`/`macro_step_from_dict` made `pub` (still needed for `MacroDef.steps`, just no longer inlined into a Binding's own encoding). Added `macro_def_to_dict`/`macros_to_dict`, folded into `config_to_dict` as a `"macros"` entry keyed by `macro_id` string.

### `daemon/src/dbus/mod.rs`

Three new `#[interface]` methods — `create_macro(name, steps) -> String`, `rename_macro(macro_id, new_name)`, `delete_macro(macro_id)` — mirroring `create_profile`/`rename_profile`/`delete_profile`'s shape, plus the matching `#[proxy]` lines. The four existing D-Bus tests that built an inline `Action::Macro { steps }` payload now call a new `TestServer::create_macro` helper (a real `CreateMacro` round-trip) first. Added D-Bus round-trip tests for all three new methods, including delete-while-referenced surfacing as `com.acheron.Daemon.Error.InvalidBinding`.

### GUI (`gui/`)

`wire.py`'s `action_to_variant` sends `"macro_id"` instead of a `"steps"` array; `macro_step_to_variant` stays (still used by `create_macro`'s steps argument). `daemon_client.py`/`daemon_stub.py` grew `create_macro`/`rename_macro`/`delete_macro`, and `daemon_stub.py`'s `get_config()` now returns a `"macros"` entry — `DaemonStub` mirrors the real Daemon's slug algorithm, `SetBinding` unknown-`macro_id` rejection, and (after a code-review fix, see below) empty-name rejection.

`binding_editor.py`'s Macro branch, per the ticket's explicit narrow-scope allowance: `action_summary` now shows the raw `macro_id` (not a resolved display name — that needs `config["macros"]` threaded in, left for ticket 52). `build_binding_editor`'s old inline step-editor UI (list/add/remove steps) is gone outright, replaced by a read-only `"Macro: <macro_id or (none)>"` label plus a one-line "picker not built yet" note; Save is disabled whenever a Macro Action has no `macro_id` to preserve (no picker yet to assign a fresh one), re-enabled for every other Action kind. Existing tests exercising the old step editor were replaced with tests for the new read-only/Save-disabled behavior.

### Code review findings, both fixed

- **`DaemonStub.create_macro`/`rename_macro` didn't validate an empty/whitespace name**, unlike the real Daemon (`CommandError::InvalidRequest`) — the stub's own docstring claims to mirror the Daemon's Profile-method validation, so this was a real Rust/Python parity gap for Macro specifically. Fixed by adding the same check, raising `InvalidBindingError`; added two regression tests. (Noted in passing: the *sibling* `create_profile`/`rename_profile` stub methods have this same pre-existing gap, untouched — out of this ticket's scope to fix.)
- **`wire.py`'s `action_to_variant` docstring still said Macro carries `"steps"`** after the diff changed it to `"macro_id"`. Corrected.

### Deliberately out of scope

- Migrating a pre-ticket-51 `config.toml` with an inline-Macro Binding — it now fails to parse (missing `macro_id`). Pre-existing gap per ticket 41's text, not resolved here.
- The real Macro-library picker/manager UI (list, create, rename, delete, assign-to-Binding) — `binding_editor.py`'s Macro branch is a read-only stub only, per this ticket's own scope. Ticket 52's job.
- Resolving a Macro's display name in `action_summary`/the read-only editor label — both show the raw `macro_id`; name resolution needs `config["macros"]` threaded through call sites ticket 52 will already be touching.
- `chords_base`/`chords_held` in `DeleteMacro`'s reference scan — chords aren't real Binding maps yet, so `macro_references` scans `base`/`held` only, matching `profile_switch_references`'s identical existing caveat.
