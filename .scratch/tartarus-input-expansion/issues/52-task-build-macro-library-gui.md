Type: task
Status: resolved
Blocked by: 51

## Question

Build the real Macro library GUI against [ticket 51](./51-task-land-macro-library-daemon.md)'s landed Daemon shape and [Prototype the Stepper library and list-editing UX](./31-prototype-stepper-library-ux.md)'s settled variant B (two adjacent panels, tab-switched, round 2's rename buttons/reorder buttons/autosave note) — split off [ticket 41](./41-task-build-stepper-macro-library-ux.md). No open design questions remain.

Scope:

- **Mount point / shell**: replaces `build_library_placeholder()` in `device_overview.py`'s Library destination ([ticket 48](./48-task-build-device-overview-nav-rail.md) built it, [ticket 49](./49-task-verify-device-overview-nav-rail-on-hardware.md) live-verified it — the Grid/Library switcher already fully swaps the content area, nothing here builds that chrome). Build the real tab-switched panel pair itself: a "Steppers" / "Macros" tab switch (same widget shape as Device Overview's own Base/Held layer tabs). The Steppers tab exists in the shell but stays a stub/placeholder — [Build the Stepper library GUI](./55-task-build-stepper-library-gui.md) fills it in.
- **Macros panel**: list chrome (name / rename "✎" / delete "×" / "+ New"), a step editor with ↑/↓/× reorder-and-remove — relocated near-verbatim from `binding_editor.py`'s existing inline Macro step editor (issue 06) rather than redesigned, now operating against `MacroDef.steps` via the library instead of a Binding's inline field. Delete ("×") is disabled with a tooltip ("Used by N Binding(s) — can't delete") while the Macro's `used_by` is non-empty, surfacing ticket 51's `DeleteMacro` refusal honestly rather than papering over it. The pane states upfront that edits save automatically (no Save button) — every mutation (add/remove/reorder/rename/delete) applies immediately, mirroring the Profile sidebar's convention.
- **Item entry**: reuses the real key/mouse-button picker `binding_editor.py` has at build time ([ticket 42](./42-task-build-key-mouse-button-picker-ux.md)'s `key_picker.py`, live-verified by [ticket 44](./44-task-verify-key-mouse-button-picker-ux-on-hardware.md)) — not a redesign, just mount the real picker component for each Macro step's key.
- **`binding_editor.py`**: the Action dropdown's existing "Macro" option now opens the library picker to assign an existing `MacroId` (or jump to "+ New") rather than authoring steps inline — replacing [ticket 51](./51-task-land-macro-library-daemon.md)'s temporary stub/disabled state with the real assignment flow.

Live-hardware verification is deliberately deferred to [Verify the Macro library on hardware](./53-task-verify-macro-library-on-hardware.md), not done in this ticket.

## Answer

Built as scoped. Daemon test count went from 217 to 221 (4 new tests); GUI test count went from 165 to 187 (22 new tests). `cargo build`/`cargo test`/`cargo clippy --all-targets -- -D warnings`/`cargo fmt --check` all clean; `.venv/bin/python -m pytest gui/tests` all green.

### A real gap surfaced while scoping: `SetMacroSteps`

Ticket 51 landed `CreateMacro`/`RenameMacro`/`DeleteMacro` only — there was no D-Bus method to overwrite an *existing* Macro's `steps` after creation. Delete-and-recreate isn't a workaround either: `DeleteMacro` refuses while a Macro is still referenced by any Binding, which is exactly the situation a user editing a Macro's steps is normally in. Without a way to persist step edits, this ticket's own scoped step editor (add/remove/reorder, autosave, no Save button) couldn't be built at all — so, mirroring ticket 41's own precedent for a follow-up ticket discovering "missed at the time" cross-module plumbing, this ticket adds `SetMacroSteps { macro_id, steps }` as a small Daemon/D-Bus addition alongside the GUI work: a pure `MacroDef.steps` field write, `NotFound` on an unknown `macro_id`, same persist/rollback-on-failure pattern as `RenameMacro`. Threaded through `daemon/src/command.rs`/`dispatch.rs`/`dbus/mod.rs` (interface method + `#[proxy]` line) and `gui/acheron_gui/daemon_client.py`/`daemon_stub.py`, each with direct unit/D-Bus-round-trip/stub-level test coverage.

### `gui/acheron_gui/library_view.py` (new)

The real Steppers/Macros tab-switched panel pair (`build_library_view`), mounted from `device_overview.build_main_view` in place of the old `build_library_placeholder()`. `build_library_tabs` mirrors `build_layer_bar`'s own Base/Held tab shape exactly. The Macros tab is selected by default (not display order's "Steppers" first) since Steppers is an inert stub (`build_steppers_stub`, naming ticket 55) pending ticket 54's Daemon/D-Bus surface — opening straight into a stub would be a worse first look at the destination than the tab that's actually real.

`build_macros_panel` is the list-plus-editor pair: a name/rename("✎")/delete("×")/"+ New" row per Macro (`build_macro_row`, ported from `build_profile_sidebar`'s row shape), selecting a row loads its editor below. Delete is disabled with a "Used by N Binding(s) — can't delete" tooltip while referenced — `macro_used_by_count` computes this client-side by scanning `GetConfig()`'s own `profiles`/Bindings (mirroring the Daemon's own `dispatch.rs::macro_references` scan, just counted instead of boolean), no new wire field needed despite the ticket text's phrasing suggesting one.

`build_macro_editor` is the step editor — ↑/↓/× reorder-and-remove plus a KeyDown/KeyUp/Delay "+ Add step" row, relocated near-verbatim from `binding_editor.py`'s pre-ticket-51 inline Macro step editor (git history, commit `cb20cc9~1`) rather than redesigned, reusing `key_picker.build_inline_key_picker` for each step's key exactly as ticket 44 left it. Every mutation calls `client.set_macro_steps` then a full `on_change()` rebuild — no local Save button, and the pane states upfront that edits save automatically, matching ticket 31 round 2's settled convention.

### `binding_editor.py`

The Macro Action branch is the real assignment flow: a dropdown of existing library entries by display name (assigning `macro_id`, defaulting to the first entry the same way the Profile-switch branch already defaults its own target dropdown) plus a "+ New Macro" popover that calls `client.create_macro(name, [])` and assigns the result immediately — replacing ticket 51's read-only stub. Full step authoring stays out of this popover, living only in the Library screen, matching how the Controller-button/Profile-switch branches also only ever assign their own single field. `action_summary`'s Macro branch now resolves the display name via a new `macros` parameter threaded in from every call site (`config["macros"]` was already present in `GetConfig()`'s shape as of ticket 51 — this just wires it through), closing the gap ticket 51's Answer explicitly deferred; falls back to the raw `macro_id` if the entry is somehow missing (e.g. the pre-Daemon-connection placeholder config).

### Shared: `build_name_prompt_popover` moved to `gtk_utils.py`

Was private to `device_overview.py` (Profile "+ New"/rename); moved verbatim (no behavior change) so `binding_editor.py`'s "+ New Macro" and `library_view.py`'s Macro list rename/create controls can reuse the same Entry-plus-submit-button-plus-inline-error pattern instead of a third hand-rolled copy.

### Deliberately out of scope

- The Steppers panel itself — ticket 55's job, blocked on ticket 54's Daemon/D-Bus surface.
- Live-hardware verification — ticket 53.
