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
What a Binding produces when triggered — either a Keypress or a Macro.
_Avoid_: output, effect

**Keypress**:
An Action that is a single remapped key or shortcut (may be a chord, e.g. Ctrl+Shift+T).
_Avoid_: single binding, simple action

**Macro**:
An Action that is a hand-specified sequence of Keypresses, each with its own delay before the next fires.
_Avoid_: script, sequence

**Trigger mode**:
Governs how a Binding fires once its Input is pressed. One of Fire-once, Hold-to-repeat, or Toggle. Applies to every Binding, regardless of whether its Action is a Keypress or a Macro.
_Avoid_: trigger type, activation mode

**Fire-once**:
The Trigger mode where the Action fires exactly once per physical press.

**Hold-to-repeat**:
The Trigger mode where the Action re-fires continuously for as long as the Input is physically held.

**Toggle**:
The Trigger mode where a single press starts the Action running continuously (looping, for a Macro; held down, for a Keypress) until the same Input is pressed again.

### Runtime

**Daemon**:
The background process that owns the actual remapping logic — captures physical Input and produces the remapped Action's output. Runs continuously, independent of whether the GUI is open.
_Avoid_: driver, service, agent

**GUI**:
The interactive application through which the user edits Profiles, Bindings, and Macros, and monitors current state (active Profile, active Layer). Configures the Daemon; does not perform remapping itself.
_Avoid_: app, client, frontend

**Output suppression**:
A connected client's request that the Daemon withhold all synthetic output while the request is active, without stopping anything internally — Trigger-mode firing, Macro looping, and a Toggle's running state continue unaffected, and only the write to the physical device is withheld. Distinct from a Toggle *stopping*: a suppressed Toggle is still active and resumes emitting the instant suppression clears. The GUI additionally stops every Toggle outright on its own window gaining focus (`StopAllToggles`, a separate call the GUI makes alongside suppression, not a side effect of suppression itself) — see spec.md's "Toggle behavior across Layer/Profile switches" and "Daemon output suppression" sections.
_Avoid_: pause, mute, disable (all imply something is stopped, not just withheld)
