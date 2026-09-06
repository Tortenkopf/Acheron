# 04 — Analog-repeat selection hint

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

- [ ] A dim `["dim"]` `Gtk.Label` (same style as `_CONTROLLER_MACRO_HINT`) appears
      directly below the Trigger-mode dropdown whenever the selected Trigger mode is
      `analog_repeat`, and is removed the moment the selection changes away.
- [ ] Works in every editor that offers the selector — the individual binding editor and
      the deep-stage editor (both flow through `build_binding_editor_fields`).
- [ ] Copy is exactly the three sentences from spec §4, leading with `⚠️`.
- [ ] The show/hide reaction does not accumulate one stale `trigger_dd` listener per
      `render_action_editor()` rebuild (reuse the persistent-handler pattern already used
      for the ticket-42 Trigger-mode-dependent key warning; note that handler's single
      slot is currently keypress-only, so this hint likely needs its own persistent slot
      since it must react for every Action kind that shows the selector).
- [ ] No config-layer change, no `ConfigError`, no blocked Save.
