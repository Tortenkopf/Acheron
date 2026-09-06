# Spec the kernel-shaped repeat behaviour (`value=2`)

Type: grilling
Status: open
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
