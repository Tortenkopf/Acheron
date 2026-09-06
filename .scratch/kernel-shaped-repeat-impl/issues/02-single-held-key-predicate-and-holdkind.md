# 02 — `single_held_key` predicate + `HoldKind` classifier

**What to build:** The pure, standalone-tested vocabulary the decision layer needs to
tell a "held single key" apart from everything else — a predicate over compiled macro
steps, an enum naming the two hold shapes, and a classifier that maps a `Binding`'s
`Action` onto that enum. New code only: `trigger::decide` still calls the existing
`sustained_hold_key` after this ticket and behaviour is unchanged (ticket 03 does the
wiring). The classifier is `#[allow(dead_code)]` until ticket 03 removes it.

Source of truth: [`spec-kernel-shaped-repeat.md`](../../humane-output-rate/spec-kernel-shaped-repeat.md)
§2.1 (the `hold_repeat_kind` / `HoldKind` / `single_held_key` rows), §6 (the predicate
spec in full), §6.1 (where each piece lives).

**Blocked by:** None — can start immediately (parallel with 01).

**Status:** ready-for-agent

- [ ] `pub(crate) fn single_held_key(steps: &[MacroStep]) -> Option<(Modifiers, KeyCode)>`
      in `executor`, beside `keypress_steps`. Pure over `&[MacroStep]`. **Qualifies**
      (`Some`):
  - `[KeyDown(k), KeyUp(k)]` — a plain unmodified key;
  - `[KeyDown(m0)…KeyDown(mN), KeyDown(k), KeyUp(k), KeyUp(mN)…KeyUp(m0)]` — a
    modifier-wrapped single key, `m*` being modifier codes in `keypress_steps`' fixed
    ctrl/shift/alt/super order (released in reverse);
  - either of the above followed by **at most one trailing `MacroStep::Delay`**
    (ignored — ticket 03's "turbo macro written with a trailing pause").
      **Disqualifies** (`None`): more than one distinct non-modifier key; any
      `MacroStep::Delay` *between* the key steps; any other shape.
- [ ] `enum HoldKind { SustainedNoRepeat(KeyCode), AutorepeatKey(Modifiers, KeyCode) }`
      in `trigger` (§2.1). `SustainedNoRepeat` = today's `sustained_hold_key` set
      (`Action::ControllerButton`, or `Action::Keypress` on a mouse-button code).
      `AutorepeatKey` = a single keyboard key: an `Action::Keypress` (with or without
      modifiers) on a non-mouse code, **or** an `Action::Macro` whose compiled steps
      satisfy `single_held_key`.
- [ ] `fn hold_repeat_kind(action: &Action, macros: &HashMap<MacroId, MacroDef>) -> Option<HoldKind>`
      in `trigger`, beside `sustained_hold_key`. For `Action::Keypress` /
      `Action::ControllerButton` it reads `mods` + `key` straight off the action (no
      macro-map access). For `Action::Macro` it resolves the `MacroDef`, compiles the
      steps (reuse `executor::compile`), and runs `single_held_key`. `Action::Step` /
      `Action::ProfileSwitch` → `None` (multi-step / handled earlier). Multi-step
      Macro, Stepper, Profile switch → `None`.
- [ ] `#[allow(dead_code)]` (or equivalent) on `hold_repeat_kind` and `HoldKind`
      with a one-line comment pointing at ticket 03 — `decide` does not call it yet.
- [ ] `single_held_key` unit-tested standalone per §6.1: every qualifying shape, each
      disqualifying shape, the trailing-`Delay` boundary (one trailing `Delay` ⇒
      `Some`, a `Delay` between the keys ⇒ `None`, two trailing `Delay`s ⇒ `None`),
      the modifier-order match (mods in `keypress_steps` order ⇒ `Some`, out of order
      ⇒ `None`).
- [ ] `hold_repeat_kind` classification tests: keyboard Keypress (plain + modified) ⇒
      `AutorepeatKey`; mouse-button Keypress ⇒ `SustainedNoRepeat`; `ControllerButton`
      ⇒ `SustainedNoRepeat`; single-key Macro ⇒ `AutorepeatKey` with the same
      `(mods, key)` the equivalent Keypress produces; multi-step Macro ⇒ `None`;
      `Step` ⇒ `None`.
- [ ] `cargo fmt --check`, `cargo clippy --all-targets`, daemon suite all clean —
      including the existing `trigger::tests::decision_table`, unchanged.
