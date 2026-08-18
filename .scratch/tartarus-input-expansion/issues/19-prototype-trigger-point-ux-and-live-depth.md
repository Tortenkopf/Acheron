Type: prototype
Status: resolved
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

Prototyped three structurally different variants in a throwaway GTK4 app (real
`binding_editor.py` widget on top, a variant-specific Actuation section below it,
switchable via a bottom pill): **A** inline/one-marker/raw, **B** inline/two-marker/
percent+badge, **C** a separate all-20-key calibration overview with presets. No real
hardware in this environment, so a `Gtk.Scale` simulated the physical key (drag or
auto-sweep) and a button flipped simulated capture mode to preview the digital fallback.
Captured on the `prototype/19-trigger-point-depth-ux` branch (not on `main` — see that
branch's commit for the full three-variant code); `main` keeps only this decision.

**Winner: variant B**, refined over two rounds of live reaction:

- **Editor surface**: inline in `binding_editor.py`, directly below the existing
  Trigger-mode/Action controls, for the single Input currently open in the editor — not a
  separate all-20 overview (that was variant C; rejected as an extra click for the common
  case of tuning the key you're already looking at).
- **The bar's width**: spans the full width of the editor/dialog (not a fixed narrow
  control) — caught and fixed twice from live screenshots: first the fill (100% depth)
  fell short of the bar's actual right edge because the fill math used a hardcoded pixel
  constant while the bar itself had already stretched to its container's width; the fix
  makes both the bar and its fill/marker math track the real allocated width (and stay
  correct across a live window resize), rather than pinning the bar to a fixed size.
- **Units**: percentage (e.g. "66%"), not raw `0`-`255` — raw felt like it needed
  interpretation Actuation/percentage doesn't.
- **Hysteresis**: two independent, explicitly draggable markers — green for Actuation
  (fires Down), amber for Release (fires Up) — not one derived from the other. Dragging
  enforces `release < actuation` by clamping. The legend text below the bar colors the
  words "green"/"amber" themselves to match their markers, so the mapping reads at a
  glance rather than requiring the reader to cross-reference a color swatch.
- **Digital-mode fallback**: shown with an explanation, not hidden — the bar greys out
  (dimmed, insensitive) and a warning ("No depth — analog capture unavailable") sits
  centered *on top of* the greyed bar in a dark, legible pill, rather than as a separate
  line of text below it (moved there after the first live reaction — a below-the-bar note
  read as disconnected from what it was explaining).
- **Discoverability**: a badge next to the "Actuation & release" heading doubles as a
  *live* capture-mode indicator, not a static "this key supports analog" label — green
  "analog" / warm-red "digital", flipping with the Daemon's actual reported mode. The
  fold-in note in the prototype: this same badge belongs in Device Overview too, not only
  this editor, so a user can tell at a glance which mode they're in without opening a
  Binding.
- **The D-Bus shape** (design sketch via the prototype's `SimDepth`/`CaptureModeState`,
  not yet built against the real Daemon): a `StartDepthStream(input)` /
  `StopDepthStream(input)` request pair, scoped to the requesting client's own bus
  connection (auto-stopped on disconnect — mirrors `SetOutputSuppressed` being
  request-scoped rather than globally toggled) plus a `DepthChanged(input, depth)` signal,
  rate-limited to roughly 30Hz — well under the ~1ms-per-change rate ticket 13 measured on
  real hardware, so the wire never gets the firehose the charting session's "always-on was
  rejected" note warned about. Independent of `StopAllToggles`/output-suppression's
  lifecycle (streaming depth doesn't need to suppress output, and vice versa). Starts when
  the editor opens for a grid Input, stops when it closes, matching the charting session's
  "on request, not always" call.
- **`GetConfig()`'s wire dict**: still doesn't serialize `default_actuation`/
  `actuation_overrides` — confirmed still open, deferred to the build ticket below.

Spawned [Build the trigger-point UX and live-depth channel for real](./26-task-build-trigger-point-depth-ux.md)
to land this design against the real Daemon D-Bus surface, `binding_editor.py`, and live
hardware — this ticket settled the shape from a throwaway; nothing here is wired into the
real GUI yet.
