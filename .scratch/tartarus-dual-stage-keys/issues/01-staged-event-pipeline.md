Type: grilling
Blocked by: —
Status: resolved

## Question

Decide **how a staged grid-key press flows through the capture-source → dispatch
pipeline**. This is the keystone ticket — the state machine (ticket 02), the config/D-Bus
surface (ticket 03), and the interaction sweep (ticket 05) all build on the answer.

Grilling + `/domain-modeling` + `/codebase-design` against the real code. Decision only —
no build (a small throwaway spike is allowed if inspection can't settle it; capture it on
a `research/staged-event-pipeline` branch with a pointer from here).

### Settled inputs (from charting — do not re-open)

- A grid key has a **primary stage** and an optional **deep stage**, each a full
  `Binding`. Deep stage's band is strictly above the primary's: `deep.release >
  primary.actuation` (disjoint, stacked).
- Deep **Binding** is per-Layer; deep **ActuationPoint** + **staging mode** are per-Input
  per-Profile, shared Base/Held.
- Four staging modes: Handoff, No-Return, Additive, Quick-Skip (definitions in `map.md`
  Notes). Quick-Skip needs the ~50ms Chord window and the Chord-style "suppress the
  primary Down" mechanism.
- Deep stage is inert in Digital Capture mode.

### The two routes

- **(a) Generalize the capture source.** `observe(prev, depth, point)` (`analog.rs:235`)
  becomes `observe(prev, depth, &[ActuationPoint])` (or similar), tracks which of up to 2
  bands the key is in, and emits **stage-tagged** events `(input, stage, state)`.
  Dispatch's `InputEvent` grows a `stage` field. Staging-mode logic (handoff, suppression,
  the 50ms window) lives... where? Capture source or dispatch?
- **(b) Keep the capture source single-threshold; stage in dispatch.** The capture source
  keeps emitting the primary Down/Up from `observe()` unchanged. A **third depth engine**
  in the dispatch task (sibling of `handle_depth_update` / `update_analog_repeats`, fed by
  `rx_depth`) watches the deep band per key, runs the staging-mode state machine, and
  drives the deep stage's firing — and, for Quick-Skip, retroactively suppresses or delays
  the primary. **Leaning (b)** — depth interpretation already converges in dispatch, and
  the capture source stays a dumb hysteresis reporter.

### Settle at least

- Route (a) vs (b), with the module seam named.
- Where the staging-mode state machine lives and whether it is a pure, table-tested unit
  (like `chord`, `analog_repeat`, `axis`).
- How Quick-Skip holds back / suppresses the primary Down when route (b) means the
  capture source has *already emitted* it — buffer in dispatch for 50ms? Have dispatch
  own the primary-Down emission for dual-stage keys? This is the sharpest sub-question.
- What the deep stage's Down/Up firing path is (it can't come from `observe()` in route
  (b)) — synthetic `InputEvent`, direct `trigger::` call, or a new path.
- Whether `rx_depth`'s ~30Hz throttle (`DepthChanged`, `analog.rs:969`) is fast enough for
  the 50ms Quick-Skip window, or the staging engine needs the un-throttled stream.
- Whether the ADR (Q15) is warranted by the chosen route (it is, if (b)).

### Output

An `## Answer` section: the chosen route, the module seam, the state-machine home, the
Quick-Skip primary-suppression mechanism, and a **drafted ADR** ("dual-stage depth
interpretation runs in dispatch, not the analog capture source") for ticket 06 to file.

## Answer

Resolved by a grilling + `/domain-modeling` + `/codebase-design` session against the real
pipeline (`capture/analog.rs`, `dispatch.rs`, `trigger.rs`, `analog_repeat.rs`, `chord.rs`,
`main.rs`), 2026-09-03.

### Grounding fact established by inspection

Dispatch's `rx_depth` is the **raw, un-throttled** live-Depth `watch` channel.
`capture::analog::relay_grid_blocking` does `depth_tx.send_replace(...)` on **every**
incoming hidraw `0x06` report (sub-millisecond while a key is moving, per ticket 13). The
~30 Hz limit (`DEPTH_STREAM_INTERVAL`, `dbus/mod.rs`) is applied **only** in the D-Bus
`run_depth_stream` pump for the GUI's `DepthChanged` signal — dispatch holds its own clone
of the un-throttled `depth_rx` (`main.rs:127/155`; ticket 71). `handle_depth_update` (Axis)
and `update_analog_repeats` both already run off this raw snapshot in the
`rx_depth.changed()` `select!` arm. **The staging engine needs no new un-throttled path;
the 50 ms Quick-Skip window has many samples inside it** unless dispatch is starved > 50 ms,
a pathological stall the Axis / Analog-repeat engines already tolerate.

### 1. Route: **(b)** — capture stays single-threshold; stage in dispatch

`capture::analog` is unchanged. `observe(prev, depth, point)` (`analog.rs:235`) keeps its
6-line body and its table tests. `PhysicalEvent` gains **no** `stage` field. All staged-
depth interpretation — deep-band hysteresis, the staging-mode state machine, deep-stage
firing, Quick-Skip primary suppression — lives in a new **dispatch-side depth engine**,
the third sibling of `handle_depth_update` and `update_analog_repeats`.

Why (b) over (a):

- The pure `observe()` becomes a **shared hysteresis primitive**: capture calls it for the
  primary band; the dispatch staging engine calls the *same function* for the deep band
  (`observe(prev_deep, depth, deep_point)`). Route (a) would rewrite `observe` into a
  multi-band form (`&[ActuationPoint]`) and complicate its body and table tests for no
  gain.
- Route (a) still can't host the staging-mode logic (handoff / additive / the 50 ms
  suppression) in capture without making capture `Config`-aware and mode-stateful — so
  that logic lands in dispatch **either way**, and (a) just adds a stage-tag round trip
  through `PhysicalEvent`.
- (b) matches the established pattern exactly: depth interpretation that needs
  `Config` / active-Layer converges in the dispatch task. Ticket 71's own code comment
  articulates this as the reason Axis `depth → value` resolution is not in capture.

### 2. Module seam: pure `daemon/src/stage.rs` + `stage::Engine` on `DispatchState`

**Pure core** (`stage.rs`), importing nothing from `dispatch` / `edit` / `chord` /
`config::Config` — the `chord` / `axis` / `analog_repeat` discipline:

- `fn advance(...) -> Vec<StageOp>` (or a single `StageOp` per call) — the staging-mode
  state machine. Inputs: the per-key prior staging state, the primary-band and deep-band
  `observe()` transitions for this tick, the staging `Mode`, and (for Quick-Skip) whether
  the 50 ms window is still open. Output is a **data-only** `StageOp` the dispatch shell
  performs: `FirePrimary` / `ReleasePrimary` / `RepressPrimary` / `FireDeep` / `ReleaseDeep`
  / `SuppressPrimary` / `Nothing`. Exhaustively table-tested per mode (ticket 02 fills in
  the exact sequences).
- `fn next_deadline(&Engine) -> Option<Instant>` and `fn tick(&mut Engine, now) -> Vec<StageOp>`
  — the Quick-Skip timeout trio, mirroring `chord::next_deadline` / `chord::tick`.

**Non-pure `stage::Engine`** on `DispatchState` (sibling field to `axis` / `analog_repeat`),
owning the runtime bookkeeping the pure core is handed:

- per-key **deep-band `KeyState`** (fed to `observe()` each tick) and **primary-band
  `KeyState`** shadow (the engine tracks the primary band itself off the depth stream, so
  it never needs the primary `PhysicalEvent` for Handoff/No-Return/Additive);
- per-key **Quick-Skip runtime state**: primary press pending? deadline `Instant`? did this
  press resolve to *skip* or to *Handoff*?
- the deep stage's slots — `firings: HashMap<StageKey, FiringHandle>` +
  `toggles: HashMap<StageKey, ActiveToggle>`, exactly `ChordRuntime`'s shape, because each
  stage is a full independent `Binding` (charting Q10) and **Additive holds both stages at
  once**.

> **Post-ticket-15 note (2026-09-04):** post-release-development ticket 15 landed
> `trigger::Slots<K>` — the `(firings, toggles)` pair with both maps private and
> `slot` / `snapshot` / `perform` / `force_release` / `stop_toggle` /
> `stop_all_toggles` as methods. `stage::Engine` should hold a `Slots<StageKey>`
> here instead of hand-rolling the pair a fourth time; the free
> `perform_trigger<K>` / `slot_for` / the `trigger_ctx!` macro named in §4 below
> are gone (`trigger::compile_action` moved alongside `perform`). The §2/§4 design
> intent stands as written — spec ticket 06 integrates the new surface.

`Engine::update(&mut self, config, active_layer, &snapshot) -> Vec<(target, StageOp)>` is
called from the existing `rx_depth.changed()` arm, third after `handle_depth_update` and
`update_analog_repeats`. Deep `ActuationPoint` comes from the active Profile (a
`resolved_deep_actuation_point(input)` accessor — ticket 03's config surface).

### 3. Quick-Skip primary suppression: dispatch-side buffer, modeled on the Chord machine

The Chord machine already solves this exact shape — buffer a member `Down` ~50 ms, then
either claim it (Chord fires, individual Binding suppressed) or release it retroactively
via `dispatch_individual_down` (`dispatch.rs:421`, "the pending member's individual Binding
fires retroactively, delayed by the window").

- **Only Quick-Skip dual-stage keys** intercept the primary edge. `handle_event` gets one
  narrow insertion, at the binding-lookup point where the `AnalogRepeat`-swallow sits
  today (`dispatch.rs:262`), gated on `event.depth.is_some()` exactly like that swallow —
  so a Digital-mode primary press never diverts and the deep stage is inert for free
  (charting Q7). It checks: active-Layer profile has a deep stage on `event.input` **and**
  its staging mode is Quick-Skip.
  - `Down` → `state.stage.begin_quick_skip(input, depth)`, then `return Ok(Vec::new())`.
    `begin_quick_skip` resolves **immediately to *skip*** if the deep band is already hot
    (the `rx_depth` arm processed the deep crossing first — `tokio::select!` is unordered);
    otherwise it arms the 50 ms deadline (`stage::next_deadline` now returns `Some`).
  - `Up` → consult `stage` runtime state: **swallow** if this press resolved to *skip*
    (the primary never fired, so capture's `Up` is unbalanced — tolerated the way
    `trigger::force_release_stuck` already tolerates a lingering entry); otherwise fall
    through to the normal `Up` path.
- **Deadline elapses with no deep crossing** → the new 4th `select!` arm
  (`wait_for_stage_deadline(stage::next_deadline(&state.stage))` → `stage::tick`) fires the
  primary retroactively via `dispatch_individual_down(config, input)` and flips this key's
  runtime state to run as **Handoff** for the rest of the press. Cost: ≤ 50 ms primary-Down
  latency, by construction (charting Q3).
- **Handoff / No-Return / Additive keys**: `handle_event` is **unchanged**. Capture's
  primary `Down`/`Up` fire normally through the ordinary path. The `stage::Engine` watches
  depth independently and manages only the *deep excursion*.

No ordering hazard between the `rx_events` and `rx_depth` arms: the deep-band crossing
emits no `PhysicalEvent` (`observe()` only transitions on the primary band), so the
deep-crossing report wakes only the `rx_depth` arm. The one multi-arm-ready instant is the
primary-actuation report, and the deep band is provably not hot there (bands are disjoint
and stacked, `deep.release > primary.actuation`). `begin_quick_skip` tolerates being called
at any depth ≥ `primary.actuation`.

### 4. Deep-stage firing path: direct `perform_trigger` keyed by `StageKey`

`perform_trigger<K: Eq + Hash + Clone>` (`dispatch.rs:935`) is already generic over the
slot key — the individual path uses `Input`, the Chord path uses `ChordKey`. The deep
stage adds a third: **`StageKey(Input)`** (newtype; the primary keeps bare `Input`).

- The dispatch shell performs a `FireDeep` / `ReleaseDeep` `StageOp` by calling
  `trigger::decide(&deep_binding, state, slot)` + `perform_trigger(decision, StageKey(input), &deep_binding, &mut ctx)`
  against `state.stage.firings` / `state.stage.toggles` — **no synthetic `PhysicalEvent`
  round trip**.
- Handoff/No-Return `ReleasePrimary` / `RepressPrimary` `StageOp`s reach into dispatch's
  **primary** `Input` keyspace: `trigger::force_release_stuck(&state.in_flight, &input, …)`
  / `trigger::stop_toggle(&mut state.toggles, &input)` to release, `dispatch_individual_down(config, input)`
  to re-press. So the `stage` shell touches **both** keyspaces — `StageKey` for the deep
  slot, `Input` for the primary — which is expected and precedented (the Chord path's
  `FireIndividual` / `ForceReleaseIndividual` effects already reach into the `Input`
  keyspace from the Chord executor).

### 5. Teardown seam

`stage::Engine::stop_all()`, wired to the **same sites** as `analog_repeat.stop_all()`:
`handle_layer_switch` (`dispatch.rs:1001`), `handle_capture_mode_change` (`:1057`), the
`run_effects` stop-all effect (`:575`), and the disconnect path. It force-releases every
live deep slot (`trigger::force_release_stuck` over the `StageKey` map, exactly the Chord
teardown) and drops any pending Quick-Skip primary. **Whether the primary re-fires or
re-presses after such a release is ticket 05's decision** — this ticket only guarantees
nothing stays stuck.

### 6. No spike

Inspection settled every sub-question. The state-machine event sequences are ticket 02's
job and are a design decision, not a feasibility question — no running prototype needed.
The `research/staged-event-pipeline` branch was not created.

### 7. `rx_depth` throttle — answered

Not applicable / already fast enough: dispatch's `rx_depth` is the raw un-throttled stream
(see "Grounding fact" above). The staging engine rides it in the existing
`rx_depth.changed()` arm.

---

## Drafted ADR — for [ticket 06](./06-write-dual-stage-spec.md) to file as `docs/adr/0007-dual-stage-depth-interpretation-in-dispatch.md`

> **# 0007. Dual-stage depth interpretation runs in the dispatch task, not the analog capture source**
>
> **Status:** Accepted
>
> **Context**
>
> "Dual-stage keys" lets a grid key in analog Capture mode carry a second Actuation stage,
> firing at a Depth strictly deeper than the primary, with a user-selectable staging mode
> (Handoff / No-Return / Additive / Quick-Skip) governing how the two stages hand off.
>
> The analog capture source (`daemon/src/capture/analog.rs`) is deliberately a **dumb
> hysteresis reporter**: it thresholds each grid key's raw Depth against **one**
> `ActuationPoint` via the pure `observe()` and synthesizes one Down/Up (plus Hold-to-repeat
> `Repeat`s) per key. It knows nothing of Layers, Bindings, Trigger modes, or Chords.
> `Config`- and Layer-aware interpretation of Depth already converges in the **dispatch
> task**: Axis-assignment `depth → axis_value` resolution (`handle_depth_update`) and the
> Analog-repeat spawn/stop + rate curve (`update_analog_repeats`) both run there off the
> live-Depth `watch` channel, precisely because only dispatch owns the `Config` / active-Layer
> state they need.
>
> Two routes were considered:
> - **(a)** Generalize `observe()` to multiple bands and emit stage-tagged events, growing
>   `PhysicalEvent` with a `stage` discriminator.
> - **(b)** Leave the capture source untouched; add a third dispatch-side depth engine that
>   owns deep-band hysteresis, the staging-mode state machine, and deep-stage firing.
>
> **Decision**
>
> Route **(b)**. A new pure module `daemon/src/stage.rs` (state machine + Quick-Skip timeout
> trio) plus a non-pure `stage::Engine` on `DispatchState`, the third sibling of
> `axis::Engine` and `analog_repeat::Engine`.
>
> - `capture::analog` and `observe()` are unchanged; `PhysicalEvent` gains no field.
> - The staging engine calls the **same** pure `observe()` for the deep band that capture
>   calls for the primary band.
> - The deep stage fires via the existing generic `perform_trigger<K>`, keyed by a new
>   `StageKey(Input)`, into `ChordRuntime`-shaped `firings` / `toggles` maps on the engine.
> - Quick-Skip's requirement to suppress or delay the primary Down — which capture has
>   already emitted — is met by a **dispatch-side buffer modeled on the Chord-detection
>   machine**: for Quick-Skip keys only, `handle_event` diverts the primary edge to the
>   engine, which either resolves to *skip* (deep band reached within the 50 ms window) or
>   fires the primary retroactively via `dispatch_individual_down` and runs the key as
>   Handoff. A new 4th `select!` arm drives the timeout, mirroring `wait_for_chord_deadline`.
> - `stage::Engine::stop_all()` is wired to the same teardown sites as
>   `analog_repeat.stop_all()`.
>
> **Consequences**
>
> - The capture source stays trivially testable and free of feature creep; `observe()`
>   remains a 6-line pure function shared by both bands.
> - The dispatch task — already large — gains one more depth engine and one more `select!`
>   arm.
> - Quick-Skip costs up to 50 ms of primary-Down latency **by construction**; this was
>   accepted during charting (Q3) as the price of the mode.
> - The `stage` shell reaches into two keyspaces (`StageKey` for the deep slot, `Input` for
>   the primary), the same shape the Chord executor already has.
> - This refines the spirit of the existing "dispatch owns Depth interpretation" pattern
>   (Axis, Analog-repeat); it does **not** supersede or refine ADR-0002 (evdev/uinput vs
>   OpenRazer).
