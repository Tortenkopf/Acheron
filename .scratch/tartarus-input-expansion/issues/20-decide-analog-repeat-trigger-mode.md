Type: grilling
Blocked by: 17, 23

## Question

Design **Analog-repeat**, a fourth Trigger mode that varies how often a Binding re-fires
according to how deep its grid key is pressed. Non-blocking for v1.0.

The use case, in the user's words: keyboard-driven driving sims and similar games where the
player manually interlaces keypresses to steer or accelerate — press lightly and the key
taps slowly, press harder and it taps faster, giving analog-feeling control to a game that
only understands discrete keys. Raised as "Simulated Analog Key-Interlacing"; **Analog-repeat**
is the canonical domain term (short, matches the existing Fire-once/Hold-to-repeat/Toggle
pattern), with the longer phrase kept as the user-facing feature name for the README.

Settled during charting, build it in: this is a **Trigger mode**, not an Action. CONTEXT.md
defines a Trigger mode as what "governs how a Binding fires once its Input is pressed,"
which is exactly what this does — so it composes with any Action for free, rather than
needing its own Action kind. It is Hold-to-repeat with a depth-driven rate.

This ticket is why depth has to reach `dispatch.rs` at all rather than being thresholded
away inside the capture layer — see [ticket 17](./17-decide-analog-data-model.md).

Blocked on [ticket 23](./23-task-wire-analog-supervisor-and-install.md) as well as 17 —
designing a depth-driven firing rate needs real depth events reaching `dispatch.rs` from a
running Daemon to test against, not just the model. (Added when ticket 18's grilling session
split the capture-path rework into tickets 21-23 and found this ticket's original
`Blocked by: 17` no longer matched what it needs.)

Settle at least:

- **The depth→frequency curve**: linear between a minimum and maximum rate, or something
  with more resolution near the top? What are sane default bounds in Hz, and does the user
  configure them per-Binding?
- **What "fires" means at rate**: a full press/release pair per tick presumably — decide the
  key-down duration, and whether it is fixed or also scales with depth (a longer contact at
  full press is arguably more faithful to how a human interlaces).
- **Relationship to Hold-to-repeat**: is Analog-repeat a separate fourth mode the user picks,
  or does Hold-to-repeat simply *become* depth-modulated on grid keys when analog is active?
  The second is tempting and probably wrong — it changes existing Bindings' behavior silently.
- **Behavior below the actuation point and at full deflection**: does firing start exactly at
  the actuation point, and is there a "hold solid" region at the top of travel where it stops
  tapping and just holds the key down? That last one is likely what a driving sim player
  actually wants for full throttle.
- **What it does in digital mode**: a Binding configured Analog-repeat has no depth when the
  Daemon degrades to evdev capture. Decide whether it falls back to plain Hold-to-repeat,
  fires at a fixed rate, or does nothing — and how the GUI communicates that.
- **Applicability**: grid keys only, since nothing else has depth. Decide what the GUI does
  when the user selects Analog-repeat for a thumbstick or wheel Binding.
- **CONTEXT.md**: on resolution, write the Analog-repeat glossary entry and correct the
  Trigger-mode entry, which currently says "one of Fire-once, Hold-to-repeat, or Toggle."

## Answer

_(unresolved)_
