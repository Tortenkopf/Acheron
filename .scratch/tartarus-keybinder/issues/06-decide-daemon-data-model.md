Type: grilling
Status: resolved

## Question

Design the Rust data model the Daemon uses internally to represent Profiles, Layers, Bindings, Actions (Keypress/Macro), and Trigger modes (Fire-once/Hold-to-repeat/Toggle) — the structs/enums, how a (Profile, Layer, Input) triple resolves to a Binding, how the currently-active Toggles are tracked given the toggle-survives-Layer-but-not-Profile rule decided in [Decide toggle behavior across switches](./04-decide-toggle-behavior-across-switches.md), and how this in-memory model maps to/from the `config.toml` schema (`schema_version = 1`) decided in [Decide config file format](./03-decide-config-file-format.md). This is the foundational shape both [Design Daemon capture/injection event loop](./07-design-daemon-capture-event-loop.md) and [Decide D-Bus interface surface](./08-decide-dbus-interface-surface.md) build on.

## Answer

Grilling session, 2026-08-13.

**Input** — composite enum: `ModeKey`, `Grid(row, col)`, `Thumbstick(Direction)`, `Wheel(WheelEvent)` (scroll up/down, middle click) — not a flat 28-variant enum, since the grid's 2D shape is worth keeping. Serializes to flat TOML string keys (`mode_key`, `grid_r1c1`, `thumbstick_up`, `wheel_scroll_up`, `wheel_middle`) matching [Enumerate physical inputs](./01-enumerate-physical-inputs.md) exactly, via a custom `Display`/`FromStr` impl.

**Layer** — closed 2-variant enum `{ Base, Held }`. Fixed hardware fact (one Mode key), no `Vec<Layer>` generality needed. Every Profile always has both Layers present at the type level.

**Mode key is independently bindable** — not reserved for Layer-switching only. It's a normal `Input`, fully integrated into the same `(Layer, Input) → Binding` lookup as any grid key. A per-Profile `mode_key_role: { LayerSwitch, Bound }` field routes its physical press/release events: `LayerSwitch` intercepts before the lookup (today's Layer-activation behavior); `Bound` routes it through the identical lookup+Trigger-mode dispatch path as any other Input, with full Fire-once/Hold-to-repeat/Toggle support. Per-Profile (not global) — consistent with Profiles already being complete, independently-switched Binding sets. Held-layer bindings are retained, not deleted, when unreachable under `Bound` — avoids a silent data-loss trap if the GUI later flips the setting back.

**Lookup structure** — per Layer, a sparse `HashMap<Input, Binding>` holding only configured overrides; an absent entry means passthrough (the Daemon re-emits the Input's original keycode unchanged). This mirrors the sparse, human-edited TOML file directly — no need to hand-bind all 28 Inputs just to keep normal behavior. Profiles are `HashMap<String, Profile>` keyed by name.

**Action (config-facing DTO)** — `{ Keypress { modifiers: Modifiers, key: evdev::Key }, Macro { steps: Vec<MacroStepDTO> } }`. Reuses `evdev::Key` directly (no parallel keycode enum — it's already the injection wire format) plus a `Modifiers` bitflags struct (ctrl/shift/alt/super) for chords. `MacroStepDTO` (and runtime `MacroStep`) = `{ KeyDown(Key), KeyUp(Key), Delay(ms) }` — keyboard-only for MVP (matches CONTEXT.md's literal Macro definition); non-keyboard steps (wheel/thumbstick injection) are explicitly out of scope for now, deferrable later as a plain enum-variant addition.

**Runtime `Binding`** — both Action kinds compile down to one `steps: Vec<MacroStep>` at config-load time (a Keypress becomes a canned modifier-down/key-down/key-up/modifier-up sequence). One shared executor runs both, so Trigger-mode logic (how each mode fires and cleanly stops) is written once. The config DTO/runtime split keeps the GUI's Keypress-vs-Macro authoring distinction intact while the executor only ever sees flattened steps.

**Trigger mode** — `{ FireOnce, HoldToRepeat, Toggle }`. `HoldToRepeat` is a bare unit variant, driven by the device's native evdev autorepeat events (same mechanism as normal OS key-repeat) — no separate repeat-interval config. `Toggle` loops the step sequence until stopped.

**Active toggles** — `HashMap<Input, ActiveToggle>`, where `ActiveToggle` tracks the live `HashSet<Key>` of currently-down keys, updated as the executor processes `KeyDown`/`KeyUp` steps. This makes the already-decided "press the same physical key to stop it, cleanly" guarantee ([Decide toggle behavior across switches](./04-decide-toggle-behavior-across-switches.md)) hold even against an unbalanced macro: the stop mechanism force-releases exactly the tracked keys rather than trusting the macro to be well-formed. Profile switch clears the whole map at once (no per-Profile scoping needed, since Profile switch already kills every active toggle per the resolved behavior).

**TOML mapping** — builds on `schema_version = 1` and Daemon-exclusive ownership from [Decide config file format](./03-decide-config-file-format.md):
```toml
schema_version = 1

[profiles.gaming]
mode_key_role = "layer_switch"

[profiles.gaming.layers.base.bindings.grid_r1c1]
trigger = "fire_once"
action = { type = "keypress", key = "KEY_F1" }

[profiles.gaming.layers.held.bindings.grid_r2c1]
trigger = "toggle"
action = { type = "macro", steps = [
  { key_down = "KEY_A" }, { delay_ms = 50 }, { key_up = "KEY_A" }, { delay_ms = 100 },
] }
```

No new tickets surfaced — this closes out the data-model branch of the fog cleanly. [Design Daemon capture/injection event loop](./07-design-daemon-capture-event-loop.md) and [Decide D-Bus interface surface](./08-decide-dbus-interface-surface.md) are now unblocked.
