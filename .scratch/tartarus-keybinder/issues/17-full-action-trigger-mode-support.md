# 17 — Full Action/Trigger-mode support

**What to build:** Macro actions and the Hold-to-repeat/Toggle trigger modes, end to end — the Daemon can run a looping or held-down Action and cleanly stop it, and the GUI can author one. See `.scratch/tartarus-keybinder/spec.md` ("Domain model (Daemon, Rust)", "Daemon event loop and concurrency", "Toggle behavior across Layer/Profile switches") for the full design.

**Blocked by:** 16

**Status:** ready-for-agent

- [ ] `Action::Macro { steps: Vec<MacroStepDTO> }` exists, `MacroStepDTO` = `{ KeyDown(Key), KeyUp(Key), Delay(ms) }`.
- [ ] Both `Action` kinds compile at config-load time into one `steps: Vec<MacroStep>` on the runtime `Binding`, run by one shared executor (a Keypress becomes a canned modifier-down/key-down/key-up/modifier-up sequence).
- [ ] `TriggerMode::HoldToRepeat` fires on `Down` and every subsequent `Repeat` (driven by the device's native evdev autorepeat, no separate repeat-interval config).
- [ ] `TriggerMode::Toggle` starts on `Down`, loops the compiled step sequence indefinitely (`tokio::time::sleep` between `Delay` steps) in its own spawned task, and ignores `Repeat`/`Up` entirely.
- [ ] Firing any Action spawns its own task; all `uinput` writes still funnel through the one injector task from ticket 13 (write-commands over a channel), so concurrently-running Toggles never interleave raw writes.
- [ ] Active Toggles are tracked in a `HashMap<Input, ActiveToggle>`, each holding a live `HashSet<Key>` of currently-down keys (updated as the executor processes `KeyDown`/`KeyUp`) plus a `CancellationToken`.
- [ ] Pressing the physical key that has an active Toggle stops that Toggle first — force-releasing exactly its tracked held keys via the injector — regardless of what Binding the key's current Layer/context nominally assigns; only then does the key resume normal evaluation.
- [ ] `SetBinding`'s existing `a{sv}` encoding (from ticket 15) is exercised for real now with `Macro`/`HoldToRepeat`/`Toggle` payloads — no wire-format changes needed.
- [ ] The GUI's shared Binding editor gains a Trigger-mode dropdown (Fire-once/Hold-to-repeat/Toggle) and a Macro step add/remove list (Keypress steps + per-step delay), per the prototype's editor component.
- [ ] Live demo: bind a grid key to a Toggle Macro with visible delays between steps, watch it loop continuously after one press, press the same key again and see it stop cleanly (no stuck keys); bind another key to Hold-to-repeat and confirm continuous refire only while physically held.
- [ ] Automated tests (fake `CaptureSource` + real dispatch/executor) cover: Toggle start/loop/stop-releases-exact-keys, Hold-to-repeat firing on Repeat events, and that an unbalanced Macro (e.g. odd KeyDown without matching KeyUp) still gets force-released cleanly on stop.
