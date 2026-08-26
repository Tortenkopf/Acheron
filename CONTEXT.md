# Acheron

An offline Linux application, named Acheron, that provides keybinding and macro control for the Razer Tartarus Pro, independent of Razer's own (Windows-only, cloud-backed) Synapse software.

## Language

### Configuration

**Profile**:
A named, complete set of Bindings the user manually switches between (e.g. "gaming", "editing"). Never switched automatically based on the focused application.
_Avoid_: preset, config, mode

**Layer**:
A temporarily-active alternate set of Bindings, active only while the Mode key is held. A Layer is momentary and always relative to whichever Profile is currently active; a Profile is persistent and manually selected.
_Avoid_: Hypershift (Razer's marketing name for this), mode, shift state

**Mode key**:
The physical button on the device that, while held, activates the Layer.
_Avoid_: Hypershift button, Fn key

**Input**:
One physical control on the Tartarus Pro that can be bound: a grid key, the Mode key, the thumbstick (in any direction), or the scroll wheel.
_Avoid_: key (too narrow — reserve for grid keys specifically), control

**Binding**:
An assignment from one Input to an Action, scoped to a specific Profile and Layer — the same physical Input can carry a different Binding in each Profile/Layer combination.
_Avoid_: mapping, keybind (as a bare synonym — "keybind" refers to a Binding whose Action is a Keypress)

**Action**:
What a Binding produces when triggered — a Keypress, a Macro, a Stepper step, a Profile Switch, or a Controller button press.
_Avoid_: output, effect

**Keypress**:
An Action that is a single remapped key or shortcut (may be a modifier combination, e.g. Ctrl+Shift+T).
_Avoid_: single binding, simple action, chord (see Chord — a distinct concept)

**Chord**:
A Binding whose trigger is a *set* of two or more Inputs pressed together (open-ended size) rather than a single Input — fires its own Action exactly like an ordinary Binding (any kind except Profile Switch, which has nowhere to run from inside the Chord-detection state machine — ticket 40), scoped to Base/Held the same way. Detected via a fixed ~50ms window between the first and last member Input going down: if satisfied, the Chord's Action fires and each member's individual Binding is suppressed for that press; if the window elapses first (or a member releases early, without ever completing one), the pending member's individual Binding fires instead. Releasing any one member ends the Chord's held/toggle state as a whole, regardless of Trigger mode. An Input may belong to any number of Chords (needed for the thumbstick's four diagonals — Up sits in both Up-Left and Up-Right); `SetChordBinding` rejects at save time only a *subset/superset* relationship between two Chords' member sets (e.g. `{Up,Left}` vs `{Up,Left,ModeKey}`), since completing the smaller one is then indistinguishable from being partway into the larger one — a plain intersection, like the diagonals, is not a conflict. The thumbstick's four diagonals are ordinary 2-member Chords over adjacent cardinal Inputs, not a separate mechanism. The GUI records membership by clicking Inputs on Device Overview's own grid while a "Select Chord members" toggle is active, not live physical-press capture on the real device (ticket 30's original design intent) — a build-time scope cut, not a redesign.
_Avoid_: combo, simultaneous binding, key combination (reserved for a Keypress's modifier combination — see Keypress)

**Macro**:
An Action that fires a named, reusable sequence of Keypresses, each with its own delay before the next fires. Macro definitions live in one global, named library — defined once (`MacroId`, a slug frozen at creation and never rewritten, paired with a separately-editable display name), then referenced by any number of Bindings across any Profile at once (ordinary shared reuse, not exclusive-owner reassignment like Stepper). Deleting a Macro still referenced by a Binding is refused, so a dangling reference is structurally impossible.
_Avoid_: script, sequence, inline macro (the pre-library form this replaced)

**Stepper**:
An Action that advances or retreats a cursor through a user-defined ordered list of items, firing the newly-selected item on every step — one motion per physical activation, never a separate select-then-confirm. Requires a *pair* of Bindings pointing at the same named Stepper list, one carrying the forward step and one the backward step (primary intended use is the scroll wheel's up/down, but any two Inputs qualify). Stepper lists live in one global, named library — defined once, reassignable to a different Input pair at any time; only one pair may reference a given list at once, and assigning it to a new pair silently moves it off the old one. A list item is a type distinct from Action, restricted to a single fire-once keyboard key or mouse-button, optionally held with a modifier combination (ticket 62's Answer — compiles through the same mods-down/key/mods-up sequence as Keypress), and designed to extend to joystick/controller buttons later — never a Macro or another Stepper. Stepping past either end of the list wraps around. The current position is per-list runtime state only, independent of Profile and Layer, and always resets to the list's first item on a Daemon restart — never written to `config.toml`. Trigger mode governs the step itself (Fire-once or Hold-to-repeat); Toggle is disallowed (see Trigger mode).
_Avoid_: cycle, carousel, weapon wheel (the primary use case, not the concept), select-then-confirm (the interaction model it deliberately isn't)

**Controller**:
An Action that emits a virtual-gamepad button press rather than a keyboard/mouse one, via a second `uinput` device (distinct from the existing keyboard device) advertising the standard Linux Gamepad Spec capability set. Reuses Binding/Trigger-mode/dispatch exactly like Keypress — no special-casing, and no hardcoded correspondence between a physical Input and a gamepad button code; any Input may be assigned any button. Covers buttons only, on any Input; continuous axis output is a distinct, structurally separate concept — see Axis assignment.
_Avoid_: gamepad (the kernel's own vocabulary for the device class, not Acheron's domain term), joystick (reserved for the broader "Controller/Joystick" strand name, not this Action specifically)

**Axis assignment**:
A per-Input, per-Layer assignment, structurally independent of Binding/Action, that makes a grid key's live Depth continuously drive one virtual gamepad axis rather than fire a discrete event — the same physical grid key can be Axis-assigned on one Layer and carry an ordinary Binding on the other. Mutually exclusive, for that Layer, with a Binding *or* Chord membership on the same Input, since an Axis-assigned key no longer produces the discrete Down/Up transitions either depends on; only grid keys are eligible (the thumbstick has no Depth to drive an axis with, and stays button-only). Reuses the key's own Actuation/Release points as its start/end thresholds rather than a separate deadzone, ramping linearly to raw Depth above the Actuation point. Assignable to one of 17 targets on the same gamepad `uinput` device Controller already uses: 5 unsigned single-key axes (Left/Right Trigger, Throttle, Gas, Brake) and 6 signed axes split into independently-assignable +/- halves (Left/Right Stick X and Y, Rudder, Wheel — 12 half-axis targets). Runtime conflicts resolve per axis: pressing both halves of one signed axis at once, the already-active key suppresses the other (a real stick can't move two directions at once); two keys sharing one same-signed target instead take the greater Depth.
_Avoid_: Action (Axis assignment is a parallel concept, not an Action variant — see Action), Controller (reserved for the discrete-button Action)

**Trigger mode**:
Governs how a Binding fires once its Input is pressed. One of Fire-once, Hold-to-repeat, Toggle, or Analog-repeat. Applies to every Binding, regardless of whether its Action is a Keypress or a Macro — except a Stepper's forward/backward Bindings, which disallow Toggle (there is no coherent continuously-running state for a cursor advance the way there is for a held Keypress or a looping Macro), and a Controller button Binding, which disallows Fire-once (ticket 78's Answer: Hold-to-repeat's sustained-hold behavior already covers a quick tap, and no real gamepad button press is decoupled from physical hold duration the way Fire-once's invented pulse is by design). Analog-repeat is further restricted to grid-key Bindings only, since it requires Depth.
_Avoid_: trigger type, activation mode

**Fire-once**:
The Trigger mode where the Action fires exactly once per physical press.

**Hold-to-repeat**:
The Trigger mode where the Action re-fires continuously for as long as the Input is physically held.

**Toggle**:
The Trigger mode where a single press starts the Action running continuously (looping, for a Macro; held down, for a Keypress) until the same Input is pressed again.

**Analog-repeat**:
The Trigger mode, grid-key-only, where the Action re-fires at a rate that varies continuously with Depth — slower near the deadzone, faster near full travel — rather than at Hold-to-repeat's fixed cadence. Starts once Depth crosses a small fixed deadzone (deliberately *not* the key's own Actuation point, so the rate curve gets the key's full travel range) and holds the key down solid, without further tapping, above a fixed near-full-travel threshold. Falls back to plain Hold-to-repeat when the Daemon is in Digital Capture mode (no Depth available). User-facing feature name: "Simulated Analog Key-Interlacing," for keyboard-driven driving sims and similar games where a player would otherwise hand-interlace keypresses to steer or accelerate.
_Avoid_: Simulated Analog Key-Interlacing (reserved for the user-facing/README name, not this domain term), analog mode (see Capture mode — a distinct concept)

**Depth**:
The 0-255 measurement of how far a grid key is physically pressed, available only while the Daemon is in analog Capture mode. `None`/absent for every Input without a depth sensor (the Mode key, thumbstick, wheel) and for a grid key while in digital Capture mode.
_Avoid_: pressure, travel

**Actuation point**:
The Depth at which a grid key's Binding is considered pressed (fires a Down). Scoped per-Input per-Profile — shared across a Profile's Base and Held Layers, since it describes the key's physical travel, not what it does when triggered.
_Avoid_: trigger point, threshold

**Release point**:
The (lower) Depth at which a grid key's Binding is considered released (fires an Up), paired with its Actuation point so a single boundary doesn't chatter (hysteresis).

### Runtime

**Daemon**:
The background process that owns the actual remapping logic — captures physical Input and produces the remapped Action's output. Runs continuously, independent of whether the GUI is open.
_Avoid_: driver, service, agent

**GUI**:
The interactive application through which the user edits Profiles, Bindings, and Macros, and monitors current state (active Profile, active Layer). Configures the Daemon; does not perform remapping itself.
_Avoid_: app, client, frontend

**Capture mode**:
Which of the Daemon's two ways of reading the grid keys is currently active — **Analog** (via the device's `hidraw` interface, carrying Depth) or **Digital** (the original evdev passthrough, no Depth). Digital is the automatic degradation path if the analog unlock fails; a separate user-facing override can force Digital even when Analog would otherwise work — the user never selects Analog as a normal path, only switches it off.
_Avoid_: driver mode (the research/prototype write-ups' working name for Analog), evdev mode (informal name for Digital)

**Output suppression**:
A connected client's request that the Daemon withhold all synthetic output while the request is active, without stopping anything internally — Trigger-mode firing, Macro looping, and a Toggle's running state continue unaffected, and only the write to the physical device is withheld. Distinct from a Toggle *stopping*: a suppressed Toggle is still active and resumes emitting the instant suppression clears. The GUI additionally stops every Toggle outright on its own window gaining focus (`StopAllToggles`, a separate call the GUI makes alongside suppression, not a side effect of suppression itself) — see spec.md's "Toggle behavior across Layer/Profile switches" and "Daemon output suppression" sections.
_Avoid_: pause, mute, disable (all imply something is stopped, not just withheld)
