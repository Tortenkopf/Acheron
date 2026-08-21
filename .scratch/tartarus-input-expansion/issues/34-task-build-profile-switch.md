Type: task
Status: resolved

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

## Answer

Built exactly to spec, no open questions surfaced during implementation.

**Daemon:** `Action::ProfileSwitch { target: String }` added to `config.rs`'s
`Action` enum. `handle_event` (`dispatch.rs`) widened to `config: &mut
Config` plus `config_path`/`actuation_tx` params; the Binding lookup now
clones the matched `Binding` out of `profile`/`bindings` before matching on
it, ending the immutable borrow on `config` so a `ProfileSwitch` arm can
call the new shared `switch_profile()` with a mutable one — the borrow-
checker issue the ticket's own design anticipated. `switch_profile()` is the
extracted body from `Command::SwitchProfile` (contains_key check,
`mem::replace`, persist, `stop_all_toggles`, `publish_actuation_snapshot`);
`Command::SwitchProfile` now just calls it and keeps its own reply-before-
signal `ActiveProfileChanged` wiring, and firing a Binding does the same
after a bare `Down` check (validated Fire-once-only, so no other event
state needs handling). `SetBinding` and `config::parse` (`load_or_seed`)
both reject a non-Fire-once `ProfileSwitch` Binding — the latter via a new
`ConfigError::InvalidProfileSwitchTrigger`, so a hand-edited `config.toml`
can't smuggle one in either. `RenameProfile` now clones the whole
`profiles` map (not just the renamed entry) before mutating, so
`cascade_rename_profile_switch_targets` can repoint every cross-Profile
reference and a persist failure can cleanly restore the entire prior state
in one assignment. `DeleteProfile` refuses via a new
`profile_switch_references()` scan. Chords don't exist in code yet, so both
scans cover only `base`/`held`, per the ticket's own note. `executor::compile`
gets an `unreachable!()` arm, since `ProfileSwitch` never reaches it.
`dbus/wire.rs` round-trips `type = "profile_switch"` / `target` the same way
as the other two variants.

**GUI:** `ACTION_TYPES` gains `("profile_switch", "Profile Switch")`.
`render_action_editor()` gained a `profile_switch` branch rendering a
`Gtk.DropDown` seeded from `sorted(config["profiles"].keys())` (no
self-exclusion, matching the ticket); `trigger_dd.set_sensitive(False)` and
forced to `fire_once` whenever this kind is selected, and `on_save` submits
`"trigger": "fire_once"` unconditionally regardless of the (disabled)
dropdown's own state, as a second line of defense. `action_summary()`
gained a third branch: `"→ {target}"`, no Trigger-mode suffix (it would
always read `[1x]`, redundant once Fire-once is the only legal value).

**Tests:** 7 new Rust tests (188 total, up from 181) — a config-parse
accept/reject pair for the Fire-once-only trigger validation, a wire
round-trip test, and four `dispatch.rs` tests: `SetBinding` rejection
(both illegal Trigger modes), `DeleteProfile` refusal against a live
reference, `RenameProfile`'s cross-Profile cascade (covering both a
reference on an *other* Profile and a self-reference on the renamed one),
and a `FakeCaptureSource`-driven dispatch test firing a real `PhysicalEvent`
through a `ProfileSwitch` Binding that also has an active Toggle running,
asserting the Profile switches and the Toggle is force-stopped in one pass
— the exact scenario live-verified below. 5 new Python tests (88 total, up
from 83): a wire round-trip, saving a `profile_switch` Binding end-to-end
through the real dropdown-driven UI, the trigger-dropdown lock, and
`action_summary`'s bare-target format (both directly and via the grid
button's rendered label). `cargo clippy --all-targets` and `cargo fmt
--check` both clean.

**Live verification** (done together with the user, against their actual
daily-driver Daemon + Tartarus Pro + GUI — this machine runs Acheron for
real, so the new binary was installed and the live service restarted mid-
session): using a temporary `ProfileSwitchTest` Profile and two throwaway
Bindings on the (empty, currently-active) `Default` Profile — grid_r1c1
("1") → Profile Switch, grid_r1c2 ("2") → Toggle with a zero-step Macro
(no real keystroke output, chosen deliberately so the live test couldn't
spam whatever window had focus) — confirmed over the real session D-Bus
(`gui/acheron_gui/daemon_client.DBusDaemonClient`, the exact class the GUI
itself uses) and two real physical key presses:
- `GetConfig()` round-trips the new Binding shape exactly.
- Pressing "2" started a real Toggle (`GetState().active_toggles ==
  ["grid_r1c2"]`), proving the real evdev capture path reaches the new code.
- Pressing "1" switched `GetState().profile`/`GetConfig().active_profile`
  to `ProfileSwitchTest` **and** force-stopped the Toggle
  (`active_toggles == []`) in the same live pass — the ticket's two hardest
  checklist items, both confirmed together.
- `SetBinding` live-rejected a Toggle-triggered `ProfileSwitch` Binding;
  `DeleteProfile` live-refused the still-referenced `ProfileSwitchTest`.
- `RenameProfile` live-cascaded the cross-Profile reference to the new name.
- The real GUI's `build_binding_editor` was constructed against the live
  Daemon's actual `GetConfig()` inside a real `Gtk.Application` window and
  presented successfully, confirming the widget tree (including the new
  target-Profile dropdown) builds cleanly off real data, not just the
  `DaemonStub`.

Every throwaway Profile/Binding was cleaned up and the Daemon switched back
to `Default` afterward; `~/.config/acheron/config.toml` diffed byte-for-byte
identical to a pre-verification backup once cleanup finished.
