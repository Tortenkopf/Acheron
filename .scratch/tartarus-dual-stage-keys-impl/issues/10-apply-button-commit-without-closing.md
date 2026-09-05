# 10 — "Apply": commit the binding without closing the editor

**What to build:** A user binding a grid key that had no Binding can commit
that Action and keep working in the same editor window — watching it turn into
the bound layout in place (the `Primary — …` summary fills in, `+ Add deep
stage` becomes enabled) — and then add a deep stage, all without the editor
ever closing and reopening.

A new plain (non-accent) **Apply** button in the grid-key editor does this. On
click it commits exactly what **Save** commits: both stages, folding the
on-screen stage's draft in first, pushing primary then deep, each only when it
actually differs from the snapshot; same disabled conditions as Save (an
Action kind with no editor, an empty Macro/Stepper library). On success it
mutates the in-memory config snapshot and rebuilds the panel locally — the
window stays open, no full app rebuild. Apply is otherwise always enabled (a
redundant Apply is a harmless no-op push).

**Save** keeps today's behaviour: commit, then close the window.

The full app rebuild (`on_change()`) moves off the per-save path and onto the
editor window's close-request handler, gated on a session flag that is set the
first time Save or Apply commits anything. Net effect:

- Save → commit → close → `on_change()` fires once.
- Apply (n times) → commit → local rebuild; dismiss the window → `on_change()`
  fires once.
- Open and close without committing → nothing; the hide-on-close cached
  window and its "reopen is instant" behaviour are preserved.

Scope is the grid-key editor. Non-grid Inputs and the Chord dialog get no
Apply button.

**Blocked by:** 09

**Status:** resolved

- [x] Apply button present in the grid-key editor; plain styling, Save keeps
      the accent.
- [x] Apply commits both stages with the same "only if changed" and same
      disabled-state rules as Save.
- [x] Apply keeps the window open and rebuilds the panel from the mutated
      snapshot; an unbound→bound transition is visible live (toggle summary,
      `+ Add deep stage` enabling, `Clear Binding` enabling).
- [x] Save still commits and closes.
- [x] `on_change()` fires from the window's close-request handler, once, only
      when something was committed this session; a pure open/close does not
      trigger a rebuild and does not destroy the cached window.
- [x] GUI tests via `DaemonStub`: Apply on an unbound key creates the Binding
      and the rebuilt panel shows the bound layout with the window still open;
      Apply then `+ Add deep stage` works in one window session; Apply is
      insensitive under the same conditions Save is; close after Apply drives
      exactly one `on_change()`; open+close with no commit drives none.

## Answer

Landed in `gui/acheron_gui/binding_editor.py` + `gui/acheron_gui/device_overview.py`.

- **`commit_stages()` seam.** `build_dual_stage_panel.on_save_stage` was
  refactored into a shared `commit_stages() -> bool` (push both stages, each
  only if it differs from the snapshot / synthetic primary unconditionally,
  primary before deep) that *also* mutates the in-memory `config` snapshot on
  every landed push (`profile_dict[layer][inp] = …` / `deep_map()[inp] = …`,
  the "+ New Macro" precedent) and clears that stage's draft, and fires a new
  `on_commit` callback the first time any push lands. `on_save_stage` is now
  `if commit_stages(): on_saved()`; `on_apply_stage` is `commit_stages();
  rebuild()`.
- **Apply button.** Built plain (no `suggested-action`) in
  `build_binding_editor` as `apply_btn`, appended to the button row **only**
  in the grid-key dual-stage branch — the non-grid / version-skew paths and
  `build_chord_binding_dialog` never get it. Its sensitivity mirrors
  `save_btn` via a one-time `save_btn` `notify::sensitive` handler, so the
  "no editor for this Action kind" / "empty Macro or Stepper library"
  disables carry over without re-deriving the matrix.
- **`on_change()` moved to the window.** `make_input_button` now holds a
  `committed` flag, set by `on_commit` (Apply's first push) and by `on_saved`
  (Save/Clear/set-default/reset-all). The full-app rebuild runs from a
  `close-request` handler (`rebuild_if_committed`, one-shot: clears the flag
  first). `on_saved` still calls `window.close()` *and* `rebuild_if_committed`
  directly, because `Gtk.Window.close()` only emits `close-request` for a
  realized window — on a realized window the handler's second run is a no-op
  (flag already cleared). A pure open/close leaves the cached hide-on-close
  window untouched and drives no rebuild.
- **Code-review follow-ups (self-review, applied):**
  - The panel's own structural edits that commit real Daemon state and
    rebuild locally (`+ Add / Remove deep stage`, staging-mode pick,
    actuation-marker drag, `Reset to Profile default`) now also call
    `on_commit()`, so dismissing the window after *only* one of those still
    drives the one deferred `on_change()` that refreshes the other cached
    editors' snapshots. (This gap pre-dated the ticket — the old code never
    fired `on_change()` for those either — but the close-request funnel is
    where it belongs.)
  - Apply with an **Axis** primary now falls back to Save's close-and-reopen
    (`on_saved()`), because an Axis assignment is not a Binding and has no
    representation in the swap panel — a local rebuild would strand the
    editor on the synthetic-primary layout with `Clear Binding` disabled.
  - `apply_btn` is constructed inside the grid-key dual-stage branch, not
    once per `build_binding_editor` call (it was an orphan widget on every
    non-grid / version-skew rebuild).
- **Tooltip.** `+ Add deep stage`'s disabled tooltip widened to "Save or
  Apply a primary Action first" (ticket 09 deferred this here).
- No ADR — this is GUI plumbing within the already-recorded dual-stage
  binding-editor design (ADR-0007 covers the depth semantics, not the editor
  UX).

Tests: `gui/tests/test_binding_editor.py` — +11 cases covering plain-vs-accent
styling, no-Apply on non-grid/Chord, Apply-binds-and-stays-open, Apply →
`+ Add deep stage` in one session, close-after-Apply → exactly one
`on_change()`, open+close → none, Save-after-Apply → one, Apply sensitivity
mirrors Save, a no-op Apply not arming the deferred rebuild, Apply-with-Axis →
close-and-reopen, and `+ Add deep stage` then dismiss → one `on_change()`.
Full GUI suite 475 pass.
