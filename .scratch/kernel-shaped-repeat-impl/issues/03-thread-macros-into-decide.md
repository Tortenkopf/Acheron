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

**Status:** done

- [x] `trigger::decide` gains `macros: &HashMap<MacroId, MacroDef>` and calls
      `hold_repeat_kind(&binding.action, macros)` instead of
      `sustained_hold_key(&binding.action)`. `decide`'s `hold_key` local now =
      `Some(code)` only for `HoldKind::SustainedNoRepeat(code)`; `AutorepeatKey(..)`
      and `None` collapse to `None` there.
- [x] `SustainedNoRepeat(code)` reproduces today's `hold_key.is_some()` arms verbatim
      (bare `HoldKeyDown` on `Down`, `Nothing` on `Repeat`, `StartToggleHeld` on
      `Toggle` `Down`). `AutorepeatKey(_, code)` maps to today's keyboard-Keypress
      behaviour: `guarded(D::SpawnFireOnce)` on `Down`/`Repeat` for
      `HoldToRepeat`/`AnalogRepeat`, `D::StartToggleLoop` on `Toggle` `Down`. `None`
      unchanged.
- [x] `sustained_hold_key` deleted (its only caller was `decide`).
- [x] All 5 `trigger::decide` call sites pass the macro map (`&config.macros` at each —
      `config` is destructured from `deps` at the two `stage.rs` sites):
  - `dispatch::handle_event` (`Repeat | Up` arm);
  - `dispatch::run_chord_effects` (`FireChord`);
  - `dispatch::dispatch_individual_down`;
  - `stage::fire` deep-stage `Down`;
  - `stage::Engine::deep_repeat` deep-stage `Repeat`.
- [x] `trigger::tests::decision_table` threads a real `hold-a` macro map (`[KeyDown(A),
      KeyUp(A)]`) through a `decide` shadow closure and asserts every existing row
      unchanged, plus a new row asserting a single-key `Action::Macro` decides
      arm-for-arm like the equivalent `Action::Keypress` under `HoldToRepeat` (Down /
      Repeat / Up) and `Toggle` (`StartToggleLoop`). `#[allow(dead_code)]` dropped
      from `HoldKind` / `hold_repeat_kind`.
- [x] `cargo fmt --check`, `cargo clippy --all-targets` clean; full daemon suite green
      (492 passed) — no dispatch/stage/chord integration test changes needed.

## Comments

### Code-review follow-ups (2026-09-06)

Two `/code-review` findings against the working tree, both addressed:

1. **Not *quite* verbatim-unchanged — a single mouse-button `Macro`.** `sustained_hold_key`
   returned `None` for *every* `Action::Macro`; ticket 02's `hold_repeat_kind` deliberately
   makes a single-key Macro classify like the equivalent `Keypress`, so a Macro compiling to
   exactly `[KeyDown(BTN_*), KeyUp(BTN_*)]` is now `SustainedNoRepeat(BTN_*)` and takes the
   latched-hold arms (`HoldKeyDown` / `StartToggleHeld`) instead of looping / pulsing. This
   matches the spec's stated "single-key Macro == equivalent Keypress" contract (§2.1, ticket
   02's `hold_repeat_kind_single_mouse_button_macro_matches_the_equivalent_keypress` test) and
   a `Keypress` on `BTN_*` can never itself be a Macro, so no keyboard-key path regresses.
   Locked with `single_mouse_button_macro_takes_the_sustained_hold_arms_like_the_equivalent_keypress`.
   Edge note for ticket 04+: a *modifier-wrapped* single mouse-button Macro (`Ctrl+Click`)
   loses its modifiers through `key_hold_kind` → `SustainedNoRepeat` (the decision carries one
   `KeyCode`, no mods) — exotic, pre-existing in ticket 02's classifier, not widened here.
2. **`hold_repeat_kind` off the hot path for `FireOnce` / `AnalogRepeat`.** `decide` now gates
   the classification (an `executor::compile` of an `Action::Macro` among it) behind
   `matches!(binding.trigger, HoldToRepeat | Toggle)` — the only arms that consult `hold_key`.
   The old eager `sustained_hold_key` computed-but-never-read it for the other two modes too,
   but that was a two-arm match, not a macro compile. The `HoldToRepeat`-Macro-per-`Repeat`
   double-compile (once to classify → `None`, once in `compile_action`) is inherent to the
   pure `decide`/`perform` seam and left as-is; a held multi-step Macro already spawns a whole
   firing per tick.
