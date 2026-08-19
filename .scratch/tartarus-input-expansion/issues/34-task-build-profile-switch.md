Type: task

## Question

Build Profile Switch for real, against [Design Profile Switch](./05-decide-profile-switch-action.md)'s settled shape. No open design questions remain — this is implementation + live verification, per this map's "resolving a ticket means actually building and testing against the real, connected Tartarus Pro" discipline.

**Daemon:**
- `Action::ProfileSwitch { target: String }` — new variant on `daemon/src/config.rs`'s `Action` enum, wire-encoded the same `a{sv}`-with-`"type"`-tag convention as `Keypress`/`Macro`.
- `SetBinding` (and `load_or_seed`) reject any `ProfileSwitch` Binding whose `trigger != TriggerMode::FireOnce` via `CommandError::InvalidRequest`.
- Extract `Command::SwitchProfile`'s handler body (`dispatch.rs`) into one shared function (contains_key check, `mem::replace` on `active_profile`, persist, `stop_all_toggles`, `publish_actuation_snapshot`, reply-before-signal `ActiveProfileChanged` ordering — `Command::SwitchProfile` keeps its own reply/signal wiring, the rest becomes shared). `handle_event` widens `config: &Config` to `&mut Config`, threads `config_path`/`actuation_tx` through, and intercepts `Action::ProfileSwitch` before calling `fire()`/`executor::compile` — that pipeline never sees this variant.
- `RenameProfile` cascade-updates every `ProfileSwitch { target }` matching the old name, across every Profile's `base`/`held`/`chords_base`/`chords_held` (chords don't exist in code yet — scan whatever Binding maps actually exist at build time; don't invent chord storage just for this).
- `DeleteProfile` refuses (`CommandError::InvalidRequest`) if any Binding anywhere still targets it — scan the same maps as the rename cascade.
- `executor::compile`'s `match` gains an unreachable/defensive arm for `Action::ProfileSwitch` (it's intercepted in `handle_event` before ever reaching `compile`) — pick whatever the surrounding code's convention is for "should never happen" (`unreachable!()` vs. empty `Vec`).

**GUI (`gui/acheron_gui/`):**
- `inputs.py`: `ACTION_TYPES` gains `("profile_switch", "Profile Switch")`.
- `binding_editor.py`: `render_action_editor()`/`on_save()` generalize to a three-way branch; Profile Switch renders a `Gtk.DropDown` from `config["profiles"].keys()` (no self-exclusion) in place of the Key/Macro editor; `trigger_dd.set_sensitive(False)` for this kind, `on_save` always submits `trigger: "fire_once"`; `action_summary()` gains a third branch (target name, no Trigger-mode suffix).

**Tests:** unit-test the rename-cascade and delete-refusal against a `Config` with cross-Profile `ProfileSwitch` references; unit-test the validation rejection (`SetBinding` with `trigger: toggle`/`hold_to_repeat` + `ProfileSwitch`); the existing `FakeCaptureSource`-driven dispatch tests are the right place to exercise "firing a Profile Switch Binding switches Profile and force-stops Toggles" without real hardware.

**Live verification:** confirm against the real Daemon + Tartarus Pro + GUI: a grid key bound to Profile Switch actually switches, an active Toggle in the old Profile is force-stopped, `GetConfig()`/the GUI round-trip the new Action kind correctly, and a rename/delete against a referenced Profile behaves as designed.

## Blocking

Not blocked by, and doesn't block, any other open ticket — same reasoning as ticket 05's own Blocking section (touches only the `binding_editor.py` branch structure and `dispatch.rs`/`config.rs`, none of which any other open ticket owns).
