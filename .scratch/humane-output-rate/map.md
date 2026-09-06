<!-- wayfinder:map -->

Status: archived — destination reached 2026-09-07; all nine tickets resolved. Both gated
specs handed off and implemented: `spec-kernel-shaped-repeat.md` →
[`kernel-shaped-repeat-impl/`](../kernel-shaped-repeat-impl/issues/) (2026-09-06),
`spec-user-facing-output-safety-guidance.md` →
[`output-safety-guidance/`](../output-safety-guidance/issues/) (2026-09-07). ADR-0008 +
the "Physical-plausibility ceiling" `CONTEXT.md` term are written and un-gated.

# Humane output rate

## Destination

Every non-macro Action that **holds or repeats** synthetic output is verified against one
bar: Acheron never emits key/button events faster than the Linux input stack itself would
for a **physically held key or button** — the kernel's configured autorepeat delay/period
for keys, and exactly one Down/Up (no repeat) for a held mouse/gamepad button. Any surface
that fails the bar is **fixed within this map** (execution is in scope, not handed off).

The finding then feeds two gated specs: a **`spec-user-facing-output-safety-guidance.md`**
for the user-facing output-safety
guidance (macro-editor disclaimer + best-practice tips covering anti-cheat plausibility and
not locking down / impeding one's own system, plus the Analog-repeat selection toast) to
land in the editor and the README; and a **`spec-kernel-shaped-repeat.md`** (ticket 01's
ratification, Q5) for making every Hold-to-repeat path emit genuine `value=2` kernel
autorepeat — with the real delay→period envelope — instead of `[KeyDown,KeyUp]` pairs, so a
heuristics report reads Acheron's virtual device as playing by the rules and hand-driven,
not automated. The principle is recorded as an **ADR + a `CONTEXT.md` glossary term**
("Physical-plausibility ceiling" — the term chosen at ticket 04; "Humane output rate"
stays only as this effort's working name) so future repeat-based features inherit the
invariant.

Done when: the audit verdict is ratified, every flagged surface is fixed, the ADR + term
are written, and both specs (`spec-user-facing-output-safety-guidance.md` and
`spec-kernel-shaped-repeat.md`) are gated —
ready for fresh implementation efforts.

## Notes

- **Domain**: `CONTEXT.md` at repo root; `docs/adr/` (0001–0007). Use the glossary's
  vocabulary — Trigger mode, Hold-to-repeat, Analog-repeat, Toggle, Capture mode, Depth,
  Macro, Chord, Stepper. This effort adds one new term — **"Physical-plausibility ceiling"**
  (ticket 04, now in `CONTEXT.md` + ADR-0008).
- **Skills**: `/grilling` + `/domain-modeling` on every grilling ticket; `/research` for
  ticket 02.
- **Execution is in scope.** This map carries its own fixes (user decision, 2026-09-06):
  fix tickets graduate from ticket 01's audit and are resolved on this map, not a handoff
  (so far: ticket 06, the pace-loop clamp). **Two** things are handed off as specs for
  fresh implementation efforts: the user-facing guidance UI + README (ticket 05's
  `spec-user-facing-output-safety-guidance.md`) and the `value=2` kernel-shaped repeat (ticket 07's
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
  — grounding written to `research/anticheat-input-timing-heuristics.md`, **relocated at
  ticket 05 to [`docs/anti-cheat-input-heuristics.md`](../../docs/anti-cheat-input-heuristics.md)**
  (non-process path → reaches `main`, cited by ADR-0008 + the README).
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
  **Implemented and shipped 2026-09-06** in
  [`.scratch/kernel-shaped-repeat-impl/`](../kernel-shaped-repeat-impl/issues/) (7
  tickets, all done). Its ticket 07 un-gated ADR-0008, restructured the `CONTEXT.md`
  "Toggle" entry for the three now-distinct hold behaviours (the "Physical-plausibility
  ceiling" term already read present-tense and was left as-is), and resolved
  **ticket 08** below (the Toggle→Macro regression split landed with that effort's
  ticket 05). Effort decision along the way: a single-key **deep-stage** Hold-to-repeat
  converts too.
- [Clamp missed deadlines in both repeat pace loops](issues/06-clamp-repeat-pace-loop-deadlines.md)
  — surfaces 2 & 5 fixed inline on `dev`. Two pure, table-tested helpers:
  `RepeatSchedule::advance_fired` re-bases the Grid Hold-to-repeat `fired` count from real
  elapsed time after a stall (emit one Repeat now, next due a full period later — no
  catch-up burst); `analog_repeat::tap_pace_wait` yields a **full period** when a tap pulse
  overruns `period`, so the Analog-repeat loop can't free-run above the 20 Hz ceiling under
  injector back-pressure. Kernel-parity: `input_repeat_key` re-arms from *now*, never
  bursts. Ticket 07's `value=2` rebuild of surface 2 must preserve `advance_fired`'s
  re-base.
- [Spec the user-facing output-safety guidance](issues/05-spec-macro-editor-safety-guidance.md)
  — gated [`spec-user-facing-output-safety-guidance.md`](spec-user-facing-output-safety-guidance.md).
  **Text + placement only** — no config check, no blocking
  widget. Two **GUI hints** (not a toast — new CONTEXT.md `### Interface` terms **Toast
  label** vs **GUI hint**): the standing macro-editor disclaimer (`⚠️` line, Macro tab
  only) and the Analog-repeat notice (`⚠️` line below the Trigger-mode dropdown while
  `analog_repeat` is selected — supersedes ticket 01 Q3's "one-time toast"). One
  collapsed **`Gtk.Expander`** "About macro safety" holds the compact tips; the long form
  is a new **`## Output safety`** README section (ceiling for users, two tip themes,
  runaway recovery, friendly no-warranty restatement, no "safe" numbers — risk depends on
  the game). Anti-cheat research **relocated this session** to
  `docs/anti-cheat-input-heuristics.md`; CONTEXT.md terms **written this session**.
  Implementation (widgets + README edit) is a fresh effort — now broken into four tickets
  at [`output-safety-guidance/`](../output-safety-guidance/issues/) (2026-09-07).
- [Record the humane-output-rate principle — ADR + CONTEXT.md term](issues/04-record-humane-output-rate-principle.md)
  — written. **ADR-0008** (`docs/adr/0008-physical-plausibility-ceiling-for-synthetic-output.md`):
  the ceiling (live kernel autorepeat rate; held `BTN_*` one Down/Up), the ticket-01 audit
  + ticket-06 clamp, the `value=2` decision (firm, marked gated pending a fresh effort), the
  Macro exception (once-fired + within-run free; trigger-repetition floored by existing
  mechanism; no Analog-repeat+Macro), and the "not guarded: pathological `kbdrate`" note.
  ADR-0008 is the first record of "uinput origin is always detectable, and accepted" —
  **not** ADR-0002. **Glossary term named "Physical-plausibility ceiling"** (Charon's call,
  not "Humane output rate") in `CONTEXT.md` Runtime section; "Humane output rate" stays only
  as this effort's working name (map heading, path, `Parent:` links) and sits on the term's
  `_Avoid_` line. Effort/map not renamed.

- [Disallow the Analog-repeat + Macro Binding](issues/09-disallow-analog-repeat-macro-binding.md)
  — execution ticket, done on `dev`. New `ConfigError::AnalogRepeatMacro(input)` in
  `config::binding::check_binding` (site-shape step, `Individual(Grid)` arm) → enforced at
  both `SetBinding` and `load_or_seed` from the one pure rule; reachable only where
  analog-repeat is otherwise legal (non-grid still `InvalidAnalogRepeatInput`, Chord still
  `InvalidChordAnalogRepeat`). `schema.rs` fixture re-blessed (20 `{macro, grid_*,
  analog_repeat}` rows → `false`); `rules.valid_triggers` + `binding_editor` drop the
  option for a Macro Action; CONTEXT.md **Trigger mode** entry gains a third "except…"
  clause (no ADR). **This was the last open ticket — the map's destination is reached and
  it can be archived.**

## Not yet specified

- ~~**Implement the user-facing output-safety guidance**~~ — **done.** Ticket 05's
  [`spec-user-facing-output-safety-guidance.md`](spec-user-facing-output-safety-guidance.md)
  was implemented in [`.scratch/output-safety-guidance/`](../output-safety-guidance/issues/)
  (2026-09-07, all 4 tickets done): the two GUI hints (`⚠️` macro-editor disclaimer +
  Analog-repeat notice), the "About macro safety" `Gtk.Expander`, the README
  `## Output safety` section, and the feature-bullet pointers.
- ~~**Implement the kernel-shaped `value=2` repeat**~~ — **done.** Ticket 07's
  [`spec-kernel-shaped-repeat.md`](spec-kernel-shaped-repeat.md) was implemented in
  [`.scratch/kernel-shaped-repeat-impl/`](../kernel-shaped-repeat-impl/issues/)
  (2026-09-06, all 7 tickets done). ADR-0008 un-gated, `CONTEXT.md` "Toggle" entry
  restructured, [ticket 08](issues/08-lock-macro-repetition-floor-tests.md) resolved.

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
