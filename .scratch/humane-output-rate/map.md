<!-- wayfinder:map -->

Status: **ARCHIVED 2026-09-07 (second time) — destination reached.** All thirteen tickets
resolved. Tickets 01–09 landed the audit, the two gated specs (both since implemented —
`kernel-shaped-repeat-impl/` and `output-safety-guidance/`), ADR-0008 and the
"Physical-plausibility ceiling" `CONTEXT.md` term. The 2026-09-07 reopening for tickets 10
and 11 is closed out: ticket 12 spliced the 40 ms Fire-once keyboard dwell (`FIRE_ONCE_KEY_DWELL`),
and [ticket 13](issues/13-remove-additive-staging-mode.md) removed the Dual-stage
**Additive** staging mode (new **ADR-0009**; hard `ConfigError::RemovedStagingModeAdditive`
at load for an existing `"additive"` config). After the cut **no staging mode ever emits
two concurrent autorepeat streams** — the invariant ADR-0008 now records. Nothing left to
decide; effort complete.

[Fire-once keyboard keystroke dwell](issues/10-fire-once-keyboard-keystroke-dwell.md)
(**resolved**): a single Fire-once keyboard press emits the same near-zero-dwell
`[KeyDown, KeyUp]` the audit flagged as its clearest synthetic tell, and it was never in the
original "holds or repeats" scope → **yes, add a fixed 40 ms dwell** to a canned one-shot
press (where `single_held_key` matches, under Fire-once), as a compliance/consistency
follow-up to the `value=2` rebuild; execution
[ticket 12](issues/12-splice-the-fire-once-keypress-dwell.md) **landed 2026-09-07** — see
Decisions so far.
[Additive Staging mode — fix or cut](issues/11-additive-staging-mode-fix-or-cut.md)
(**resolved 2026-09-07**): the `value=2` rebuild (ticket 07) left the Dual-stage **Additive**
mode emitting two concurrent `value=2` autorepeat streams from one press — a shape no
physical keyboard produces (the kernel autorepeats only the most-recently-pressed key) → **cut
the mode**; execution graduated as [ticket 13](issues/13-remove-additive-staging-mode.md)
(**resolved 2026-09-07**). After the cut no staging mode ever emits two concurrent
autorepeat streams.

Prior archive note (2026-09-07, superseded): all nine tickets resolved. Both gated specs
handed off and implemented: `spec-kernel-shaped-repeat.md` →
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

**Also in scope (added 2026-09-07):** a **Fire-once** press of a keyboard key — not a hold
or a repeat, but a single canned `[KeyDown, KeyUp]` the user has no timing control over,
which today goes out with ~0 ms dwell (no `Delay` in `keypress_steps`). The bar's spirit —
timing a physical hand could reproduce — extends to that one-shot press: whether it needs a
fixed dwell floor, and where, is [ticket
10](issues/10-fire-once-keyboard-keystroke-dwell.md). (A *Macro* fired once stays out — its
author sequences their own timing; see Out of scope.)

**Also in scope (added 2026-09-07):** the Dual-stage **Additive** Staging mode, which this
effort's `value=2` rebuild left emitting two concurrent kernel-autorepeat streams from a
single finger press. [Ticket 11](issues/11-additive-staging-mode-fix-or-cut.md)
(**resolved**) chose to **cut the mode** — two held autorepeating keys is a shape no
physical keyboard produces. Execution is
[ticket 13](issues/13-remove-additive-staging-mode.md), on this map.

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
are written, both specs (`spec-user-facing-output-safety-guidance.md` and
`spec-kernel-shaped-repeat.md`) are gated — ready for fresh implementation efforts — **and**
the Fire-once keyboard-dwell question (ticket 10 → ticket 12, landed 2026-09-07) and
the Additive fix-or-cut question (ticket 11 → ticket 13, cut, landed 2026-09-07) are
decided, with the fixes each calls for made on this map. **All satisfied — effort
complete, map archived.**

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
- **Reopened for ticket 11 (2026-09-07).** The `value=2` kernel-shaped-repeat work
  (ticket 07, shipped in `kernel-shaped-repeat-impl/`) converted single-key deep-stage
  Hold-to-repeat to genuine kernel autorepeat — so a Dual-stage **Additive** key with both
  stages Hold-to-repeat emitted *two* concurrent `value=2` streams from one press,
  phase-locked to the one synthesized `RepeatSchedule` cadence. Owned here (not the archived
  Dual-stage map) because this effort's work is what broke it and what raises the ceiling
  question. **Resolved 2026-09-07: cut the mode** — two held autorepeating keys is a shape
  no physical keyboard produces (the kernel autorepeats only the most-recently-pressed key),
  so it is an ADR-0008 violation, not a borderline regularity call; no use case, always the
  shakiest of the four modes. Execution on this map as
  [ticket 13](issues/13-remove-additive-staging-mode.md) — `StagingMode` → Handoff /
  No-Return / Quick-Skip, hard `ConfigError` at load for an existing `"additive"` config
  (no known users), new **ADR-0009**. After the cut, no staging mode emits two concurrent
  autorepeat streams (Handoff/No-Return hand the primary off, Quick-Skip suppresses it).
  See Decisions so far.
- **Reopened for ticket 10 (2026-09-07).** The bar as charted was "holds or repeats,
  steady-state rate only." Ticket 10 extended the *spirit* of it — timing a physical hand
  could reproduce — to a single **Fire-once keyboard press**, whose canned `[KeyDown,
  KeyUp]` has ~0 ms dwell. **Resolved 2026-09-07:** yes, a fixed 40 ms dwell, execution
  on this map as [ticket 12](issues/12-splice-the-fire-once-keypress-dwell.md), which
  amends ADR-0008 and adds a clause to the `CONTEXT.md` "Physical-plausibility ceiling"
  term (both currently worded "holding or repeating"). See Decisions so far.
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
  it can be archived.** *(Superseded: the map was reopened 2026-09-07 for tickets 10 & 11.)*

- [Remove the Additive staging mode](issues/13-remove-additive-staging-mode.md)
  — execution ticket, done on `dev` 2026-09-07. `StagingMode` is now **Handoff / No-Return
  / Quick-Skip**. Migration mechanism = a raw-`toml::Value` scan
  (`config::find_removed_additive_staging`, run in `parse()` ahead of the typed deserialize,
  mirroring the legacy-inline-Macro guard) → `ConfigError::RemovedStagingModeAdditive`
  (names Additive, cites ADR-0009, points at Handoff/No-Return/Quick-Skip or a deep-stage
  Macro); `Additive` fully removed from `enum StagingMode` — no retained marker variant
  leaking into `match`es. At the D-Bus `SetStagingMode` boundary `"additive"` just gets the
  ordinary unknown-mode error. `stage.rs` `additive()` + table + `advance` arm deleted, the
  Additive-naming self-references (`QuickSkipPhase::Skipped` doc, `primary_handed_off` doc,
  `Engine` doc, the `advance` doc) reworded to describe the ops directly,
  `quick_skip_skipped_runs_additive_…` test renamed (assertions kept); `deep_repeat` /
  `primary_handed_off` **stay** (Handoff/No-Return need them). Test fixtures re-pointed
  (`edit.rs`, `dbus/mod.rs`, `dbus/wire.rs`, GUI `test_binding_editor` / `test_daemon_stub`
  / `prototype`); `dual_stage_additive_holds_both_stages…` deleted (surviving-mode
  deep-repeat coverage confirmed intact). New **ADR-0009**
  (`0009-additive-staging-mode-removed.md`); ADR-0007 parenthetical trimmed + pointer;
  ADR-0008 gains a note (Additive = first surface the ceiling *deleted*); `CONTEXT.md`
  `Additive` entry removed + `Staging mode` list trimmed; README bullet removed.
  Regression test: a `mode = "additive"` `config.toml` fails `load_or_seed` and is left
  untouched on disk. Suite: daemon **525** pass, clippy `-D warnings` clean, `cargo fmt`
  clean (also swept ticket 09's pre-existing `binding.rs` violation); GUI **504** pass.
  `grep -i additive` clean (only ADR-0009 / the pointers / the `RemovedStagingModeAdditive`
  machinery / unrelated prose). **Last open ticket — destination reached, map archived.**

- [Additive Staging mode — fix or cut](issues/11-additive-staging-mode-fix-or-cut.md)
  — grilling with Charon. **Cut the Dual-stage Additive mode.** `StagingMode` becomes
  **Handoff / No-Return / Quick-Skip**. Decisive point: Additive with both stages
  Hold-to-repeat emits **two concurrent `value=2` autorepeat streams from one press**
  (`stage::Engine::deep_repeat` rides the primary's `RepeatSchedule`, so also phase-locked)
  — and a real keyboard autorepeats only the *most-recently-pressed* key, so holding two is
  physically impossible, not a borderline regularity tell → an ADR-0008 violation. Also: no
  use case Charon can name, always the shakiest of the four (three `dual-stage-keys-impl` #03
  post-ship corrections clustered here). **After the cut no staging mode ever emits two
  concurrent autorepeat streams** (Handoff/No-Return hand the primary off via
  `primary_handed_off`; Quick-Skip `Skipped` suppresses it). Q2 (ceiling per-stream vs
  aggregate) is **moot** — the output is impossible either way; a user wanting a fast
  alternating-keystroke macro that *looks* like autorepeat can build one (the Macro
  exception working as intended). **Migration**: hard `ConfigError` at load for an existing
  `staging_mode = "additive"` — no silent downgrade (no known users); mechanism is
  ticket 13's, constraint = an Additive-specific message. **Record**: new **ADR-0009**
  ("Additive staging mode removed — a real keyboard can't hold two autorepeating keys"),
  citing ADR-0008/0007; one-line pointers into both; `CONTEXT.md` `Additive` entry removed +
  `Staging mode` list trimmed, alongside the code. **Execution-in-scope** →
  [ticket 13](issues/13-remove-additive-staging-mode.md) (`Blocked by: 11`). No README
  "use a Macro" note (old behaviour was impossible; a note would describe a different
  thing).

- [Fire-once keyboard keystroke dwell](issues/10-fire-once-keyboard-keystroke-dwell.md)
  — grilling with Charon. **Yes, floor a canned one-shot keyboard press with a fixed
  `40 ms` dwell** between Down and Up — the zero-dwell `[KeyDown, KeyUp]` pair is the shape
  ADR-0008 already calls "the clearest synthetic tell," erased everywhere else by the
  `value=2` rebuild but still shipping on a plain Fire-once Keypress. Framed as
  **compliance, not defense** (the `uinput` origin stays visible; the output just should
  not be a shape no hand makes). **Scope = wherever `executor::single_held_key` matches,
  under Fire-once** — plain + modifier Keypress, single-key Macro, single-key Stepper step,
  single-key Chord; multi-step Macro, Analog-repeat (incl. Digital-Capture fallback), and
  `ControllerButton` all excluded *by that predicate*, no new content inspection. New
  `FIRE_ONCE_KEY_DWELL` constant in `executor.rs`, **not** shared with
  `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`. **Execution-in-scope** →
  [ticket 12](issues/12-splice-the-fire-once-keypress-dwell.md) (the splice + tests +
  **amend ADR-0008** — no new ADR — + the `CONTEXT.md` ceiling-term clause; `CONTEXT.md`
  "Fire-once" untouched). Mechanism (decide-split vs perform-branch) left to ticket 12
  under three binding constraints (no Analog-repeat impact; reuse `single_held_key`;
  Fire-once only).

- [Splice the Fire-once keypress dwell](issues/12-splice-the-fire-once-keypress-dwell.md)
  — execution ticket, done on `dev` 2026-09-07. New `executor::FIRE_ONCE_KEY_DWELL`
  (`40 ms`, own doc comment, not shared with `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD`) +
  `executor::fire_once_key_steps` (= `keypress_steps` with a `Delay` between the base key's
  edges). **Mechanism = `perform`-local branch**, not the decide-split ticket 10 preferred:
  the split can't reach a Fire-once **Stepper `Key` step** (`decide` stays abstract for
  `SpawnFireOnce`; `hold_repeat_kind` is `None` for `Action::Step`), so `Slots::perform`'s
  `D::SpawnFireOnce` arm runs `single_held_key` on the *compiled* steps and swaps in the
  dwelled sequence — covering plain/modifier Keypress, single-key Macro, Stepper `Key`
  step, single-key Chord uniformly. `decide` untouched → the Analog-repeat arm
  (incl. Digital-Capture fallback) is provably unaffected (locked by a decide-table test).
  **New wrinkle:** `Slots::perform` is shared with `stage::Engine`, so a new
  `fire_once_key_dwell: bool` on `PerformDeps` turns the dwell **off** for a dual-stage
  `RepressPrimary` / deep fire (depth-driven, machine-sequenced — not a user one-shot; a
  40 ms hold there would let the overlap guard swallow a fast deep-band wiggle's re-press).
  **Accepted residual:** a sub-40 ms physical tap emits a redundant trailing `value=0`
  (kernel-deduped; same shape as `CONTROLLER_BUTTON_DIGITAL_PULSE_HOLD` on a sub-35 ms
  Analog-repeat `Up`). ADR-0008 "The ceiling" + `CONTEXT.md` ceiling term amended (no new
  ADR); "Fire-once" `CONTEXT.md` entry untouched. Suite: daemon 525 pass, clippy clean,
  GUI 503 pass; no GUI / `rules.py` / `ConfigError` surface. *(Aside: `dev` carries a
  pre-existing `cargo fmt` violation in `daemon/src/config/binding.rs` from ticket 09's
  `afed7ce` — worth a sweep before `main` is rebuilt.)*

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

- ~~**The Additive fix or removal itself**~~ — **done.** Ticket 11 chose **cut**;
  [ticket 13](issues/13-remove-additive-staging-mode.md) executed it on `dev`
  2026-09-07 — `StagingMode::Additive` removed, `ConfigError::RemovedStagingModeAdditive`
  load rejection, ADR-0009, and the `CONTEXT.md` / ADR-0007 / ADR-0008 / README edits.

**Nothing outstanding — the map is archived.**

## Out of scope

- **A speed ceiling on a Macro fired once, and on the keystroke cadence *within* one run
  of a multi-step Macro** — Macros are the deliberate exception; an author may sequence
  Keypresses at any cadence. Ticket 03 **kept both out**: the once-fired burst is bounded
  by step count + injector backpressure (not ticket 26's unbounded loop), and a within-run
  floor would cap legitimate fast combos. Text-only guidance in ticket 05, no code.
  *(Distinct from a plain `Action::Keypress` fired once — a canned Down/Up with no
  author-controlled timing — which ticket 10 pulls back **in** scope. The distinction is
  authorship of the timing, not the Trigger mode.)*
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
