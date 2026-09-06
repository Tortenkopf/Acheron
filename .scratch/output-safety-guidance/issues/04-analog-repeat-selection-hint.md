# 04 — Analog-repeat selection hint

Status: done — 2026-09-07

**What to build:** In any editor that offers the Trigger-mode selector, choosing
**Analog-repeat** reveals an advisory line directly below the dropdown: it can drive a key
far faster than a hand could near full travel, so single-player / known-safe games only,
never competitive multiplayer. Changing the selection away from Analog-repeat removes the
line immediately.

**Blocked by:** None — can start immediately.

Source: `.scratch/humane-output-rate/spec-user-facing-output-safety-guidance.md` §4.
This is a **GUI hint** (tracks the live selection), not a Toast label — it deliberately
supersedes ticket 01 Q3's one-time-toast phrasing, so no `ui_state` seen-flag or dismiss
control.

- [x] A dim `["dim"]` `Gtk.Label` (same style as `_CONTROLLER_MACRO_HINT`) appears
      directly below the Trigger-mode dropdown whenever the selected Trigger mode is
      `analog_repeat`, and is removed the moment the selection changes away.
- [x] Works in every editor that offers the selector — the individual binding editor and
      the deep-stage editor (both flow through `build_binding_editor_fields`).
- [x] Copy is exactly the three sentences from spec §4, leading with `⚠️`.
- [x] The show/hide reaction does not accumulate one stale `trigger_dd` listener per
      `render_action_editor()` rebuild (reuse the persistent-handler pattern already used
      for the ticket-42 Trigger-mode-dependent key warning; note that handler's single
      slot is currently keypress-only, so this hint likely needs its own persistent slot
      since it must react for every Action kind that shows the selector).
- [x] No config-layer change, no `ConfigError`, no blocked Save.

## Comments

**Implemented 2026-09-07** (commit `1a77d99`). All work in
`gui/acheron_gui/binding_editor.py` + tests; no config, `rules`, or daemon change.

- `_ANALOG_REPEAT_HINT` module constant — spec §4 copy verbatim, one wrapped line,
  leading `⚠️` (the same no-emoji-norm exception as the ticket-02 disclaimer).
- The hint lives in `build_action_and_trigger_fields` (the real name of the ticket's
  `build_binding_editor_fields`) — the one editor core both the individual binding
  editor and the deep-stage editor flow through, so both get it. A Chord's own
  Binding also flows through it but never offers `analog_repeat` (`inp is None`), so
  the hint can't appear there.
- `sync_analog_hint()` genuinely inserts / removes the `["dim"]` `Gtk.Label` directly
  after the Trigger-mode row (`fields.insert_child_after` / `fields.remove`) as the
  live selection crosses `analog_repeat` — not a CSS-hidden reserve, matching the
  ticket's "removed the moment the selection changes away".
- **Persistent-handler choice:** rather than copy ticket-42's `_trigger_handler`
  dict-slot dance, the hint rides a single `trigger_dd.connect("notify::selected", …)`
  wired **once** at construction (never inside `render_action_editor()`), so it cannot
  accumulate a stale listener per rebuild. That slot pattern only exists because
  ticket-42's handler must be *disconnected* when leaving the keypress branch; this
  one never does. A belt-and-braces `sync_analog_hint()` also runs at the end of
  `render_action_editor()`, where an Action-kind switch can pull `analog_repeat` out
  of the model entirely (Profile Switch, Axis) or mid-rebuild `set_model()` fires
  `notify` while `trigger_keys` is still stale.
- **Tests:** 8 in `test_binding_editor.py` — exact copy, reveal/hide on selection,
  placement directly below the Trigger row, shows on open for an existing
  `analog_repeat` Binding, never reachable off-grid, tracks selection across
  Action-kind changes (incl. the Profile Switch clear), no duplicate labels after
  repeated kind cycling, and the deep-stage editor. Full GUI suite green at commit
  time (502 passed).
- **Interaction with humane-output-rate ticket 09** (Analog-repeat + Macro made
  unsaveable): that session updated the one shared test
  (`test_analog_repeat_hint_tracks_the_selection_across_action_kind_changes`) to
  expect the hint to clear when the Action kind becomes Macro. The hint code itself
  needs no change — it already tracks whatever the settled model allows.

Follow-up commit `7590f4d` (ticket 03): extended
`test_macro_binding_editor_does_not_carry_the_macro_editor_disclaimer` to also assert
no `Gtk.Expander` leaks into the assignment editor — the small test-hardening the
ticket-03 session had staged but left out of `1e706af`.
