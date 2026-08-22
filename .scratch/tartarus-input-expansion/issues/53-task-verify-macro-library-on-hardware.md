Type: task
Status: resolved
Blocked by: 52

## Question

Live-verify [ticket 52](./52-task-build-macro-library-gui.md)'s Macro library GUI against the real Daemon and Tartarus Pro — split off [ticket 41](./41-task-build-stepper-macro-library-ux.md), matching this map's discipline that a ticket resolves only once actually tested against the real, connected hardware, and matching the task/verify pairing precedent set by tickets 42/44 and 48/49.

Checklist:

- [ ] Create a new Macro, add/reorder/remove steps using the real key/mouse-button picker, confirm it fires correctly when assigned to a Binding and triggered on the physical device.
- [ ] Rename a Macro; confirm every Binding referencing it keeps working (identity is the frozen `MacroId`, not the display name).
- [ ] Assign the same Macro to multiple Bindings, including across two different Profiles; confirm both fire the shared definition and an edit to one propagates to all.
- [ ] Attempt to delete a Macro still referenced by a Binding; confirm the GUI surfaces the refusal (disabled "×" with tooltip) rather than silently failing or crashing.
- [ ] Delete a Macro with no remaining references; confirm it succeeds and disappears from the picker.
- [ ] Confirm the "changes save automatically" behavior holds — no stray Save button, no edits lost on navigating away from the panel.

Fix any real bugs found live before considering this resolved, per this map's standing discipline.

## Answer

Live-verified against the real Daemon (`cargo build --release` + `install.sh`, 221 Rust tests / 187 Python tests / clippy / fmt all clean beforehand) and the connected Tartarus Pro. No GUI-automation/screenshot tooling exists (same gap tickets 44/49 hit), so the GUI was launched (`gui/.venv/bin/python3 gui/main.py`) for the user to drive directly; the checklist was run item-by-item live at the machine and reported back. All six items confirmed working with no GUI/Daemon bugs found in the Macro library itself: a new Macro's steps (add/reorder/remove) work through the real picker and fire correctly on the physical device once assigned to a Binding; renaming a Macro leaves every referencing Binding working (frozen `MacroId`, not display name); the same Macro assigned to Bindings in two different Profiles (Testing and MnM) both fired the shared definition, and editing it propagated to both; deleting a still-referenced Macro correctly disabled the "×" with the "Used by N Binding(s)" tooltip instead of failing or crashing; deleting it after clearing all references succeeded and it disappeared from the picker; the "changes save automatically" behavior held with no stray Save button and no edits lost navigating away from and back to the Library panel.

### A real bug found live, before verification could even begin

The Daemon crash-looped on startup against the user's actual `config.toml`: `TOML parse error ... missing field 'macro_id'` on `[profiles.MnM.base.thumbstick_left.action]`, which still carried the pre-ticket-51 inline `Action::Macro { steps }` shape. Ticket 51's Answer had explicitly flagged this exact gap as out of its own scope ("existing `config.toml` files with inline Macro Bindings are out of this ticket's scope to migrate") — it wasn't a surprise regression, but it was still a genuine live blocker: under `systemd --user`'s restart policy the Daemon didn't just fail once with a readable error, it crash-looped (`Start request repeated too quickly`) until `systemctl reset-failed` was used.

Unblocked by hand-editing the single affected Binding directly in `~/.config/acheron/config.toml` — converted it to `[macros.thumbstick-right-click]` (`name`, one `key_down = "BTN_RIGHT"` step) plus `macro_id = "thumbstick-right-click"` on the Binding, a data fix confirmed against the user before making it, not a code change. Building a real migration or a friendlier guard is real design/implementation work in its own right (auto-migrate vs. refuse-with-a-clear-message, plus the systemd restart-policy question), so rather than silently expanding this ticket's scope, spawned [ticket 57](./57-task-migrate-or-guard-pre-macro-library-config.md) to decide and build it — matching this map's own precedent (ticket 21, ticket 41→51) for splitting off newly-discovered plumbing gaps rather than folding them into whatever ticket happened to surface them.

No Rust or Python code changed by this ticket. Daemon/GUI test counts unchanged (221 / 187).
