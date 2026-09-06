# 03 — Thread `&macros` into `trigger::decide`, switch to `hold_repeat_kind`

**What to build:** The seam that lets `decide` see a Binding's held-key shape — its
`macros` parameter and the swap from `sustained_hold_key` to ticket 02's
`hold_repeat_kind`. Still **no behaviour change**: `decide` maps `HoldKind::AutorepeatKey`
onto exactly the decisions `sustained_hold_key`'s `None` produced today (the ordinary
`SpawnFireOnce` / `StartToggleLoop` arms). Ticket 04 flips those arms to the `value=2`
path. This ticket is the mechanical call-site fan-out, kept on its own so tickets 04
and 05 start from a green tree with the parameter already in place.

Source of truth: [`spec-kernel-shaped-repeat.md`](../../humane-output-rate/spec-kernel-shaped-repeat.md)
§3 (first paragraph — the parameter and its call sites).

**Blocked by:** 02 — `single_held_key` predicate + `HoldKind` classifier.

**Status:** ready-for-agent

- [ ] `trigger::decide` gains `macros: &HashMap<MacroId, MacroDef>` and calls
      `hold_repeat_kind(&binding.action, macros)` instead of
      `sustained_hold_key(&binding.action)`.
- [ ] `SustainedNoRepeat(code)` reproduces today's `hold_key.is_some()` arms verbatim
      (bare `HoldKeyDown` on `Down`, `Nothing` on `Repeat`, `StartToggleHeld` on
      `Toggle` `Down`). `AutorepeatKey(_, code)` maps to today's keyboard-Keypress
      behaviour: `guarded(D::SpawnFireOnce)` on `Down`/`Repeat` for
      `HoldToRepeat`/`AnalogRepeat`, `D::StartToggleLoop` on `Toggle` `Down`. `None`
      unchanged.
- [ ] `sustained_hold_key` deleted (its only caller was `decide`).
- [ ] All 5 `trigger::decide` call sites pass the macro map, each already having it in
      scope:
  - `dispatch::handle_event` (`Repeat | Up` arm) — `config.macros`;
  - `dispatch::run_chord_effects` (`FireChord`) — `config.macros`;
  - `dispatch::dispatch_individual_down` — `config.macros`;
  - `stage.rs` deep-stage `Down` (~`:396`) — `deps.config.macros`;
  - `stage.rs` deep-stage `Repeat` (~`:857`) — `deps.config.macros`.
- [ ] `trigger::tests::decision_table` updated for the new signature (thread an empty
      / small macro map through) and asserting the **unchanged** decisions — plus a
      new row proving a single-key `Action::Macro` currently resolves to the same
      decision as the equivalent `Action::Keypress` (locks the "no behaviour change
      yet" contract that ticket 04 then deliberately breaks).
- [ ] `cargo fmt --check`, `cargo clippy --all-targets`, the full daemon suite green —
      no dispatch/stage/chord integration test changes expected.
