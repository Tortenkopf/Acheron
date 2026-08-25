Type: grilling
Status: open

## Question

Surfaced directly by the user while resolving [ticket 75](./75-decide-controller-button-pulse-fix.md): should mouse-button output (`Action::Keypress` with a `BTN_LEFT`/`BTN_RIGHT`/etc. `KeyCode`) get the same sustained-hold treatment under Hold-to-repeat that ticket 75 just gave `Action::ControllerButton` — i.e., `KeyDown` once on the physical Down, held continuously (ignoring kernel-autorepeat `Repeat` events) until the physical Up, rather than today's repeat-tap-train — so that a Hold-to-repeat mouse-button Binding becomes usable for click-and-drag?

Ticket 75 deliberately scoped its fix to `Action::ControllerButton` only and left this as its own question, since mouse buttons are Keypress under the hood and changing Hold-to-repeat's meaning there is a different blast radius (Keypress's Hold-to-repeat is shared with every keyboard key, not gamepad-specific) with its own tradeoffs — e.g. whether anyone actually wants/relies on today's rapid-mash-click behavior for a Hold-to-repeat mouse Binding, and whether the distinction should be made by output kind (mouse button vs. keyboard key) rather than by Action variant, since both currently ride the same `Action::Keypress`.

Open questions for this session:

- Is there an actual use case for today's mash-click Hold-to-repeat behavior on a mouse button worth preserving (e.g. a game needing rapid clicks), or does drag support just strictly dominate it?
- If both are wanted, does this need the same mode-split question ticket 78 is already deciding for `ControllerButton` (sustained-hold Hold-to-repeat + a separate Auto-fire/Turbo-style mash mode), applied here too — and should the two decisions converge on one shared mechanism/constant, or stay independent since they're different Action variants?
- Mechanically, how does the fix distinguish "this Keypress's KeyCode is a mouse button" from "this Keypress's KeyCode is a keyboard key" for the purpose of deciding sustained-hold vs. repeat-train — is there already a `is_mouse_button`-style predicate (mirroring `input::is_gamepad_button`) or does one need to be added?
- Any interaction with Chord (a Chord's Action can be a mouse-button Keypress) or Macro (a mouse-button KeyDown/KeyUp as a Macro step) worth calling out explicitly, mirroring ticket 75's blast-radius confirmation.

Record the settled design as this ticket's Answer; spawn build/verify tasks per this map's standing precedent.
