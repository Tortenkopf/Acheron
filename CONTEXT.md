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
What a Binding produces when triggered — a Keypress, a Macro, a Stepper step, a Switch Profile, or a Controller button press.
_Avoid_: output, effect

**Keypress**:
An Action that is a single remapped key or shortcut (may be a modifier combination, e.g. Ctrl+Shift+T).
_Avoid_: single binding, simple action, chord (see Chord — a distinct concept)

**Chord**:
A Binding whose trigger is a *set* of two or more Inputs pressed together (open-ended size) rather than a single Input — fires its own Action exactly like an ordinary Binding (any kind except Switch Profile, which has nowhere to run from inside the Chord-detection state machine — ticket 40), scoped to Base/Held the same way. Detected via a fixed ~50ms window between the first and last member Input going down: if satisfied, the Chord's Action fires and each member's individual Binding is suppressed for that press; if the window elapses first (or a member releases early, without ever completing one), the pending member's individual Binding fires instead. Releasing any one member ends the Chord's held/toggle state as a whole, regardless of Trigger mode. An Input may belong to any number of Chords (needed for the thumbstick's four diagonals — Up sits in both Up-Left and Up-Right); `SetChordBinding` rejects at save time only a *subset/superset* relationship between two Chords' member sets (e.g. `{Up,Left}` vs `{Up,Left,ModeKey}`), since completing the smaller one is then indistinguishable from being partway into the larger one — a plain intersection, like the diagonals, is not a conflict. The thumbstick's four diagonals are ordinary 2-member Chords over adjacent cardinal Inputs, not a separate mechanism. The GUI records membership by clicking Inputs on Device Overview's own grid while a "Select Chord members" toggle is active, not live physical-press capture on the real device (ticket 30's original design intent) — a build-time scope cut, not a redesign.
_Avoid_: combo, simultaneous binding, key combination (reserved for a Keypress's modifier combination — see Keypress)

**Macro**:
An Action that fires a named, reusable sequence of Keypresses, each with its own delay before the next fires. A KeyDown/KeyUp step may also target a controller button (ticket 92's Answer — any `KeyCode` is accepted and the injector routes a gamepad code to the gamepad device); because most games poll controller input once per frame, the author must insert a Delay step of at least ~35 ms between such a button's Down and Up, and before pressing it again. Macro definitions live in one global, named library — defined once (`MacroId`, a slug frozen at creation and never rewritten, paired with a separately-editable display name), then referenced by any number of Bindings across any Profile at once (ordinary shared reuse, not exclusive-owner reassignment like Stepper). Deleting a Macro still referenced by a Binding is refused, so a dangling reference is structurally impossible.
_Avoid_: script, sequence, inline macro (the pre-library form this replaced)

**Stepper**:
An Action that advances or retreats a cursor through a user-defined ordered list of items, firing the newly-selected item on every step — one motion per physical activation, never a separate select-then-confirm. Requires a *pair* of Bindings pointing at the same named Stepper list, one carrying the forward step and one the backward step (primary intended use is the scroll wheel's up/down, but any two Inputs qualify). Stepper lists live in one global, named library — defined once, reassignable to a different Input pair at any time; only one pair may reference a given list at once, and assigning it to a new pair silently moves it off the old one. A list item is a type distinct from Action, and is either a single fire-once keyboard key or mouse-button — optionally held with a modifier combination (ticket 62's Answer — compiles through the same mods-down/key/mods-up sequence as Keypress) — or a single controller button (ticket 92's Answer — `StepperItem::ControllerButton`, compiled to the same down/dwell/up triple as a Controller button Action and routed to the gamepad device; no modifier combination). It is never a Macro or another Stepper. Stepping past either end of the list wraps around. The current position is per-list runtime state only, independent of Profile and Layer, and always resets to the list's first item on a Daemon restart — never written to `config.toml`. Trigger mode governs the step itself (Fire-once or Hold-to-repeat); Toggle is disallowed (see Trigger mode).
_Avoid_: cycle, carousel, weapon wheel (the primary use case, not the concept), select-then-confirm (the interaction model it deliberately isn't)

**Stepper cursor**:
A Stepper list's current position — where the next step starts from. Per-list runtime state the Daemon owns, one index per list, independent of Profile and Layer and never written to `config.toml`; a Daemon restart always begins with every list at its first item (index 0), and a list never yet stepped reports position 0. A step moves it one place (wrapping at either end) and fires the newly-selected item; editing a list's items reconciles it (clamped if the list shrank below it, dropped if the list was emptied or deleted). Lives in `daemon/src/stepper.rs` as `stepper::Cursors` (post-release ticket 12), fed by `edit::Effect::ReconcileStepperCursor` on a library edit and read by `GetState()` for the GUI.
_Avoid_: index, pointer, selection (reserve "selection" for the item a step lands on, not the position itself)

**Library**:
A global, named set of reusable Action definitions a Binding refers to rather than carrying inline — the Macro library and the Stepper library. Each entry is keyed by a slug frozen at creation (`MacroId` / `StepperId`) paired with a separately-editable display name, and holds an ordered list of items fired in sequence. Both libraries are Profile- and Layer-agnostic. How widely one entry may be shared differs by kind: a Macro is ordinary shared reuse (any number of Bindings, across any Profile at once); a Stepper list is held by exactly one forward/backward Input pair and moved wholesale when reassigned. An entry still referenced by any Binding — the scan spans every Profile's Base/Held *and* Chord Layers (`edit::macro_references` / `stepper_references`, mirrored GUI-side) — cannot be deleted, so a dangling reference is structurally impossible. The GUI presents both through one screen (`gui/acheron_gui/library_view.py`) parameterised over a `LibraryKind` adapter that carries the per-kind noun, wire shape, item label, and "add item" controls (post-release ticket 13); the kind-agnostic half — the browse list, rename/delete with its reference-count guard, and the ordered-item editor frame — is written once.
_Avoid_: registry, store, palette, collection (as a bare synonym)

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
The Trigger mode where a single press starts the Action running continuously (looping, for a Macro; held down, for a Keypress or a Controller button — ticket 78 gave Controller button the same sustained-hold treatment already given a mouse-button Keypress, ticket 82, and Controller button's own Hold-to-repeat, ticket 75/76: no real gamepad button has a "turbo" Toggle mode any more than it autorepeats) until the same Input is pressed again.

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

**Actuation stage**:
A `(ActuationPoint, Binding)` pair — an Actuation/Release point paired with a Binding it fires. Every grid key's ordinary Binding is its **primary stage**; a grid key may additionally carry a **deep stage**, a second Actuation stage whose band sits strictly deeper (`deep.release > primary.actuation` — the two bands are disjoint and stacked, never overlapping). A deep stage requires a primary stage on the same Input/Layer to exist at all — deleting the primary cascade-deletes the deep stage. Capped at two stages (primary + deep); key travel makes a third impractical. Silently inert in Digital Capture mode (no Depth to threshold against) — only the primary stage fires. The deep stage's Binding is scoped per-Input per-Layer, like the primary's; its Actuation/Release point and Staging mode are scoped per-Input per-Profile, shared across Base and Held, like the primary's own Actuation point. User-facing feature name: "Dual-stage keys."
_Avoid_: sub-binding, second binding, layer (Actuation stage is a depth concept, unrelated to the Base/Held Layer)

**Staging mode**:
A per-Input, per-Profile choice governing how a grid key's primary and deep Actuation stages hand off as Depth crosses the deep band: Handoff, No-Return, Additive, or Quick-Skip. Shared across Base and Held, like the deep stage's own Actuation point — it interprets physical travel, not what either stage does when triggered. A key with no deep stage has no meaningful Staging mode.
_Avoid_: transition mode, handoff mode (reserved for the Handoff mode specifically)

**Handoff**:
The Staging mode where crossing into the deep band releases the primary stage and presses the deep stage, and crossing back out releases the deep stage and re-presses the primary — exactly one stage held at a time, symmetric (the "camera shutter" model). The default Staging mode.

**No-Return**:
The Staging mode identical to Handoff on the way deeper, but the primary stage does not re-press on the way back up — once the deep stage has fired, the key stays quiet until fully released and pressed again.

**Additive**:
The Staging mode where both stages fire and are held simultaneously — reaching the deep band adds the deep stage's firing without releasing the primary.

**Quick-Skip**:
The Staging mode where the primary stage's Down is held back for a ~50ms window (the Chord-detection window's constant, reused): if the deep band is reached within that window, the primary is suppressed entirely for the rest of the press (never fires, and its eventual release does not fire it either — the release path behaves like No-Return); otherwise the primary fires late (delayed by up to the window) and the key runs as ordinary Handoff for the rest of the press. Costs up to 50ms of primary-Down latency by construction — the price of not knowing, at the moment of the primary crossing, whether the press will continue into the deep band.

**Status LED**:
One of the three fixed-colour (orange, green, blue) on/off indicator LEDs on the device's left side. On/off only — no brightness, no custom colour, no non-static effect (all hardware limits). Driven only by the active Profile's Status LED assignment, never by a Binding or a Layer.
_Avoid_: profile LED, keymap indicator (Razer's Synapse term), Chroma (the separate per-key backlight)

**Status LED assignment**:
The per-Profile triple of on/off states for the three Status LEDs. Every Profile has one (default all-off); the Daemon asserts the active Profile's assignment on Profile switch, on Daemon startup, and on every device (re)connect — the firmware reclaims the LEDs to its orange-only default on every USB enumeration, so re-assertion is mandatory, not an optimisation. Cleared to all-off on clean Daemon exit. Stored as a `[profiles.<name>.status_leds]` table in `config.toml`.
_Avoid_: LED profile, LED state (too vague — reserve for the momentary hardware condition)

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

**Physical-plausibility ceiling**:
The ceiling on how fast the Daemon emits synthetic key/button events: no holding or repeating Action produces events faster than the Linux input stack does for a physically held key — the machine's configured kernel autorepeat delay/period (`analog::read_kernel_auto_repeat`, fallback 250 ms / 33 ms) — and a held mouse or gamepad button emits exactly one Down/Up with no repeat. A held or repeated single key presents as genuine kernel autorepeat (`value=1` then `value=2` on the real delay→period envelope), not a stream of Down/Up pairs. A Macro is the sole deliberate exception: its author sequences Keypresses at any cadence, and only *trigger-driven Macro repetition* is floored (to `max(kernel period, MIN_TOGGLE_LAP)`); a Macro fired once and the keystroke cadence within one run are unrestricted, and Analog-repeat cannot wrap a Macro. Not a disguise — the `uinput` origin is always detectable (ADR-0008); the goal is plausibility of *rate*. The Daemon's compliance with this ceiling was audited surface by surface in `.scratch/humane-output-rate/`.
_Avoid_: humane output rate (the working name of the effort that established this — `.scratch/humane-output-rate/`, not a domain term), humanization, anti-cheat evasion, input sanitisation, jitter

### Interface

**Toast label**:
A transient one-shot notice in the GUI — a short highlighted line shown once, immediately after the action that triggered it, and cleared on the next redraw of that view. Reports what just happened (a Stepper list moved off its former Input pair; an axis target already claimed). Never blocks, has no dismiss control, does not reappear.
_Avoid_: notification (reserve for OS-level), banner, alert, snackbar

**GUI hint**:
A persistent advisory line in the GUI, shown for as long as its condition holds and removed once it no longer applies — dim, inline, non-blocking, no dismiss control. Attaches a caveat to a choice the user currently has in effect (a Macro step targeting a controller button; Analog-repeat selected as a Trigger mode; the standing macro-editor caution, whose condition is simply "the Macro editor is open"). Distinct from a Toast label, which fires once and vanishes regardless of state.
_Avoid_: tooltip (hover-only — a distinct thing), inline warning, disclaimer (the macro-editor line is one instance, not the general term)
