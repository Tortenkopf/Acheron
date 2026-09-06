<!-- wayfinder:map -->

# Humane output rate

## Destination

Every non-macro Action that **holds or repeats** synthetic output is verified against one
bar: Acheron never emits key/button events faster than the Linux input stack itself would
for a **physically held key or button** — the kernel's configured autorepeat delay/period
for keys, and exactly one Down/Up (no repeat) for a held mouse/gamepad button. Any surface
that fails the bar is **fixed within this map** (execution is in scope, not handed off).

The finding then feeds two gated specs: a **`spec.md`** for the user-facing output-safety
guidance (macro-editor disclaimer + best-practice tips covering anti-cheat plausibility and
not locking down / impeding one's own system, plus the Analog-repeat selection toast) to
land in the editor and the README; and a **`spec-kernel-shaped-repeat.md`** (ticket 01's
ratification, Q5) for making every Hold-to-repeat path emit genuine `value=2` kernel
autorepeat — with the real delay→period envelope — instead of `[KeyDown,KeyUp]` pairs, so a
heuristics report reads Acheron's virtual device as playing by the rules and hand-driven,
not automated. The principle is recorded as an **ADR + a `CONTEXT.md` glossary term**
("Humane output rate") so future repeat-based features inherit the invariant.

Done when: the audit verdict is ratified, every flagged surface is fixed, the ADR + term
are written, and both specs (`spec.md` and `spec-kernel-shaped-repeat.md`) are gated —
ready for fresh implementation efforts.

## Notes

- **Domain**: `CONTEXT.md` at repo root; `docs/adr/` (0001–0007). Use the glossary's
  vocabulary — Trigger mode, Hold-to-repeat, Analog-repeat, Toggle, Capture mode, Depth,
  Macro, Chord, Stepper. This effort proposes one new term ("Humane output rate", ticket 04).
- **Skills**: `/grilling` + `/domain-modeling` on every grilling ticket; `/research` for
  ticket 02.
- **Execution is in scope.** This map carries its own fixes (user decision, 2026-09-06):
  fix tickets graduate from ticket 01's audit and are resolved on this map, not a handoff
  (so far: ticket 06, the pace-loop clamp). **Two** things are handed off as specs for
  fresh implementation efforts: the user-facing guidance UI + README (ticket 05's
  `spec.md`) and the `value=2` kernel-shaped repeat (ticket 07's
  `spec-kernel-shaped-repeat.md` — carved out at ticket 01's ratification because it is a
  cross-cutting behaviour change, not a localised fix).
- **The bar** (settled while charting): the Linux **kernel autorepeat rate** as read live
  by `analog::read_kernel_auto_repeat` (`daemon/src/capture/analog.rs`), fallback 250 ms
  delay / 33 ms period. Steady-state repeat *rate* only — a physical key's ~250 ms
  initial-repeat delay is judged per surface, not required everywhere (Analog-repeat
  deliberately simulates tapping, not holding). Held buttons: one Down/Up, no repeat.
- **ADR-0002 does *not* cover uinput-origin detectability** (found while resolving ticket
  07): it is about "direct evdev/uinput instead of OpenRazer" only. No ADR in the repo
  records the "uinput origin is always detectable" premise. Ticket 04 writes it as a new
  ADR (0008); the glossary term and any rationale cite 0008, not 0002.
- **Prior art in-tree**: ticket 26 (`MIN_TOGGLE_LAP` 20 ms floor, added after a zero-delay
  toggled Keypress hard-froze a machine), ticket 68 (`resolve_toggle_lap_target` live
  kernel read), ticket 20 (Analog-repeat 2–20 Hz curve), ticket 18 §4 (`RepeatSchedule`
  seeded from the real kernel delay/period), tickets 75/76 & 79/80 (`spawn_held` sustained
  hold for buttons — no synthetic autorepeat), ticket 28 (`CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`
  35 ms). The audit re-checks these, it doesn't assume them correct.

## Decisions so far

<!-- one line per closed ticket: gist + link; zoom the ticket for detail -->

- [Anti-cheat input-detection heuristics for the macro tips](issues/02-anticheat-input-detection-research.md)
  — grounding written to [`research/anticheat-input-timing-heuristics.md`](research/anticheat-input-timing-heuristics.md).
  Confirms the bar: Linux held-key autorepeat is software (`input.c`, `input_enable_softrepeat(dev, 250, 33)`
  — 250 ms delay then ~33 ms/~30 Hz), `BTN_*` never autorepeats. **Regularity, not raw rate,
  is the universal tell** (osu! circleguard ~5 ms SD = bot; CS2 "0 ms overlap/neutral" = macro;
  naive fixed/PRNG timing trivially separable from human). Human inter-key ~50–200 ms; reaction
  time floor ~150 ms. No mainstream game anti-cheat publishes evdev/uinput timing thresholds.
- [Decide the self-DoS macro guardrail](issues/03-self-dos-macro-guardrail.md)
  — trigger-driven Macro *repetition* is **already floored** to `max(kernel period,
  MIN_TOGGLE_LAP)` on every path (`run_toggle_loop`'s `target_lap`; kernel-`Repeat` +
  the `FiringUnfinished` overlap guard) → **no new floor code**, just **ticket 08**
  (regression tests + comments). The within-a-single-run keystroke burst and a Macro
  fired once stay **unrestricted** (Macro = the deliberate exception; text-only guidance
  in ticket 05, no editor warning). A trigger-looped **single-key** Macro routes through
  ticket 07's `value=2` held-key path (predicate is 07's). **Analog-repeat + Macro is
  banned outright** → **ticket 09** (new `ConfigError`, rejected at `SetBinding` + load).
- [Audit every holding/repeating output path against the kernel-autorepeat bar](issues/01-audit-holding-repeating-output-paths.md)
  — 12-surface verdict table ratified with Charon. **8 surfaces OK** (floored Toggle loops,
  1:1 kernel-Repeat→fire gating, genuine single holds for mouse/gamepad buttons). **2 flagged**
  (surfaces 2 & 5: repeat pace loops lack missed-deadline clamping → catch-up burst under
  stall) → **ticket 06**, inline fix. **Cross-cutting**: Hold-to-repeat always emits
  `[KeyDown,KeyUp]` pairs, never `value=2` (uinput device has no `EV_REP`) — rate-compliant
  but the clearest synthetic tell → **ticket 07** speccs a `value=2` rebuild. Macro paths
  (surfaces 9 & 10) → ticket 03. Pathologically fast kernel `kbdrate` ruled out of scope.
- [Spec the kernel-shaped repeat behaviour (`value=2`)](issues/07-spec-kernel-shaped-repeat.md)
  — gated [`spec-kernel-shaped-repeat.md`](spec-kernel-shaped-repeat.md). Fork: **inject
  `value=2` ourselves** (evdev 0.13.2 can't enable `EV_REP` on a `VirtualDevice`;
  `translate` already emits `value=2` for passthrough). New `D::RepeatKey` + `hold_repeat_kind`
  extending `sustained_hold_key`. **Converts**: Digital + Analog-synth Hold-to-repeat, Chord
  (single-key), Analog-repeat hold-solid, Toggle → keyboard Keypress / single-key Macro —
  all emit `value=1` then `value=2` at the live kernel `REP_DELAY`→`REP_PERIOD` (no delay
  for hold-solid). **Untouched**: Stepper Hold-to-repeat, Toggle → button (7/8), Toggle /
  Hold-to-repeat → multi-step Macro (9/10 — `run_toggle_loop` + `target_lap` survive here
  only). Ticket 06's `advance_fired` clamp carries untouched (decides *when*, not *what*);
  Toggle + hold-solid emitters adopt it. Single-key predicate (`[KeyDown(k),KeyUp(k)]`,
  modifier-wrapped, one trailing `Delay`) detected in `Slots::perform`. → **ticket 08**
  rewired `Blocked by: 01, 07`; **ticket 04** ADR cites 0008 not 0002.
- [Clamp missed deadlines in both repeat pace loops](issues/06-clamp-repeat-pace-loop-deadlines.md)
  — surfaces 2 & 5 fixed inline on `dev`. Two pure, table-tested helpers:
  `RepeatSchedule::advance_fired` re-bases the Grid Hold-to-repeat `fired` count from real
  elapsed time after a stall (emit one Repeat now, next due a full period later — no
  catch-up burst); `analog_repeat::tap_pace_wait` yields a **full period** when a tap pulse
  overruns `period`, so the Analog-repeat loop can't free-run above the 20 Hz ceiling under
  injector back-pressure. Kernel-parity: `input_repeat_key` re-arms from *now*, never
  bursts. Ticket 07's `value=2` rebuild of surface 2 must preserve `advance_fired`'s
  re-base.

## Not yet specified

- **Implement the user-facing output-safety guidance** — the GtkExpander / hint widgets,
  the Analog-repeat toast, the README section. Blocked on ticket 05's `spec.md`. A fresh
  implementation effort, not resolved here.
- **Implement the kernel-shaped `value=2` repeat** — ticket 07's
  [`spec-kernel-shaped-repeat.md`](spec-kernel-shaped-repeat.md) is now **gated and ready**.
  A fresh implementation effort, not resolved here.

## Out of scope

- **A speed ceiling on a Macro fired once, and on the keystroke cadence *within* one run
  of a multi-step Macro** — Macros are the deliberate exception; an author may sequence
  Keypresses at any cadence. Ticket 03 **kept both out**: the once-fired burst is bounded
  by step count + injector backpressure (not ticket 26's unbounded loop), and a within-run
  floor would cap legitimate fast combos. Text-only guidance in ticket 05, no code.
- **A new floor for *trigger-driven Macro repetition*** (was pulled *in* by ticket 01, Q6)
  — ticket 03 found the Toggle→Macro loop-lap and the Hold-to-repeat→Macro re-fire cadence
  are **already** floored to `max(kernel period, MIN_TOGGLE_LAP)` by existing mechanism, so
  no new floor is built — only [ticket 08](issues/08-lock-macro-repetition-floor-tests.md)
  locks the invariant with tests. (A trigger-looped single-key Macro still rides ticket
  07's `value=2` path — that part stayed in and is now ticket 07's to spec.)
- **Guarding against a pathologically fast kernel autorepeat config** (`kbdrate` at 100/s+)
  — following the live kernel rate *is* the bar as written, and a misconfigured OS would
  flood identically from the user's physical keys. Ticket 01, Q4: not Acheron's job.
- **Jitter / anti-regularity injection into synthetic output** — the bar is the kernel
  rate, not statistical indistinguishability. uinput origin is always detectable (accepted
  from the start); the goal is physical *plausibility of rate*, not disguise.
- **Passthrough, axis writes, and staging-mode handoff latency** — not holding/repeating
  surfaces. Staging adds latency (safe direction). Ticket 01 confirms this in passing; no
  work item.
