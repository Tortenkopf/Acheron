Type: task
Blocked by: None — can start immediately

## Question

Land [Design the Stepper list-stepping construct](./03-decide-stepper-list-stepping.md)'s settled shape into the real Daemon/config, Daemon-side only — split off [ticket 41](./41-task-build-stepper-macro-library-ux.md), mirroring [ticket 51](./51-task-land-macro-library-daemon.md)'s split of the Macro half. This ticket is Daemon/config/D-Bus only — no GUI widgets. [Build the Stepper library GUI](./55-task-build-stepper-library-gui.md) is the paired follow-up that consumes this shape. Independent of ticket 51/Macro's schema (a different `Action` variant, a different `Config` field, and — unlike Macro's cutover — purely additive, since `Action::Step`/a Stepper library don't exist at all yet), so it carries no blocking edge on the Macro-side tickets and can be built in either order relative to them.

Scope:

- **Config**: a Stepper-list global library equivalent to `Config.macros` — a named, ordered list of items, each item a type distinct from `Action` and structurally restricted to a single fire-once keyboard key or mouse-button (never a Macro or another Stepper), per ticket 03's Answer. Identity follows the same frozen-slug `StepperId`-from-`name` pattern as `MacroId` (ticket 15), for consistency across the two libraries.
- **Action**: new `Action::Step { stepper: StepperId, direction: Forward | Backward }` — one variant with a direction field, occupying an Input's ordinary Binding slot like Keypress/Macro/Chord do. Net-new addition, no existing shape to cut over.
- **D-Bus**: `CreateStepper`, `RenameStepper`, `DeleteStepper`, and the forward/backward pair-assignment call — assigning a list to a new Input pair silently moves it off its old pair (no reject-at-save step, unlike Chord's overlap rejection; ticket 03 confirmed no ambiguity to prevent here). At most one pair may reference a given list at a time.
- **Runtime cursor**: per-list cursor is Daemon-side-only runtime state, independent of Profile/Layer, never persisted to `config.toml` — always resets to the list's first item on Daemon restart (mirrors `capture_mode`'s live-`GetState()` precedent, ticket 03). Wraps around at either end. Threaded into `GetState()` for the GUI's benefit, the same way `capture_mode` is.
- **Trigger mode**: Fire-once and Hold-to-repeat both apply to a Stepper's forward/backward Bindings via the existing repeat machinery; Toggle must be rejected for `Action::Step` — the first documented Trigger-mode exception (ticket 03's Answer, already reflected in CONTEXT.md's Trigger mode entry).
- **Cross-module plumbing, not GUI widgets**: thread the new wire shape (`steppers`/cursor state in `GetConfig()`/`GetState()`, the new D-Bus methods) through `daemon_client.py`/`daemon_stub.py` so the Rust+Python test suites stay green, per the same precedent [ticket 51](./51-task-land-macro-library-daemon.md) follows from ticket 21. Since `Action::Step` is net-new (no existing GUI code path references it), there is no equivalent stub/disable step needed here — `binding_editor.py`'s Action dropdown simply doesn't offer "Stepper" yet until ticket 55 adds it.

Verified via the Rust + Python test suites — no live hardware needed for a Daemon/config-only ticket.
