# Remove the Additive staging mode

Type: task
Status: resolved
Blocked by: 11
Parent: [Humane output rate](../map.md)

> Execution ticket — resolved on this map, not handed off (map Notes: "execution is in
> scope"). Graduated from [ticket 11](11-additive-staging-mode-fix-or-cut.md)'s decision to
> **cut** the Dual-stage **Additive** Staging mode. Reason: Additive with both stages
> Hold-to-repeat emits two concurrent `value=2` autorepeat streams from one press — a shape
> no physical keyboard produces (the kernel autorepeats only the most-recently-pressed
> key), so it is an ADR-0008 violation, not a borderline regularity question. No known use
> case, always the shakiest of the four modes. After the cut **no** staging mode ever emits
> two concurrent autorepeat streams.

## Question

Remove `StagingMode::Additive` and everything that references it. `StagingMode` becomes
**Handoff / No-Return / Quick-Skip**. All policy is settled by ticket 11 — this ticket is
the implementation.

### 1. Config surface + migration (`daemon/src/config.rs`)

- Drop the `Additive` variant from `enum StagingMode` (`config.rs:~463`).
- **A `config.toml` carrying `staging_mode = "additive"` (i.e. `[…] deep_stage.mode` /
  `DeepStageConfig.mode`) is rejected at load with an Additive-specific `ConfigError`** — no
  silent downgrade to Handoff. The error message must name Additive and say it was removed
  (point the user at rebinding the deep stage or using a Macro). Generic serde parse
  failures are not acceptable.
- **Mechanism — your call, one constraint (the specific message).** Preferred shape: keep
  `"additive"` *parseable* (e.g. retain a `#[serde(rename = "additive")]` unit variant on a
  dedicated `RemovedStagingMode` marker, or a `#[serde(alias)]` capture) and reject it in
  `load_or_seed` / `config::validate` with `ConfigError::RemovedStagingModeAdditive` (name
  at your discretion). A custom `Deserialize` that errors inline is acceptable only if it
  still yields the specific message. Whichever: a hand-written `config.toml` with
  `mode = "additive"` must fail load with a message a human immediately understands.
- Check `schema.rs` / any `config::validate` truth-table fixtures (post-release ticket 14's
  `check_binding` seam is per-Binding; staging mode lives in `DeepStageConfig`, per-Input
  per-Profile — trace where it is actually validated).

### 2. D-Bus wire (`daemon/src/dbus/wire.rs`)

- `staging_mode` to-string (`wire.rs:~188`): drop the `Additive => "additive"` arm.
- `staging_mode` from-string (`wire.rs:~197`): drop `"additive" => Ok(...)`. An unknown
  string here already errors — confirm `"additive"` now takes that path with a sensible
  message (this is the D-Bus `SetStagingMode` boundary, distinct from config load; a client
  sending `"additive"` should just get the ordinary unknown-mode rejection).
- `wire.rs:~1564` test using `StagingMode::Additive` — re-point or drop.

### 3. Stage machine (`daemon/src/stage.rs`)

- Delete `fn additive()` (`stage.rs:~222`) and the `StagingMode::Additive => (additive(...))`
  arm in `advance()` (`stage.rs:~143`).
- Delete the `additive_table()` test (`stage.rs:~1121`).
- **Reword the self-references that name the dead mode** — describe the ops directly:
  - `QuickSkipPhase::Skipped` doc comment (`stage.rs:~118`): "runs Additive-with-no-primary,
    release path = No-Return" → describe it as its own thing (deep-only crossings, primary
    stays suppressed, release path behaves like No-Return).
  - `stage.rs:~368` ("Additive never emits an inner…") and `stage.rs:~412` ("Additive can
    hold both stages live at once…") — reword; `Slots<StageKey>` is still the right
    structure (Quick-Skip `Skipped` still fires the deep stage independently), just not
    *because* of Additive.
  - Test `quick_skip_skipped_runs_additive_with_no_primary_release_path_no_return`
    (`stage.rs:~1243`) — rename; keep the assertions.
- `deep_repeat` / `primary_handed_off` **stay** — Handoff / No-Return still need them
  (deep-stage Hold-to-repeat still rides the primary's `RepeatSchedule` pulses; the primary
  is still handed off in the deep band). Only the "Additive leaves `primary_handed_off`
  false, so its primary repeats through the ordinary path" comment in `dispatch.rs:~328`
  goes away.

### 4. Dispatch (`daemon/src/dispatch.rs`)

- `dispatch.rs:~80`, `~247`, `~328`: comments listing "Handoff/No-Return/Additive" → drop
  "Additive".
- Delete the `dual_stage_additive_holds_both_stages_on_independent_untouched_cadences` test
  (`dispatch.rs:~5581`). Confirm the Handoff / No-Return / Quick-Skip dual-stage tests still
  cover the deep-repeat path.

### 5. Other daemon tests

- `edit.rs:~1827` / `~1847` — fixture builds a `DeepStageConfig { mode: Additive }` and
  asserts it round-trips. Re-point to Handoff or No-Return (or add a *new* test asserting an
  `additive` config value is rejected at load).
- `dbus/mod.rs:~2944` / `~3013` / `~3018` — a `set_staging_mode("grid_r1c1", "additive")`
  D-Bus round-trip test. Re-point to a surviving mode; optionally add a test that
  `"additive"` is now rejected at the D-Bus boundary.

### 6. GUI (`gui/`)

- `acheron_gui/binding_editor.py:~980` — remove the `("additive", "Additive")` entry from
  `STAGING_MODES`.
- `acheron_gui/daemon_stub.py:~75` — remove `"additive"` from `_STAGING_MODES`.
- `tests/test_binding_editor.py:~1566` — test clicks an "Additive" toggle button. Re-point
  to "No-Return" (Handoff is likely the default/pre-selected — pick a mode the test can
  actually toggle *to*).
- `tests/test_daemon_stub.py:~945` / `~1033` / `~1040` — `set_staging_mode(..., "additive")`
  calls. Re-point to a surviving mode; the stub should reject `"additive"` like any unknown
  mode.
- No `rules.py` mirror for staging mode — confirm.

### 7. Docs

- **New `docs/adr/0009-additive-staging-mode-removed.md`** — "Additive staging mode removed:
  a real keyboard can't hold two autorepeating keys." Content: the impossibility argument
  (kernel autorepeats only the most-recently-pressed key), the `value=2`-rebuild trigger
  (two phase-locked streams), no use case / shakiest mode, the clean post-cut invariant (no
  mode emits two concurrent autorepeat streams), the hard-`ConfigError`-not-downgrade
  migration choice. Cites ADR-0008 (the reason) and ADR-0007 (the 4-mode context). Follow
  the effort/ticket-ref convention ticket 09 used (`humane-output-rate` ticket 13).
- **ADR-0007** (`0007-dual-stage-depth-interpretation-in-dispatch.md:~5`) — the
  "(Handoff / No-Return / Additive / Quick-Skip)" parenthetical → three modes + a one-line
  "(Additive later removed — ADR-0009)" note.
- **ADR-0008** — one-line pointer that Additive was removed as the first concrete surface
  the ceiling *deleted* rather than reshaped (ADR-0009).
- **`CONTEXT.md`**:
  - Remove the `**Additive**:` glossary entry (`CONTEXT.md:~107`).
  - `**Staging mode**:` entry (`CONTEXT.md:~98`) — "Handoff, No-Return, Additive, or
    Quick-Skip" → "Handoff, No-Return, or Quick-Skip".
  - `**Quick-Skip**:` entry — check it does not lean on "Additive" for its wording (it
    describes its own suppression; likely fine).
- **`README.md:~254`** — remove the `- **Additive** — …` bullet from the Dual-stage keys
  section. No "use a Macro instead" note (ticket 11: not needed).
- **`prototype/04-dual-stage-binding-editor-layout/prototype.py:~96`** — remove the
  `"additive"` entry (throwaway prototype, but keep it consistent).

### 8. Verify

- Full suite green: `cargo test` (daemon), GUI `pytest`, `cargo clippy` — as tickets 09/12.
- A `config.toml` with `deep_stage.mode = "additive"` fails `load_or_seed` with the
  Additive-specific message (add this as a regression test).
- `grep -rn -i additive` over `daemon/ gui/ docs/ README.md CONTEXT.md` returns nothing but
  ADR-0009 and the ADR-0007/0008 pointers (and unrelated hits — `config.rs` has several
  "additive change" prose uses that are not this mode).

## Answer

Done on `dev` 2026-09-07. `StagingMode` is now **Handoff / No-Return / Quick-Skip**.

### Migration mechanism — raw-TOML scan, not a serde alias

`ConfigError::RemovedStagingModeAdditive(Vec<String>)` (carries a dotted breadcrumb per
hit, like `LegacyInlineMacroBinding`). `Additive` is *fully gone* from `enum StagingMode`
— no retained marker variant. Detection is a new `config::find_removed_additive_staging`
raw-`toml::Value` walk run in `parse()` right after `find_legacy_macro_bindings` and ahead
of the typed `Config` deserialize (which, with the variant gone, would otherwise reject
`"additive"` as serde's generic "unknown variant"). It matches a table whose path contains
`.deep_stages.` and whose `mode` key is the string `"additive"`. The `Display` message
names Additive, cites ADR-0009, and points at rebinding to Handoff/No-Return/Quick-Skip or
a Macro on the deep stage. Chose the scan over a `#[serde(alias)]`/marker-variant because
a retained variant leaks into every `match` on `StagingMode` for a value that must never
be constructed — the raw scan keeps the live enum honest and mirrors the existing
legacy-Macro guard exactly. `config::validate` needs no change (the bad value never
reaches a typed `Config`). `schema.rs` confirmed untouched — it drives `check_binding`
(per-Binding), and staging mode lives in `DeepStageConfig` (per-Input per-Profile), never
routed through that seam.

### Code removed / reworded

- **`config.rs`**: `StagingMode::Additive` variant dropped; enum doc reworded ("three
  staging modes", + a note on the removal & the load rejection); `RemovedStagingModeAdditive`
  variant + `Display` arm + `find_removed_additive_staging` + the `parse()` call.
- **`dbus/wire.rs`**: `staging_mode_str` / `staging_mode_from_str` `Additive`/`"additive"`
  arms dropped — a client sending `"additive"` now gets the ordinary
  `"…is not a valid staging mode"` rejection (the migration-flavoured message is
  config-load only). `every_staging_mode_round_trips…` test trimmed to three; new
  `staging_mode_from_str_rejects_the_removed_additive_mode`.
- **`stage.rs`**: `fn additive()` and the `advance()` arm deleted; `additive_table()` test
  deleted; `QuickSkipPhase::Skipped` doc, the `advance` doc, the `primary_handed_off`
  doc, and the `Engine` doc all reworded to describe the ops directly (no dead-mode
  name); test `quick_skip_skipped_runs_additive_with_no_primary_release_path_no_return`
  → `quick_skip_skipped_runs_the_deep_stage_alone_with_a_no_return_release_path`
  (assertions unchanged). `deep_repeat` / `primary_handed_off` kept — Handoff/No-Return
  still need them.
- **`dispatch.rs`**: the three "Handoff/No-Return/Additive" comments trimmed; the
  "Additive leaves `primary_handed_off` false…" comment removed; the
  `dual_stage_additive_holds_both_stages_on_independent_untouched_cadences` test deleted.
  Deep-repeat coverage for the surviving modes confirmed intact
  (`dual_stage_handoff_deep_stage_hold_to_repeat_actually_repeats`,
  `dual_stage_handoff_suppresses_the_hold_to_repeat_primary_under_the_deep_stage`,
  `dual_stage_no_return_keeps_the_hold_to_repeat_primary_suppressed_past_the_deep_band`).
- **`edit.rs`**: `set_deep_actuation_on_an_existing_entry_leaves_its_mode_untouched`
  fixture `Additive` → `NoReturn`.
- **`dbus/mod.rs`**: `deep_stage_edits_over_real_dbus_persist_and_surface_via_get_config`
  re-pointed `"additive"` → `"no_return"` (3 spots).
- **GUI**: `binding_editor.STAGING_MODES` and `daemon_stub._STAGING_MODES` lose
  `additive`; `test_binding_editor` picks "No-Return"; `test_daemon_stub` re-points
  three calls + adds `test_set_staging_mode_rejects_the_removed_additive_mode`;
  `prototype/04-…/prototype.py` entry removed. No `rules.py` mirror — confirmed
  (`rules.py` mirrors `config::validate` only; staging mode is a wire-parse-boundary
  check).

### Docs

- **New `docs/adr/0009-additive-staging-mode-removed.md`** — the impossibility argument
  (kernel's single `repeat_key`), the `value=2`-rebuild trigger, no use case / shakiest
  mode, the clean post-cut invariant, the hard-`ConfigError` migration choice; cites
  ADR-0008 and ADR-0007.
- **ADR-0007** line 5–7: parenthetical → three modes + "(A fourth mode, Additive, was
  later removed — ADR-0009.)"
- **ADR-0008**: a short paragraph in the `value=2` section — Additive is the first
  surface the ceiling *deleted* rather than reshaped.
- **`CONTEXT.md`**: `**Additive**:` glossary entry removed; `**Staging mode**:` list
  trimmed to "Handoff, No-Return, or Quick-Skip" + a parenthetical pointer to ADR-0009.
  `**Quick-Skip**:` entry checked — describes its own suppression, no lean on Additive.
- **`README.md`**: the `- **Additive** — …` bullet removed; no "use a Macro" note
  (ticket 11).

### Regression test

`config::tests::refuses_to_start_on_a_removed_additive_staging_mode_and_names_it` — a
`config.toml` with `deep_stages.grid_r1c1.mode = "additive"` fails `load_or_seed` with
`RemovedStagingModeAdditive(["profiles.Default.deep_stages.grid_r1c1"])`, the message
contains "Additive", and the on-disk file is left untouched (refused, not downgraded).

### Verify

- `cargo test` (daemon): **525 passed** (−1 `dual_stage_additive…`, −1 `additive_table`;
  +1 wire rejection, +1 config regression — net 0). `cargo clippy --all-targets -D
  warnings`: clean. `cargo fmt --check`: clean (also swept the pre-existing
  `config/binding.rs` violation from ticket 09's `afed7ce`, per ticket 12's aside).
- GUI `pytest`: **504 passed** (+1 `test_set_staging_mode_rejects_the_removed_additive_mode`).
- `grep -rn -i additive` over `daemon/ gui/ docs/ README.md CONTEXT.md prototype/`:
  only ADR-0009, the ADR-0007/0008 pointers, the `RemovedStagingModeAdditive` machinery
  (variant/Display/`find_removed_additive_staging`/regression test), the re-pointed
  test comments, and the unrelated `config.rs` "additive change" / `#[serde(default)]`
  prose. Clean.
