# Decide the self-DoS macro guardrail

Type: grilling
Status: resolved
Blocked by: 01
Parent: [Humane output rate](../map.md)

## Question

Macros are the deliberate exception to the humane-output-rate bar — an author may sequence
Keypresses at any cadence. But ticket 26 showed a zero-delay Macro under Toggle compiles to
`[KeyDown, KeyUp]` with no `Delay` and loops as fast as the injector allows — an unbounded
keystroke flood that froze the focused app, then the whole input pipeline, hard enough to
need a power cycle. `MIN_TOGGLE_LAP` (20 ms) now floors the *Toggle→Macro* loop lap, but
nothing floors:

- a Macro with many near-zero `Delay` steps (or none) fired repeatedly under Hold-to-repeat
- a very long Macro with no delays run once (a burst, not a loop)
- (whatever else ticket 01's audit surfaces about the Macro paths)

Decide the guardrail, if any:

1. **Text only** — the disclaimer + tips (ticket 05) warn about it; no code.
2. **Soft warning** — the macro editor flags a Macro whose step timing could run away
   (e.g. total delay below a threshold, or a step count / delay ratio heuristic),
   non-blocking. Where the check lives (editor-only, or `config::validate` /
   `config::binding`) is part of this decision.
3. **Hard floor** — an absolute minimum effective cadence enforced at execution
   (generalise `MIN_TOGGLE_LAP` beyond Toggle→Macro), regardless of the Macro's `Delay`
   steps. Blocks the footgun but caps a legitimate fast Macro.

### Ticket 01's ratification narrowed this (2026-09-06)

- **The pathologically fast kernel autorepeat config is *out of scope*** (ticket 01, Q4) —
  it does not fold in here. Following the live kernel rate is the bar as written.
- **Charon's lean (ticket 01, Q6): option 3, scoped to *trigger-driven Macro repetition*.**
  Floor the **Toggle→Macro loop-lap** and the **Hold-to-repeat→Macro re-fire cadence** to
  `executor::combine_toggle_lap_target`'s value (`max(live kernel period, MIN_TOGGLE_LAP)`),
  i.e. generalise the existing `MIN_TOGGLE_LAP` floor beyond Toggle→Macro. A Macro fired
  **once**, and the keystroke cadence *within* one run of a multi-step Macro, stay
  unrestricted. Surface 10's within-a-single-run burst (a multi-step no-delay Macro
  re-fired under Hold-to-repeat — the overlap guard throttles *runs*, not the N keystrokes
  inside a run) is the one case option 3 does *not* mechanically cover; decide whether that
  falls to soft-warning (option 2) or is accepted.
- **If a trigger-looped Macro's body is effectively one key down/up**, route it through
  ticket 07's `value=2` path so it presents as a held key rather than a tap loop. This
  ticket decides *whether* that routing exists; ticket 07 specs *how*.

Output feeds ticket 07 (the `value=2` spec is `Blocked by` this ticket for the degenerate
single-key-Macro routing).

Output feeds ticket 04 (the ADR must state the exception precisely) and ticket 05 (the tips
describe whatever the editor actually does). Any code the decision calls for is built on
this map (execution is in scope) — likely as a fix ticket graduated from here.

## Answer

Ratified with Charon in a `/grilling` + `/domain-modeling` session (2026-09-06). The audit
traced every trigger-driven Macro path in the daemon first; the wrinkle it surfaced —
**the inter-run cadence is already floored on every path** — narrowed the decision to the
within-run burst, the degenerate single-key case, and one path that gets banned outright.

### What the code actually does today

| Path | Cadence *between* whole-Macro runs | Cadence *within* one run |
|---|---|---|
| **Toggle → Macro** (`executor::run_toggle_loop`) | **already floored** — `target_lap = combine_toggle_lap_target(live kernel period) = max(kernel period, MIN_TOGGLE_LAP 20 ms)`, measured from `lap_start`, so a zero-`Delay` Macro's lap is padded to `target_lap` | **unfloored** — N `[KeyDown,KeyUp]` steps with no `Delay` fire back-to-back at lap start |
| **Hold-to-repeat → Macro** (surface 10) | **already bounded** — each run needs a fresh kernel `EventState::Repeat` (~kernel `REP_PERIOD`); the `Slot::FiringUnfinished` overlap guard in `trigger::decide` *drops* Repeats that land mid-run; ticket 06 adds the stall clamp | **unfloored** — same within-run burst |
| **Analog-repeat → Macro** | ≤ 20 Hz, Depth-driven | none — `analog_repeat::fire_analog_repeat_pulse` collapses the whole Macro to one simultaneous pulse, **ignoring embedded `Delay` steps** |
| **Macro fired once** (Fire-once; or a long no-`Delay` burst) | n/a | **unfloored**, but bounded by step count and the injector's `mpsc::channel(256)` backpressure through the single serialized `uinput` fd — self-terminating, *not* ticket 26's unbounded loop |

Stepper → Macro does not exist (a Stepper item is never a Macro — CONTEXT.md). Chord →
Hold-to-repeat → Macro rides surface 10 via `chord::feed_repeat`'s single leader member.

### Decisions

1. **The generalised floor needs no new runtime code (Q1/Q4).** "Generalise
   `MIN_TOGGLE_LAP` beyond Toggle→Macro" (Charon's ticket-01 lean) turns out to be already
   satisfied: `run_toggle_loop`'s `target_lap` floors the Toggle path, and kernel-`Repeat`
   arrival + the `FiringUnfinished` overlap guard bound the Hold-to-repeat path. There is
   no path on which a Trigger mode *re-fires a whole Macro* faster than
   `max(kernel period, MIN_TOGGLE_LAP)`. → **ticket 08**, a **test-only** fix ticket:
   regression tests pinning the invariant on both paths + comments in `run_toggle_loop`
   and at the overlap guard naming this decision as the load-bearing mechanism. No
   production logic change, no new constant.

2. **The within-a-single-run keystroke burst is accepted (Q2 → option a).** A multi-step
   Macro with near-zero `Delay` steps still emits its N keystrokes in a sub-millisecond
   bunch — once per lap under Toggle, once per kernel Repeat under Hold-to-repeat, or once
   outright when fired once. This stays **unrestricted**: Macros are the deliberate
   exception, the once-fired burst is bounded (step count + injector backpressure, not
   ticket 26's infinite loop), and any heuristic sharp enough to catch a real footgun
   would also cry wolf on legitimate fast combos (fighting-game inputs, rapid
   weapon-switch macros). **No editor warning, no `config::validate` check.** Covered by
   ticket 05's disclaimer + best-practice tips (text only).

3. **The degenerate single-key Macro routes through ticket 07's `value=2` path (Q3).**
   A Macro repeated by **Toggle or Hold-to-repeat** whose compiled body is effectively one
   key down/up should present as a *held key* (genuine kernel autorepeat) rather than a
   fast tap loop. This ticket records the **decision that the routing exists**; **ticket
   07 owns the exact detection predicate** and the mechanism, since it is designing the
   `value=2` path anyway. Ticket 07's Question item 6 lands here rather than deferring
   back. (Analog-repeat → Macro is out — see decision 4.)

4. **Analog-repeat + Macro is disallowed outright (Q5).** Firing a Macro through an
   Analog-repeat Binding has no coherent meaning — `fire_analog_repeat_pulse` already
   collapses a multi-step Macro to a single simultaneous pulse with its `Delay` steps
   ignored — and no useful reason to exist. → **ticket 09**: a new `ConfigError` variant,
   rejected at `SetBinding` (GUI cannot save it) **and** at `load_or_seed` (the Daemon
   refuses to start on a hand-edited `config.toml` carrying it), matching every sibling
   restriction (`InvalidChordAnalogRepeat`, `AnalogRepeatOnDualStageKey`, Fire-once
   `ControllerButton` refused, Toggle `Step` refused). An existing offending config is
   **refused at start** with a clear message — no silent coercion. Ticket 09 also updates
   the CONTEXT.md **Trigger mode** entry with a third exception in its "except…" list
   (Stepper/Toggle, Controller/Fire-once, **Analog-repeat/Macro**). No ADR — the sibling
   restrictions were recorded as CONTEXT.md + ticket ref, not ADRs.

### Downstream

- **Ticket 07** — Question item 6 now lands here: spec the single-key-Macro detection
  predicate and route it through the `value=2` held-key path (Toggle + Hold-to-repeat).
- **Ticket 04 (ADR)** — the ADR must state the exception precisely: trigger-driven Macro
  *repetition* is floored to `max(kernel period, MIN_TOGGLE_LAP)` (by existing mechanism,
  not a new floor); a Macro fired **once** and the keystroke cadence **within one run**
  stay unrestricted; **Analog-repeat cannot wrap a Macro**.
- **Ticket 05 (spec)** — the disclaimer/tips cover the accepted within-run burst (text
  only, no widget); the Analog-repeat + Macro combo no longer needs a tip because it is
  rejected at the config layer.
- **Map** — the "speed ceiling on a Macro" Out-of-scope line rewritten (below).

### New tickets

- **08** — Lock the trigger-driven Macro-repetition floor with regression tests (`task`,
  test-only). Blocked by 01.
- **09** — Disallow the Analog-repeat + Macro Binding (`task`). Blocked by 01.
