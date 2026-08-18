Type: prototype
Blocked by: 17, 23

## Question

Design **how a user sets a grid key's trigger point**, and the live-depth channel that makes
it possible. Non-blocking for v1.0. This is the feature the whole analog strand exists to
deliver from a user's point of view — everything else is plumbing.

The key question is "how should it look and behave," so `/prototype` is the type: build a
throwaway to react to before committing to a shape in `binding_editor.py`.

Blocked on [ticket 23](./23-task-wire-analog-supervisor-and-install.md) as well as 17 — a
depression bar prototyping "live depth" needs live depth actually flowing through a running
Daemon to react to, not just the data model it's shaped by. (Added when ticket 18's grilling
session split the capture-path rework into tickets 21-23 and found this ticket's original
`Blocked by: 17` no longer matched what it needs.)

Settled during charting, build it in rather than re-litigating: **depth crosses the D-Bus
wire on request, not always.** The GUI enters a live-depth mode while the binding editor is
open and leaves it when closed. Always-on was rejected as a firehose — ticket 13 measured
~1 ms between reports while a key is moving, against a wire that today carries state
*changes*. Never-streamed was rejected because it makes setting an actuation point pure
guesswork: "set it to 137" is meaningless without seeing your own key travel. The user's
intended surface is a **depression bar** in the editor that moves as you press.

Settle at least:

- **The editor surface**: where the actuation point control and the live bar sit in
  `binding_editor.py` alongside the controls tickets 01/02/03/05/15 are adding, and whether
  the bar shows one key or all 20.
- **Units the user sees**: raw `0`–`255`, a percentage, or named presets (light/medium/heavy).
  Ticket 13 observed the full 8-bit range on every keycap, so the resolution is real — decide
  whether exposing it helps or just intimidates.
- **Hysteresis, if [ticket 17](./17-decide-analog-data-model.md) put a release point in the
  model**: does the user see and set two markers on the bar, or one with the release point
  derived?
- **The D-Bus shape**: the method or signal that starts and stops live-depth streaming, its
  payload, and its rate limiting. Note that the GUI already coordinates with the Daemon
  around focus (`StopAllToggles`, output suppression — see `CONTEXT.md`); decide whether
  live depth hooks the same lifecycle or its own.
- **What it does in digital mode**: the editor must still work when the Daemon degraded to
  evdev capture. Decide what the bar and the actuation-point control show when there is no
  depth to display — hidden, greyed, or shown with an explanation.
- **Discoverability**: how a user learns their keypad has this at all, given Synapse users
  will expect it and everyone else won't know to look.
- **`GetConfig()`'s wire dict** doesn't yet serialize `default_actuation`/
  `actuation_overrides` — [ticket 21](./21-task-apply-analog-data-model-to-code.md) deferred
  it here deliberately. Whatever binding-editor wiring this ticket lands needs to add it.

## Answer

_(unresolved)_
