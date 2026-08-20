Type: grilling
Blocked by: 17, 23
Status: resolved

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

`TriggerMode` gets a fourth variant, `AnalogRepeat` — a separate, explicitly-chosen mode, never
a silent depth-modulation of existing Hold-to-repeat Bindings (which would change an existing
Binding's behavior out from under the user the moment Analog Capture mode turns on).

**Start/stop**: gated by a small, **fixed, hardcoded deadzone** — deliberately *not* the key's
own tunable Actuation/Release points. Reusing the Actuation point would waste however much of
the key's travel the user set it to on "off," and the whole point of this mode is to feel like a
real analog axis using (close to) the key's full physical travel. Structurally this reuses the
existing `observe()`-shaped Depth→transition hysteresis, just with hardcoded constants standing
in for the per-key `ActuationPoint` it normally reads from Config.

**Rate curve**: linear, mapped across the key's full 0–255 Depth range (not renormalized to an
actuation/release band). Bounds (min/max Hz) are hardcoded dispatch.rs constants, not
per-Binding configurable, for this fast-follow — see the fog note below for future tunability.

**Each fire**: a Down+Up pulse of a fixed short duration, not Depth-scaled — only the *frequency*
of taps varies with Depth, not how long each tap holds the key down.

**Full deflection**: above a fixed near-full-travel threshold, the key holds down solid
(continuous Down, no further tapping) rather than continuing to fire at the curve's max rate —
what a driving-sim player actually wants for full throttle.

**Digital Capture mode fallback**: a Binding set to Analog-repeat has no Depth once the Daemon
degrades to Digital. Falls back to plain Hold-to-repeat at the kernel-autorepeat cadence — the
same behavior every other grid key already has in Digital mode, rather than doing nothing or
firing at one arbitrary fixed rate.

**Applicability**: grid-key Bindings only. The GUI greys out "Analog-repeat" as a Trigger-mode
option for non-Grid Inputs, mirroring the existing `is_grid_input()` gate already used for the
Actuation & release section — structurally prevents the nonsensical case rather than needing a
runtime fallback for an Input with no Depth sensor.

**Architecture**: `dispatch.rs` takes its own receiver on the existing `depth_tx` watch channel
(report-rate, already flowing to the D-Bus layer for `DepthChanged` — mirrors how it already
holds `actuation_tx`), rather than relying on `PhysicalEvent.depth` as carried by the capture
layer's own synthesized `Repeat` events, which are throttled to the (coarser) kernel-autorepeat
cadence and would cap how responsive the rate curve could feel. Analog-repeat runs as a **new
per-Input background task** — its own `HashMap<Input, _>` tracking alongside the existing
`toggles`/`in_flight` maps — spawned when Depth crosses the fixed deadzone going up, cancelled
crossing back down, recomputing its own next sleep duration from the latest Depth on every tick.
Structurally closer to `ActiveToggle`'s existing spawned-task-per-Input shape than to
Hold-to-repeat's reactive branch in `fire()`, since the rate has to vary continuously between
whatever discrete events the capture layer happens to emit.

**Numbers** (deadzone threshold, min/max Hz, fire duration, hold-solid threshold): left
TBD — tuned live against the real device by the build ticket, not decided blind here. Spawned
[Build and tune Analog-repeat on hardware](./39-task-build-analog-repeat.md).

CONTEXT.md gained the Analog-repeat glossary entry; the Trigger-mode entry's "one of Fire-once,
Hold-to-repeat, or Toggle" is corrected to name all four.
