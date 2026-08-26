Type: grilling
Status: resolved

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

## Answer

Grilled with the user; every open question settled in two rounds.

**Fire-once: disallowed for `Action::ControllerButton`.** Hold-to-repeat's sustained-hold behavior (ticket 75) already covers a quick tap perfectly — a quick physical Down+Up naturally produces a quick output Down+Up. That leaves nothing for Fire-once's invented, decoupled-from-hold-duration pulse to uniquely serve: no real gamepad button press works that way. Implemented the same shape as the existing `InvalidProfileSwitchTrigger`/`InvalidStepTrigger` checks (confirmed in `config.rs` `parse()` — both are simple scans over `profile_all_bindings`, cheap, in-pattern precedent for a new Action-aware restriction): a new `ConfigError::InvalidControllerButtonTrigger` refuses to start, naming the offending Binding(s).

**Ticket 76's `CONTROLLER_BUTTON_FIRE_ONCE_PULSE_HOLD` dwell-insertion code is dead once this lands — delete it outright**, constant and all. No shipped users to protect a no-op path for.

**Analog-repeat: stays allowed, no new restriction.** Unlike Fire-once, it has a real hardware precedent for gamepad output — a turbo trigger that fires faster the harder it's pressed is exactly what Acheron's depth-driven rate curve already does. It also stays reachable independent of Action (still validated as grid-key-scoped only, per ticket 20) — no new Action-aware check added, since there's no motivating reason to restrict it.

**Toggle: unchanged, already correct.** A latched held button is a plausible real gamepad macro (e.g. a held sprint/aim); Hold-to-repeat's shape change didn't touch Toggle's own semantics.

**No new Auto-fire/Turbo Trigger mode.** Same conclusion [ticket 79](./79-decide-mouse-button-sustained-hold-drag.md) reached for mouse buttons, for the identical reason: a Macro assigned under Toggle is already a fully general, arbitrarily-timed turbo-fire (confirmed `MacroStep::KeyDown`/`KeyUp` already carries a bare `KeyCode`, e.g. `KeyCode::BTN_SOUTH` per ticket 76), strictly more flexible than any fixed-rate mode this ticket could add.

**Analog-repeat's dwell gets a gamepad-specific floor.** Checked the constants directly (`dispatch.rs`): `ANALOG_REPEAT_MAX_HZ = 20.0` already yields a 50ms period at the fastest end of the curve, comfortably above the 35ms dwell ticket 76 already vetted for frame-safe registration against a polled 60fps game read (the same class of problem [ticket 74](./74-research-gamepad-button-registration-timing.md) flagged as unaddressed for Analog-repeat's existing 15ms `ANALOG_REPEAT_PULSE_HOLD`). New `ANALOG_REPEAT_CONTROLLER_PULSE_HOLD = 35ms`, selected at fire time when the bound Action is `ControllerButton`; the existing 15ms dwell stays for Keypress/mouse-button output (interrupt-driven, not subject to a game's per-frame-polling risk the same way). `MAX_HZ`/the rate curve itself is untouched — costs nothing at the top of the curve.

**Migration/guard: not moot.** Grepped the user's own live `~/.config/acheron/config.toml` directly rather than assuming pre-release means nothing to check: the **Testing** profile currently has three Fire-once + `ControllerButton` bindings that the new validation will refuse to start against — `grid_r4c4`→`BTN_START`, `grid_r4c2`→`BTN_SELECT`, `grid_r4c3`→`BTN_MODE`. No auto-migration, per this project's established precedent (`InvalidProfileSwitchTrigger`, `InvalidStepTrigger`, ticket 57's `LegacyInlineMacroBinding` all refuse-to-start with a breadcrumbed error, never silently coerce). The verify ticket hand-fixes these three bindings' Trigger mode directly in the live config, mirroring ticket 53's exact precedent, not a code change.

CONTEXT.md's Trigger mode entry updated to name the new Controller-button/Fire-once exception alongside the existing Stepper/Toggle one.

Spawned [Build the Controller-button Trigger-mode restriction](./85-task-build-controller-button-trigger-mode-restriction.md) and [Verify the Controller-button Trigger-mode restriction on hardware](./86-task-verify-controller-button-trigger-mode-restriction-on-hardware.md).
