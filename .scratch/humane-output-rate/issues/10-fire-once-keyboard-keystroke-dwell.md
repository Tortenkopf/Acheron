# Fire-once keyboard keystroke dwell

Type: grilling
Status: resolved
Blocked by: —
Parent: [Humane output rate](../map.md)

## Question

The [audit](01-audit-holding-repeating-output-paths.md) and everything built from it (the
`value=2` rebuild, ADR-0008, the "Physical-plausibility ceiling" `CONTEXT.md` term) scoped
the bar to Actions that **hold or repeat**. A single **Fire-once** press of a keyboard key
was never in that scope — but it emits the same near-zero-dwell `[KeyDown, KeyUp]` shape
the audit's own cross-cutting finding named "the single most concrete synthetic tell." It
thematically belongs on this map (the effort was reopened to add it). Decide whether it
needs fixing, and if so, how.

### The finding (2026-09-07 code read)

`(FireOnce, Down)` → `TriggerDecision::SpawnFireOnce` (`daemon/src/trigger.rs:266`) →
`executor::spawn_fire_once(injector, compile(action))`. For `Action::Keypress`, `compile`
→ `keypress_steps` produces `[KeyDown(mods…), KeyDown(key), KeyUp(key), KeyUp(mods…)]` with
**no `Delay` step anywhere in it** (`daemon/src/executor.rs:99`, `:223`). `run_once` walks
the steps back-to-back, sleeping only on an explicit `MacroStep::Delay`
(`daemon/src/executor.rs:295`). So the Down and Up leave as two consecutive `set_key_state`
messages — each its own `SYN_REPORT` frame, kernel-stamped at handling time (so the
*timestamps* themselves aren't a quantised tell), but separated only by two mpsc
round-trips: sub-millisecond, effectively zero dwell. Same for the modifier press/release.

The one place a dwell **is** inserted is `Action::ControllerButton`
(`CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD` = 35 ms, `daemon/src/executor.rs:76`), and Fire-once
is locked out for that Action entirely (ticket 78) — so it never benefits a Fire-once press.

### Why it was out of the original scope

The bar was deliberately "steady-state repeat *rate* only" (map Notes).
[Ticket 03](03-self-dos-macro-guardrail.md) kept "a Macro fired once" **out of scope** — but
that is about *Macros*, whose author sequences their own Keypresses at any cadence. A plain
`Action::Keypress` under Fire-once is a *canned* Down/Up the user has no timing control
over — a different case, which is why it is being pulled back in rather than left under the
Macro exclusion.

### Grounding (see [`docs/anti-cheat-input-heuristics.md`](../../../docs/anti-cheat-input-heuristics.md))

- Keystroke-dynamics work discards down→up intervals outside ~30–500 ms as non-typing;
  naive fixed/PRNG dwell is "trivially separable" from human (QUACK) — but that is over
  *streams* (ROC-AUC > 0.9 within ~70 keystrokes), not a single isolated press.
- CS2's published rule: "exactly 0 ms overlap and 0 ms neutral" between opposite inputs =
  macro. A 0 ms dwell sits squarely in that shape.
- "One physical press = one game action" is already satisfied (Fire-once fires once per
  `Down`); the open question is only the **dwell within** that one press.

### To settle

1. **Does a single Fire-once keyboard press need a dwell floor at all?** One isolated event
   is a far weaker signal than a repeat stream; weigh that against the ceiling's stated
   goal of *rate plausibility* and the fact that the fix is cheap.
2. **If yes, what value?** The user's stated latitude: a **fixed** dwell, identical for
   every keystroke, is acceptable — "it just has to be long enough." Candidates:
   reuse/parallel `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD` (35 ms), or a value in the
   ~30–500 ms keystroke-dynamics human band. Not tuned against a real game.
3. **Where does it live?** It **cannot** go into `keypress_steps` — `single_held_key`
   (`daemon/src/executor.rs:126`, the Hold-to-repeat / Toggle shape detector) rejects any
   `Delay` *between* the key edges, so a `Delay` there would break `value=2` classification
   for held single keys. Likely a Fire-once-only splice (`spawn_fire_once`, or a dedicated
   `fire_once_keypress_steps`).
4. **Scope of the fix.** In: `Action::Keypress` under Fire-once (plain and
   modifier-wrapped). Also weigh: a Fire-once single-step **Macro** (author-controlled →
   probably stays out, like every Macro), a Fire-once **Stepper** `Key` item (canned
   `keypress_steps` → probably in), a Fire-once **Chord** (canned → probably in).
   `Action::ControllerButton` is already locked out of Fire-once and already carries its
   own 35 ms dwell.
5. **Downstream.** ADR-0008 and the `CONTEXT.md` "Physical-plausibility ceiling" term are
   both currently worded "holding or repeating" — a dwell floor on a one-shot press
   broadens that and needs a scope sentence in each. Decide whether the fix is
   **execution-in-scope on this map** (like tickets 06 / 09) or a spec handoff.

## Answer

Resolved by grilling with Charon (2026-09-07). **A canned one-shot keyboard press gets a
fixed dwell floor.** Framed as **compliance, not defense** — Acheron does not hide that its
output is synthetic (the `uinput` origin stays fully visible, ADR-0008); it just should not
emit a keystroke shape no hand produces. A follow-up to the `value=2` rebuild, not a new
detection concern.

### Decisions

1. **Need a floor at all? — Yes.** The zero-dwell `[KeyDown, KeyUp]` pair is the exact
   shape ADR-0008 already names "the clearest synthetic tell," and the `value=2` rebuild
   erased it everywhere the audit flagged it *except* here — a plain Fire-once Keypress,
   the single most common Action. One isolated press is a weak signal, so this is a
   **low-priority consistency fix**, not an exploit closure — but it is cheap and the gap
   is conspicuous.

2. **Value — a new `40 ms` constant.** `FIRE_ONCE_KEY_DWELL` (name TBD by ticket 12), in
   `daemon/src/executor.rs` near `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`, with its **own**
   doc comment stating it is **deliberately not shared** with that constant (different job —
   35 ms targets single-poll-frame coverage). 40 ms: ~2.5 frames at 60 Hz, clears the
   CS2 "0 ms" shape, inside the ~30–500 ms keystroke-dynamics typing band, and ~2×
   headroom under the ~80 ms human same-key double-tap floor so `decide`'s `FiringUnfinished`
   overlap guard never drops a real user's second press. 35 / 50 were on the table; not
   above 50.

3. **Scope — where `executor::single_held_key` matches, under Fire-once.** Reuse the
   existing bright-line predicate from the `value=2` work — no new content inspection.
   This covers, for free:
   - Fire-once `Action::Keypress`, plain **and** modifier-wrapped;
   - Fire-once **single-key** `Macro` (`[KeyDown, KeyUp]`, no `Delay`) — already classified
     as a keypress by `single_held_key` for `value=2`; same logic here;
   - Fire-once Stepper `Key` step (compiles via `keypress_steps`);
   - Fire-once Chord whose Action is a single key.

   And excludes, for free:
   - Fire-once **multi-step** `Macro` — predicate returns `None`; the Macro exception holds
     **by shape, not by inspecting authorial intent**;
   - **Analog-repeat**, including the Digital-Capture-mode fallback — not Fire-once; it is
     an already-audited *tap* stream, and a fixed 40 ms dwell would break its 20 Hz (50 ms
     period) fast end;
   - `Action::ControllerButton` — locked out of Fire-once (ticket 78), already carries its
     own 35 ms dwell.

4. **Execution-in-scope on this map**, like tickets 06 and 09 — not a spec handoff.
   Graduated as **[ticket 12](12-splice-the-fire-once-keypress-dwell.md)** (the splice +
   tests + the ADR-0008 / `CONTEXT.md` wording).

5. **Mechanism — constraints binding, exact variant left to ticket 12.** Binding:
   (a) must not touch the Analog-repeat path (incl. the Digital-Capture fallback);
   (b) reuse `single_held_key`, no new content inspection; (c) Fire-once only.
   *Preference* (not mandated): mirror the `value=2` pure-layer split — `decide` already
   has `(FireOnce, Down)` and `(AnalogRepeat, …)` as separate arms that merely happen to
   share `guarded(D::SpawnFireOnce)`; splitting the Fire-once arm to carry the classified
   `(Modifiers, KeyCode)` keeps `perform` a mechanical executor, consistent with
   `D::HoldKeyDown` / `D::RepeatKey`.

6. **Downstream docs — amend ADR-0008, no new ADR** (a small consistent extension, not a
   new hard-to-reverse trade-off). Ticket 12 lands, alongside the code:
   - **ADR-0008, "The ceiling" paragraph** — add: *"A canned one-shot keyboard press — a
     Fire-once Keypress, and its single-key Macro / Stepper-step / Chord equivalents —
     carries a fixed 40 ms dwell between Down and Up (`FIRE_ONCE_KEY_DWELL`), rather than
     the near-zero-dwell pair the research names the clearest synthetic tell. This is a
     rate-plausibility consistency follow-up, not a detection defense: a Macro fired once
     still keeps its author's cadence, and Analog-repeat's tap pulses are unchanged."*
   - **`CONTEXT.md` "Physical-plausibility ceiling"** — after the `value=1`/`value=2`
     sentence: *"A canned one-shot keyboard press carries a fixed ~40 ms Down→Up dwell
     rather than a zero-dwell pair."*
   - **`CONTEXT.md` "Fire-once"** — left as-is (clean one-liner; the ceiling term carries
     the dwell).

The map stays open until ticket 12 (this fix) and ticket 11 (Additive fix-or-cut) are
both done.
