Type: grilling

## Question

Fix the stuck-key/stuck-button gap in Fire-once and Hold-to-repeat, surfaced live during [Finalize mouse-button/full-keyboard output and design the picker](./02-decide-mouse-button-output-and-picker.md): a Macro whose steps include a `KeyDown` with no matching `KeyUp` (e.g. a single-step Macro used to fake a "hold" — the exact technique tried with `KEY_LEFTCTRL` under Hold-to-repeat) leaves that key held down at the OS level forever.

Root cause, confirmed by reading `dispatch.rs::fire()`: Fire-once and Hold-to-repeat only ever fire-and-forget their compiled step sequence — neither reacts to the physical Input's `Up` event at all (only `Down`/`Repeat` are handled), and neither tracks which keys a firing left down the way `Toggle`'s `run_toggle_loop` tracks `held: HashSet<KeyCode>` and force-releases it via `injector::force_release_key` when the Toggle is cancelled. Reproduced live: recovering from the stuck `KEY_LEFTCTRL` required a full reboot — releasing the grid key, re-pressing it, and even changing the binding away all did nothing.

Toggle itself is unaffected and already the correct, proven pattern for "hold until pressed again" — confirmed via the user's own working mouse-look toggle on the MnM profile's thumbstick, `Action::Macro{steps: [KeyDown(BTN_RIGHT)]}` under `TriggerMode::Toggle`, which force-releases correctly on the second press.

Settle and build:

- Give Fire-once/Hold-to-repeat the same held-key tracking + force-release `Toggle` already has — most likely: track keys left down by an in-flight Fire-once/Hold-to-repeat firing per `Input` (mirroring `dispatch.rs`'s existing `in_flight: HashMap<Input, JoinHandle<()>>`), and force-release them on that Input's physical `Up` (currently ignored entirely for these two Trigger modes).
- Decide whether this is purely a Daemon-side fix, or whether the GUI's Macro-step editor should also warn/block saving an unbalanced Macro under Fire-once/Hold-to-repeat (belt-and-suspenders, per ticket 02's README footgun-list note) — or both.
- Confirm the fix doesn't regress Toggle's own existing force-release path, and doesn't break a normal *balanced* Fire-once/Hold-to-repeat Macro (which already self-releases correctly today).

This map carries execution: build the fix and verify live against the real Daemon/Tartarus Pro — reproduce the original stuck-key scenario first (confirm it still reproduces pre-fix), then confirm release now happens correctly on physical key-up, without needing a reboot.
