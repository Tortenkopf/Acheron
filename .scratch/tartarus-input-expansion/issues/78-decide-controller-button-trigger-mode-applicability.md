Type: grilling
Status: open

## Question

Surfaced mid-[ticket 75](./75-decide-controller-button-pulse-fix.md), deliberately deferred: which Trigger modes should actually be *allowed* for `Action::ControllerButton`, now that Hold-to-repeat means "sustained hold matching the physical press" for it (ticket 75's Answer) rather than a repeat-tap train?

The user's framing during ticket 75's grilling: gamepad buttons should share Hold-to-repeat, Toggle, and maybe a new Auto-fire/Turbo mode — "all made to look like what a real gamepad would be sending." Fire-once and Analog-repeat were flagged as making sense only for simulated keyboard keys, not gamepad output. No urgency — the software isn't released and has no users yet, so this can be decided without protecting anything already shipped.

Open questions for this session:

- Should Fire-once be validation-locked out for `Action::ControllerButton` (mirroring existing precedent: Stepper disallows Toggle, Profile-Switch is locked to Fire-once)? If disallowed, ticket 76's Fire-once dwell fix becomes dead code for new Bindings — decide whether to remove it then or leave it as a harmless no-op path (e.g. for existing config.toml entries, if a migration/guard isn't otherwise forced).
- Should Analog-repeat be disallowed for `Action::ControllerButton` too? It's currently reachable independent of Action (grid-key-scoped, not Action-scoped, per ticket 20) — this would add an Action-aware restriction that doesn't exist today.
- Is Toggle's existing behavior (a latch: first press holds, second press releases) already correct for gamepad output as-is, or does it need any adjustment now that Hold-to-repeat's shape has changed?
- Is a new Auto-fire/Turbo Trigger mode actually wanted, or was it a "maybe" that doesn't survive scrutiny? If wanted: this is the discrete-tap-train behavior Hold-to-repeat used to have for `ControllerButton` before ticket 75 — should it be built as its own named mode (reusing ticket 76's Fire-once-style dwell-pulse machinery, repeated), and should it be available for other Actions too or `ControllerButton`-only? Real turbo controllers are a genuine hardware precedent worth citing if built.
- If any modes end up disallowed for `ControllerButton`, does that need a config.toml migration/guard for anyone who already hand-authored a now-invalid combination (mirrors [ticket 57](./57-task-migrate-or-guard-pre-macro-library-config.md)'s precedent) — likely moot pre-release, but worth a one-line confirmation.

Record the settled design as this ticket's Answer; spawn build/verify tasks per this map's standing precedent if it changes shipped behavior.

## Comments

**From [ticket 79](./79-decide-mouse-button-sustained-hold-drag.md)'s resolution**: while deciding the mouse-button mirror of this ticket's sibling ([ticket 75](./75-decide-controller-button-pulse-fix.md)), the user pointed out that `MacroStep::KeyDown`/`KeyUp` already carries a bare, unrestricted `KeyCode` (confirmed in code — `executor.rs:500` already uses `MacroStep::KeyDown(KeyCode::BTN_SOUTH)` for ticket 76's own dwell fix), so a Macro under Toggle is already a fully general, arbitrarily-timed turbo-fire for `Action::ControllerButton` too, not just for Keypress/mouse-button output. Worth weighing directly against this ticket's own open "is a new Auto-fire/Turbo mode actually wanted" question — ticket 79 concluded no dedicated Turbo mode was needed for mouse buttons on that basis. Not resolved here; left for whoever picks up this ticket.
