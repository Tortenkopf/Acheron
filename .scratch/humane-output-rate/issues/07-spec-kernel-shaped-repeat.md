# Spec the kernel-shaped repeat behaviour (`value=2`)

Type: grilling
Status: resolved
Blocked by: 01, 03
Parent: [Humane output rate](../map.md)

> Deliverable is a gated spec handed to a fresh implementation effort — the one behaviour
> change on this map that is *not* built inline (ticket 01, Q5). Mirrors how ticket 05's UI
> work is handled.

## Question

Produce `.scratch/humane-output-rate/spec-kernel-shaped-repeat.md` for making Acheron's
Hold-to-repeat output present as **genuine kernel autorepeat** rather than a stream of
`[KeyDown, KeyUp]` pairs.

### Background (from ticket 01's audit)

- Every Hold-to-repeat path — Digital capture (surface 1), Analog-capture synthesized
  (surface 2), and via Chord / Stepper (surfaces 3, 4) — ultimately runs `keypress_steps`
  = `[KeyDown, KeyUp]` once per `EventState::Repeat`, emitting `value=1` then `value=0`
  with ~0 ms dwell.
- A physically held key on Linux repeats with **`value=2`** after one `REP_DELAY` gap, then
  at the shorter steady `REP_PERIOD` (`input.c` `input_repeat_key`; ticket 02 research §4).
- Acheron's uinput device does **not** advertise `EV_REP` (verified: `evdev` 0.13.2's
  `VirtualDeviceBuilder` has no repeat method; `injector::build_device` declares only keys
  + relative axes), so the kernel does not softrepeat a held key on it — a bare held
  `KeyDown` produces zero downstream autorepeat.
- The bar as written is rate-only, so the pair shape *passes* — but Charon's call is to fix
  it, and to be able to state plainly in a heuristics report that this virtual device
  plays by the rules and is human-driven, not automated.

### The spec must settle

1. **Implementation fork.**
   - **(a) Inject `value=2` ourselves** — a new `MacroStep` / injector capability
     (`KeyEvent::new(code, 2)`), driven by a scheduler that owns the delay→period envelope.
     Full per-binding rate control; more moving parts.
   - **(b) Advertise `EV_REP` + `EVIOCSREP`** on the virtual device and let the kernel
     repeat held keys at a configured rate. Fewer moving parts for the steady phase; less
     control over the initial-delay-vs-steady distinction; couples every held key on the
     device (check interaction with Toggle-held mouse/gamepad — `BTN_*` is not
     repeat-eligible, so likely fine, but confirm).
   - Pick one, with rationale.

2. **Which paths convert.** All Hold-to-repeat paths (1–4) so behaviour does not depend on
   capture mode. Chord/Stepper inherit via the shared executor. Confirm Digital's
   1:1-with-kernel timing is preserved.

3. **The envelope.** One initial `~REP_DELAY` gap then steady `~REP_PERIOD`, sourced from
   the live `read_kernel_auto_repeat()` value (fallback 250 / 33 ms) — the same source
   surface 2 already uses. How the first `Down` relates to the first `value=2`.

4. **Analog-repeat's hold-solid phase (surface 5).** Depth ≥ `ANALOG_REPEAT_HOLD_SOLID`
   (235) currently fires a bare `KeyDown` and holds. The spec must make that phase present
   as genuine autorepeat too, so the tap-ramp → hold-solid transition reads as a hand
   ramping its tapping rate up until it blends into a held key. The tapping band below 235
   keeps its current pulse shape (deliberately human — ticket 20).

5. **Missed-deadline clamping.** Ticket 06 adds it to today's loops; the replacement
   scheduler must keep it (kernel `input_repeat_key` re-arms from *now*, never bursts).

6. **The degenerate single-key Macro.** Ticket 03 (resolved 2026-09-06) decided this
   routing **exists**, for **Toggle → Macro and Hold-to-repeat → Macro**, and handed the
   detection predicate and mechanism to *this* ticket (it did not land in 03). Spec:
   - the exact predicate — e.g. "compiled steps are exactly `[KeyDown(k), KeyUp(k)]` for a
     single `k`, ignoring at most one trailing `Delay`"; decide whether a modifier-wrapped
     single key (`keypress_steps` with `Modifiers`) also qualifies;
   - where it is detected (executor, when compiling the Toggle loop steps / on the
     Hold-to-repeat `SpawnFireOnce` path) and how it swaps the tap loop for the `value=2`
     held-key stream;
   - that a Macro *not* matching the predicate keeps today's behaviour (Toggle loop floored
     at `target_lap`; Hold-to-repeat re-fire gated by kernel `Repeat`).
   Analog-repeat → Macro is **not** in scope here — ticket 09 bans that combination.

7. **Force-release / stop semantics.** A `value=2` stream needs a terminating `value=0` on
   physical `Up` / stop / force-release, same as today's pairs. Confirm ticket 33's
   `force_release_stuck` path still balances it.

8. **The heuristics-report rationale.** A short written section the ADR (ticket 04) and any
   future heuristics report can cite: uinput origin is always visible (ADR-0002); what this
   change buys is that the *rate and shape* of held/repeated output are now
   indistinguishable-in-timing from a physical hold, and the only variable input — the
   Analog-repeat ramp — is continuously hand-driven. Not disguise; plausibility of rate +
   human-in-the-loop.

Invoke `/grilling` and `/domain-modeling`. Feeds ticket 04 (ADR must describe the decision)
and ticket 05 (the tips describe the post-change reality). Implementation is a fresh effort.

## Answer

Grilled + ratified with Charon (2026-09-06). Deliverable:
[`spec-kernel-shaped-repeat.md`](../spec-kernel-shaped-repeat.md) — gated, ready for a fresh
implementation effort. Summary of the decisions:

1. **Implementation fork → inject `value=2` ourselves.** `evdev` 0.13.2 cannot enable
   `EV_REP` on a `VirtualDevice` (no builder method; `sys`/`EVIOCSREP` private;
   `UI_SET_EVBIT` must precede the `build()`-internal `UI_DEV_CREATE`). Option (b) would
   need hand-rolled `libc` uinput ioctls + a dispatch refactor. `injector::translate`
   already emits `value=2` for passthrough — precedent that our device carries autorepeat.
   New primitives: `InjectorMessage::RepeatKey` / `Injector::repeat_key` →
   `KeyEvent::new(code, 2)` (subject to `suppressed`); `TriggerDecision::RepeatKey`;
   `hold_repeat_kind(&Action, &macros) -> Option<HoldKind>` extending `sustained_hold_key`
   (`SustainedNoRepeat` = mouse/gamepad, Repeat→Nothing; `AutorepeatKey(mods,code)` =
   single key, Repeat→`RepeatKey`); pure `single_held_key(&[MacroStep])` predicate.

2. **Paths converted:** Digital Hold-to-repeat (1), Analog-synth Hold-to-repeat (2), Chord
   Hold-to-repeat for single-key Actions (3), Analog-repeat hold-solid ≥ 235 (5), Toggle →
   keyboard Keypress (6), Toggle → single-key Macro (6b). **Not converted:** Stepper
   Hold-to-repeat (4 — no single held key; each Repeat targets a different item), Toggle →
   mouse/gamepad button (7/8 — `BTN_*` never autorepeats), Toggle → multi-step Macro (9 —
   the sole remaining user of `run_toggle_loop` + `target_lap`), Hold-to-repeat →
   multi-step Macro (10 — ticket 08). Digital's 1:1 kernel timing is preserved (one
   `value=2` per incoming `EventState::Repeat`).

3. **Envelope:** no envelope of the spec's own — `value=1` at press, first `value=2` when
   the path's existing driver says a repeat is due, steady at that driver's period. Full
   `REP_DELAY`→`REP_PERIOD` for surfaces 1/2 and Toggle (a Toggle-held key *is* a held
   key); **no initial delay** for hold-solid (top of a hand-driven ramp). Default
   timestamps (kernel monotonic stamp, like `input_repeat_key`'s `ktime_get()`).

4. **Hold-solid (surface 5):** `run_analog_repeat_loop` gains its own `RepeatSchedule` +
   `solid_since`/`solid_fired`; the `HoldSolid` arm replaces its `select!` park with a
   `value=2` emitter at `period_ms`. Tap band < 235 unchanged (ticket 20).

5. **Missed-deadline clamp:** surface 2's `advance_fired` (ticket 06) is untouched — it
   decides *when*, we change *what*. Toggle + hold-solid emitters adopt the identical
   `advance_fired` discipline (a `_steady` sibling for the no-`delay_ms` hold-solid case).
   `tap_pace_wait` unchanged.

6. **Single-key predicate:** `[KeyDown(k), KeyUp(k)]`; modifier-wrapped single key
   (modifiers held `value=1`, only `k` autorepeats — real kernel behaviour); either + at
   most one trailing `Delay`. Anything else (>1 key, `Delay` between keys) keeps today's
   behaviour. Detected in `Slots::perform`/`decide` (`&macros` threaded in); the predicate
   fn stays pure over `&[MacroStep]`, unit-tested standalone.

7. **Force-release/stop:** held `value=1` tracked in `held` exactly as `D::HoldKeyDown`
   today (plus modifiers); `value=2` events are stateless. All six teardown paths
   (individual `Up`, chord dissolve, `stop_all`, Toggle second press, analog cancel /
   depth-leaves-solid, dropped connection) already emit the terminating `value=0`. Ticket
   33's `force_release_stuck` unchanged. Required test: a queued `RepeatKey` when `Up`
   arrives must never land after the `value=0`.

8. **Heuristics rationale** (spec §8, lifted verbatim into ticket 04's ADR-0008): origin
   stays visible and accepted; the change makes held/repeated output timing-
   indistinguishable from a physical hold (value, envelope, dwell); the Analog-repeat ramp
   is continuously hand-driven. Plausibility of rate, not disguise. **Cite ADR-0008 (new),
   not ADR-0002** — ADR-0002 is about direct evdev/uinput vs OpenRazer and says nothing
   about detectability. No ADR in the repo currently records the premise.

### Cross-ticket effects (done on this resolution)

- **Ticket 08** rewired `Blocked by: 01, 07`; test note added (single-key Macro Toggle
  rides `value=2`; multi-step Macro Toggle paces at `target_lap`; chord-keyed
  Hold-to-repeat→Macro shares the seam).
- **Ticket 04** note updated: glossary term + new ADR cite ADR-0008, not ADR-0002.
- **Map** Decisions-so-far + an ADR-0002-correction line under Notes.
- Tickets 04 and 05 now unblock (all their blockers resolved).
