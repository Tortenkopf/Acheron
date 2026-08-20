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
A Binding whose trigger is a *set* of two or more Inputs pressed together (open-ended size) rather than a single Input — fires its own Action (Keypress or Macro) exactly like an ordinary Binding, scoped to Base/Held the same way. Detected via a fixed ~50ms window between the first and last member Input going down: if satisfied, the Chord's Action fires and each member's individual Binding is suppressed for that press; if the window elapses first, the pending member's individual Binding fires instead, delayed by the window. Releasing any one member ends the Chord's held/toggle state as a whole. A given Input may belong to at most one Chord per Profile/Layer — overlapping Chord definitions are rejected at save time. The thumbstick's four diagonals are ordinary 2-member Chords over adjacent cardinal Inputs, not a separate mechanism.
_Avoid_: combo, simultaneous binding, key combination (reserved for a Keypress's modifier combination — see Keypress)

**Macro**:
An Action that fires a named, reusable sequence of Keypresses, each with its own delay before the next fires. Macro definitions live in one global, named library — defined once (`MacroId`, a slug frozen at creation and never rewritten, paired with a separately-editable display name), then referenced by any number of Bindings across any Profile at once (ordinary shared reuse, not exclusive-owner reassignment like Stepper). Deleting a Macro still referenced by a Binding is refused, so a dangling reference is structurally impossible.
_Avoid_: script, sequence, inline macro (the pre-library form this replaced)

**Stepper**:
An Action that advances or retreats a cursor through a user-defined ordered list of items, firing the newly-selected item on every step — one motion per physical activation, never a separate select-then-confirm. Requires a *pair* of Bindings pointing at the same named Stepper list, one carrying the forward step and one the backward step (primary intended use is the scroll wheel's up/down, but any two Inputs qualify). Stepper lists live in one global, named library — defined once, reassignable to a different Input pair at any time; only one pair may reference a given list at once, and assigning it to a new pair silently moves it off the old one. A list item is a type distinct from Action, restricted to a single fire-once keyboard key or mouse-button (designed to extend to joystick/controller buttons later) — never a Macro or another Stepper. Stepping past either end of the list wraps around. The current position is per-list runtime state only, independent of Profile and Layer, and always resets to the list's first item on a Daemon restart — never written to `config.toml`. Trigger mode governs the step itself (Fire-once or Hold-to-repeat); Toggle is disallowed (see Trigger mode).
_Avoid_: cycle, carousel, weapon wheel (the primary use case, not the concept), select-then-confirm (the interaction model it deliberately isn't)

**Controller**:
An Action that emits a virtual-gamepad button press rather than a keyboard/mouse one, via a second `uinput` device (distinct from the existing keyboard device) advertising the standard Linux Gamepad Spec capability set. Reuses Binding/Trigger-mode/dispatch exactly like Keypress — no special-casing, and no hardcoded correspondence between a physical Input and a gamepad button code; any Input may be assigned any button. Currently covers buttons only — axis output (e.g. the thumbstick as an analog stick, depth-driven triggers) is a distinct, not-yet-designed capability, not part of this term yet.
_Avoid_: gamepad (the kernel's own vocabulary for the device class, not Acheron's domain term), joystick (reserved for the broader "Controller/Joystick" strand name, not this Action specifically)

**Trigger mode**:
Governs how a Binding fires once its Input is pressed. One of Fire-once, Hold-to-repeat, or Toggle. Applies to every Binding, regardless of whether its Action is a Keypress or a Macro — except a Stepper's forward/backward Bindings, which disallow Toggle: there is no coherent continuously-running state for a cursor advance the way there is for a held Keypress or a looping Macro.
_Avoid_: trigger type, activation mode

**Fire-once**:
The Trigger mode where the Action fires exactly once per physical press.

**Hold-to-repeat**:
The Trigger mode where the Action re-fires continuously for as long as the Input is physically held.

**Toggle**:
The Trigger mode where a single press starts the Action running continuously (looping, for a Macro; held down, for a Keypress) until the same Input is pressed again.

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
