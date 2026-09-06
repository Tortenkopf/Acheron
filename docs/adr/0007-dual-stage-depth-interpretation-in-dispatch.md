# Dual-stage depth interpretation runs in the dispatch task, not the analog capture source

"Dual-stage keys" lets a grid key in analog Capture mode carry a second Actuation stage —
its own Actuation/Release pair plus its own Binding — firing at a Depth strictly deeper than
the primary stage, with a user-selectable staging mode (Handoff / No-Return / Additive /
Quick-Skip) governing how the two stages hand off (see `.scratch/tartarus-dual-stage-keys/spec.md`).

The analog capture source (`daemon/src/capture/analog.rs`) is deliberately a dumb hysteresis
reporter: it thresholds each grid key's raw Depth against **one** `ActuationPoint` via the pure
`observe()` and synthesizes one Down/Up (plus Hold-to-repeat `Repeat`s) per key. It knows
nothing of Layers, Bindings, Trigger modes, or Chords. `Config`- and Layer-aware interpretation
of Depth already converges in the dispatch task instead: Axis-assignment `depth → axis_value`
resolution (`handle_depth_update`) and the Analog-repeat spawn/stop + rate curve
(`update_analog_repeats`) both run there off the live-Depth `watch` channel, precisely because
only dispatch owns the `Config` / active-Layer state they need.

We considered generalizing `observe()` to multiple bands and emitting stage-tagged events,
growing `PhysicalEvent` with a `stage` discriminator. Rejected: the staging-mode logic
(handoff, suppression, the ~50ms Quick-Skip window) can't live in capture without making it
`Config`-aware and mode-stateful, so that logic lands in dispatch either way — a stage-tagged
`PhysicalEvent` would just add a round trip for no gain, and it would complicate `observe()`'s
6-line body and table tests to serve a caller (capture) that doesn't need the extra bands.

Instead, `capture::analog` and `observe()` are unchanged; `PhysicalEvent` gains no field. All
staged-depth interpretation — deep-band hysteresis, the staging-mode state machine, deep-stage
firing, Quick-Skip primary suppression — lives in a new pure module `daemon/src/stage.rs` plus
a non-pure `stage::Engine` on `DispatchState`, the third depth engine alongside `axis::Engine`
and `analog_repeat::Engine`:

- The staging engine calls the **same** pure `observe()` for the deep band that capture calls
  for the primary band — a shared hysteresis primitive, not a rewritten one.
- The deep stage fires through `trigger::Slots<K>` (the `(firings, toggles)` handle pair —
  post-release ticket 15), keyed by a new `StageKey(Input)`, the same way the Chord path uses
  `Slots<ChordKey>` and the individual path uses `Slots<Input>`.
- Quick-Skip's requirement to suppress or delay the primary Down — which capture has already
  emitted by the time the deep band is even reachable — is met by a dispatch-side buffer
  modeled on the Chord-detection machine: for Quick-Skip keys only, `handle_event` diverts the
  primary edge to the engine, which either resolves to *skip* (deep band reached within the
  window) or fires the primary retroactively via `dispatch_individual_down` and runs the key as
  Handoff for the rest of the press. A fourth `select!` arm drives the timeout, mirroring
  `wait_for_chord_deadline`.
- `stage::Engine::stop_all()` is wired to the same teardown sites as `analog_repeat.stop_all()`
  (Layer switch, Profile switch, the Digital-mode capture flip), plus a new explicit disconnect
  hook and a per-key `stop_stage()` for cascade-delete — none of these had a generic hook to
  reuse; each is new machinery (see `.scratch/tartarus-dual-stage-keys/issues/05-interaction-sweep.md`).

Consequences: the capture source stays trivially testable and free of feature creep —
`observe()` remains a 6-line pure function shared by both bands. The dispatch task — already
large — gains one more depth engine and one more `select!` arm. Quick-Skip costs up to 50ms of
primary-Down latency by construction; this was accepted during charting as the price of the
mode. The `stage` shell reaches into two keyspaces (`StageKey` for the deep slot, `Input` for
the primary), the same shape the Chord executor already has.

This refines the spirit of the existing "dispatch owns Depth interpretation" pattern (Axis,
Analog-repeat established it); it does not supersede or refine ADR-0002 (evdev/uinput vs
OpenRazer), which is about the capture/injection transport, not where Depth is interpreted.
