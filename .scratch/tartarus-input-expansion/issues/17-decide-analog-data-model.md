Type: grilling
Blocked by: 16

## Question

Decide **how analog depth is represented** across the Daemon's event stream, the `Binding`
model, the config schema, and the D-Bus wire. This is the one analog ticket promoted into
the v1.0 required floor (see the map's Destination): the capture rework and every feature
above it stay non-blocking, but the *model* must be settled before the remaining
Binding-editor tickets (01, 02, 03, 05, 15) write their half of `binding_editor.py` and the
config schema against a shape analog will force us to break.

Deliberately **not** wired as a blocker on those five — see the map's standing discipline.
It is simply the ticket to take next.

Settle at least:

- **Does `PhysicalEvent` widen to carry depth?** `daemon/src/capture/mod.rs` currently
  defines `PhysicalEvent { input: Input, state: EventState }` with `EventState` as
  `Down`/`Repeat`/`Up`, and its doc comment states the stream is the *only* contract
  anything downstream relies on. The cheap option is for an analog `CaptureSource` to
  threshold depth internally and emit exactly today's `PhysicalEvent`, changing nothing
  downstream. That cannot support the Analog-repeat Trigger mode ([ticket
  20](./20-decide-analog-repeat-trigger-mode.md)), live depth in the GUI ([ticket
  19](./19-prototype-trigger-point-ux-and-live-depth.md)), or real analog axes (ticket 14),
  all of which need depth to reach `dispatch.rs`. Decide the widened shape — an optional
  depth field, a separate variant, a parallel channel — and what it means for `fake.rs` and
  the 72 existing Daemon tests.
- **Where does the actuation point live?** Per-`Binding`, per-`Input` per-Profile, a global
  default with per-Binding override, or per-Profile only. Note the asymmetry: an actuation
  point is a property of a *physical grid key*, while a `Binding` is scoped to a
  Profile/Layer pair — so a naive per-Binding field means the same key can have two
  different actuation points in Base and Held, which may or may not be wanted.
- **One threshold or two?** A single threshold chatters at the boundary. Decide whether the
  model carries an actuation point plus a separate (lower) release point — hysteresis — and
  whether the user sees both or the release point is derived.
- **What happens to the 14 non-grid Inputs?** Mode key, thumbstick ×4, wheel ×3 have no
  depth and never will. Decide whether the model makes depth structurally optional on
  `Input` (so a thumbstick Binding simply has no actuation point), or whether grid keys
  become a distinct type. Depends on [ticket 16](./16-task-analog-mode-hardware-facts.md)'s
  finding on whether those Inputs even survive driver mode.
- **How is device mode represented, and what does the config/wire say about it?** Per the
  map's Notes the digital path survives as an automatic degradation path plus an explicit
  user-facing force-digital override. Decide where that override lives (config file, D-Bus
  call, both), how the Daemon reports which mode it actually landed in, and what the GUI
  shows when the user asked for analog and got digital.
- **Config migration**: whether an existing `config.toml` written by the shipped MVP still
  loads unchanged, and what an actuation point defaults to when absent.

## Answer

_(unresolved)_
