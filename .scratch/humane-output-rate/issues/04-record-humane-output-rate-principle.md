# Record the humane-output-rate principle — ADR + CONTEXT.md term

Type: grilling
Status: resolved
Blocked by: 01, 07
<!-- 07 resolved 2026-09-06 (spec-kernel-shaped-repeat.md). For this ADR:
     - The heuristics-report rationale is spec §8 — lift it verbatim as ADR-0008's rationale
       block (origin visible + accepted; the change makes held/repeated output
       timing-indistinguishable from a physical hold — value, envelope, dwell; the
       Analog-repeat ramp is hand-driven; plausibility of rate, not disguise).
     - **ADR-0002 does NOT cover uinput-origin detectability** — it is "direct evdev/uinput
       vs OpenRazer" only. No ADR records the premise. This ADR (0008) is the first. Fix the
       "(ADR-0002)" citation in the glossary draft below → "(ADR-0008)".
     - value=2 decision to record: inject it ourselves (evdev 0.13.2 can't enable EV_REP on
       a VirtualDevice); converts Digital/Analog-synth Hold-to-repeat, Chord (single-key),
       Analog-repeat hold-solid, and Toggle → single key; NOT Stepper, NOT button Toggles,
       NOT multi-step Macro loops. -->
<!-- Analog-repeat cannot wrap a Macro (ticket 03 decision 4 / ticket 09) — note it in the
     ADR and the glossary term. Cite ticket 08's mechanism (run_toggle_loop target_lap + the
     FiringUnfinished overlap guard), not a new constant, for trigger-driven Macro repetition.
     Both blockers (01, 07) are resolved as of 2026-09-06 — this ticket is on the frontier. -->

<!-- 03 (self-DoS macro guardrail) resolved 2026-09-06 — see issues/03. Key points for the ADR:
     trigger-driven Macro *repetition* is floored to max(kernel period, MIN_TOGGLE_LAP) by
     EXISTING mechanism (run_toggle_loop's target_lap + the FiringUnfinished overlap guard),
     not a new floor; a Macro fired once and the keystroke cadence within one run stay
     unrestricted; Analog-repeat cannot wrap a Macro (ticket 09). Cite ticket 08 (the
     invariant's regression tests), not a new constant. -->
Parent: [Humane output rate](../map.md)

## Question

Write the durable record so future repeat-based features inherit the invariant without
re-deriving it:

1. **A new `docs/adr/` entry** (next number after 0007) — the decision that synthetic
   output never exceeds the physical-plausibility ceiling, its rationale (some games watch
   for inhuman input rates; uinput origin is already detectable so this is about rate
   plausibility, not disguise), the bar (live kernel autorepeat delay/period; held buttons
   one Down/Up), the Macro exception as reframed by ticket 01/03 (once-fired and
   within-run cadence free; *trigger-driven repetition* floored), and the list of surfaces
   the ticket-01 audit brought into line. Note the existing constants that implement it
   (`MIN_TOGGLE_LAP`, `combine_toggle_lap_target` / `resolve_toggle_lap_target`,
   `RepeatSchedule` kernel seeding, `run_toggle_held` / `spawn_held`,
   `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`). Also record:
   - the **missed-deadline clamp** (ticket 06) on both repeat pace loops;
   - the **`value=2` kernel-shaped repeat** decision (ticket 07) — that Hold-to-repeat
     emits genuine autorepeat with the real delay→period envelope, not `[KeyDown,KeyUp]`
     pairs — and its heuristics-report rationale (this virtual device's held/repeated
     output is timing-indistinguishable from a physical hold, and the only variable input,
     the Analog-repeat ramp, is continuously hand-driven — plausibility of rate +
     human-in-the-loop, not disguise);
   - that a **pathologically fast kernel `kbdrate`** is explicitly *not* guarded against
     (ticket 01, Q4).

2. **A `CONTEXT.md` glossary term** — draft:

   > **Humane output rate**: The ceiling on how fast the Daemon emits synthetic
   > key/button events — no holding or repeating Action produces events faster than the
   > Linux input stack does for a physically held key (the kernel's configured autorepeat
   > delay/period), and a held mouse/gamepad button emits exactly one Down/Up with no
   > repeat. A Macro is the sole deliberate exception: its author sequences Keypresses at
   > any cadence, subject only to <ticket-03 guardrail>. Not a disguise — `uinput` origin
   > is always detectable (ADR-0002); the goal is plausibility of *rate*.
   > _Avoid_: humanization, anti-cheat evasion, input sanitisation.

   Refine wording against `/domain-modeling` and the terms it touches (Trigger mode,
   Hold-to-repeat, Analog-repeat, Toggle, Macro).

Invoke `/domain-modeling`. Cross-check against ADR-0001/0002 for tone and against any
`_Avoid_` collisions.

## Answer

Grilled with Charon (`/domain-modeling`, 2026-09-06). Both artifacts written.

### 1. ADR — `docs/adr/0008-physical-plausibility-ceiling-for-synthetic-output.md`

Title: *"Synthetic output holds to a physical-plausibility ceiling; held keys present as
genuine kernel autorepeat."* Prose, seven short paragraphs matching the 0006/0007 house
density. Records, in order:

- **The rationale** — some games/anti-cheat flag inhuman rate + regularity (cites the
  `research/` file, not a fresh derivation); the `uinput` origin is already visible and
  that is **accepted, not fought**; plausibility of *rate*, not disguise of *origin*; no
  jitter / anti-regularity injection. This ADR is the **first** place the
  "uinput-origin-always-detectable" premise is recorded — the closing paragraph states
  ADR-0002 is neither its basis nor refined by it (0002 is transport only).
- **The ceiling** — live kernel autorepeat delay/period (`analog::read_kernel_auto_repeat`,
  fallback 250/33); one Down/Up for a held `BTN_*`; steady-state rate is the invariant,
  initial-repeat delay judged per surface. Enforced by existing constants
  (`MIN_TOGGLE_LAP`, `combine_toggle_lap_target`, `RepeatSchedule` seeding,
  `run_toggle_held` / `spawn_held`, `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`), not a
  central gate.
- **The audit (ticket 01)** — 12 surfaces, 8 OK, 2 flagged → the missed-deadline clamp
  (ticket 06) on both repeat pace loops, re-basing from real elapsed time like
  `input_repeat_key`.
- **`value=2` (ticket 07)** — recorded as a **firm decision, marked gated / pending a
  fresh implementation effort** (Charon's call): inject `value=2` ourselves (evdev 0.13.2
  can't enable `EV_REP` on a `VirtualDevice`; `translate` already emits it); converts
  Digital + analog-synth Hold-to-repeat, single-key Chord, Analog-repeat hold-solid,
  Toggle / Hold-to-repeat of a single key or single-key Macro; **not** Stepper, button
  Toggles, or multi-step Macro loops. Rationale block is spec §8 in substance.
- **The Macro exception** — once-fired burst + within-run cadence unrestricted;
  trigger-driven Macro *repetition* floored to `max(kernel period, MIN_TOGGLE_LAP)` by
  **existing** mechanism (`run_toggle_loop` `target_lap` + `FiringUnfinished` overlap
  guard), no new floor, ticket 08 locks it; Analog-repeat cannot wrap a Macro (ticket
  09).
- **Not guarded** — a pathologically fast kernel `kbdrate` (ticket 01, Q4).

### 2. Glossary term — `CONTEXT.md`, Runtime section, after *Output suppression*

**Term renamed** (Charon's call): **"Physical-plausibility ceiling"**, not "Humane output
rate". "Humane output rate" survives only as this effort's working name — the map heading,
the `.scratch/humane-output-rate/` path, and every ticket's `Parent:` link keep it; it is
listed under the term's `_Avoid_` line as "the working name of the effort that established
this, not a domain term". Effort/map **not** renamed (identity churn across ~10 files not
worth it).

Full-detail entry (house style): the rate ceiling + held-button rule, the `value=2`
autorepeat presentation, the Macro exception in full, "not a disguise — `uinput` origin
always detectable (ADR-0008); plausibility of *rate*". Citation fixed to **ADR-0008**, not
the wrong `(ADR-0002)` in the ticket's draft.

### 3. Map + README touched

- Map: the two lines equating the effort name with the term (Destination para, Notes
  Domain bullet) reworded to name "Physical-plausibility ceiling" as the codified term and
  demote "Humane output rate" to the working name.
- `.scratch/README.md`: the `("Humane output rate")` parenthetical on the term → the new
  name.
- Decisions-so-far: context pointer appended.
