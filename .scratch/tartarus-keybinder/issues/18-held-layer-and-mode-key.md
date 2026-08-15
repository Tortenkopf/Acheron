# 18 — Held Layer + Mode key

**What to build:** The Mode key activates a second, momentary Layer of Bindings while held — Hypershift-style layering — and can optionally be rebound to a normal Action per-Profile instead. See `.scratch/tartarus-keybinder/spec.md` ("Domain model (Daemon, Rust)" — Layer and mode-key-role bullets) for the full design.

**Blocked by:** 17

**Status:** implemented-pending-live-verification

- [x] `Layer` is a closed 2-variant enum, `Base` / `Held`; the `Default` Profile (and any Profile) always has both present at the type level, each with its own sparse Binding map.
- [x] A per-Profile `mode_key_role: { LayerSwitch, Bound }` field exists, defaulting to `LayerSwitch`.
- [x] Under `LayerSwitch` (default): the Mode key's press/release intercepts before the Binding lookup — pressing it activates the Held Layer for as long as it's held, and Base resumes on release. This is the dispatch task's job, not a per-Binding concept.
- [x] Under `Bound`: the Mode key routes through the identical `(Layer, Input) → Binding` lookup and Trigger-mode dispatch as any other Input, with full Fire-once/Hold-to-repeat/Toggle support from ticket 17.
- [x] Switching a Profile's `mode_key_role` from `Bound` back to `LayerSwitch` does not delete that Profile's Held-layer bindings, even though they were unreachable while `Bound` was active.
- [x] `ActiveLayerChanged(layer: s)` (`"base"`/`"held"`) fires correctly on every Mode-key press/release under `LayerSwitch`.
- [x] A `SetModeKeyRole` D-Bus method exists to flip the per-Profile role.
- [x] The GUI's Base/Held tab row (from the prototype) is wired to real `ActiveLayerChanged` state and to editing each Layer's Bindings independently; the Mode key becomes clickable/bindable in the Binding editor when the active Profile's role is `Bound`.
- [ ] Live demo: hold the physical Mode key, GUI's tab indicator flips to Held and a Held-layer remap fires instead of the Base-layer one for the same grid key; release, Base resumes. Flip `mode_key_role` to `Bound`, bind the Mode key itself to a Keypress via the GUI, confirm pressing it now fires that Keypress instead of switching Layers.
- [x] Automated tests cover: Held-layer resolution while Mode key is down, reversion to Base on release, `Bound` routing through full Trigger-mode dispatch, and Held-layer bindings surviving a `Bound`→`LayerSwitch`→`Bound` round trip.

## Comments

Implemented by agent (no physical device access in this environment): `daemon/src/config.rs` (`Layer`, `ModeKeyRole`, `Profile::held`/`layer()`/`layer_mut()`), `daemon/src/command.rs` + `daemon/src/dispatch.rs` (Mode-key interception under `LayerSwitch`, `active_layer` runtime state, `SetModeKeyRole` command, `ActiveLayerChanged` emission via a `SignalEmitter` threaded in from `main.rs`), `daemon/src/dbus/{mod,wire}.rs` (Layer-scoped `SetBinding`/`ClearBinding`, `SetModeKeyRole` method, wire encoding for `Layer`/`ModeKeyRole`), and the GUI (`device_overview.py`'s clickable Base/Held tab row + mode-key-role toggle, `app.py`'s `ActiveLayerChanged` subscription, `daemon_client.py`/`daemon_stub.py`/`binding_editor.py`/`action_table.py` threading a `layer` parameter throughout). 72 daemon tests + 28 GUI tests passing, `/code-review` run and three findings fixed (an orphaned-Toggle-on-Mode-key edge case when leaving `Bound`, a missing error-revert on a failed `SetModeKeyRole` in the GUI, a dead wire-encoding wrapper). The live-demo checkbox is left unchecked — it requires the real, connected Tartarus Pro, which this agent has no access to; **@justin, please run the live demo and check that box (or reopen with what broke) once verified.**
