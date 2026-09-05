# 09 — Unify the grid-key binding editor on the dual-stage panel

**What to build:** Opening the binding editor for any grid key — one that
already carries a primary Binding *and* one that doesn't — shows the same
panel with the same vertical order: the shared Actuation bar, then the
Primary/Deep toggle row, then (deep only) the Staging-mode row, then the
single Trigger/Action editor slot, then the primary-actuation profile-default
controls. The two visually different layouts a user sees today — the plain
Trigger/Action fields + a separate boxed Actuation section for an unbound key,
versus the swap-toggle panel for a bound one — collapse into one.

An unbound grid key renders inside that panel as:

- a synthetic primary stage (Keypress / `KEY_A` / the Input's default Trigger
  mode), shown on the `Primary — …` toggle as the passthrough-default label;
- the shared Actuation bar with just the two primary markers (release +
  actuation), drawn and drag-editable in analog Capture mode, backed by the
  Profile's `default_actuation` / per-key override exactly as the standalone
  Actuation section shows it today;
- `+ Add deep stage` **disabled**, with a tooltip along the lines of "Save or
  Apply a primary Action first";
- `Clear Binding` **disabled** (nothing bound, no axis assignment to clear).

The old separate unbound branch in the editor and its dim "Bind a primary
Action first to add a deep stage." line are removed. The plain
Trigger/Action-only path plus the standalone Actuation section are kept *only*
as the fallback for a pre-dual-stage Daemon whose `GetConfig()` has no
`deep_base` / `deep_held` / `deep_stages` keys.

So that a later in-place rebuild (ticket 10) can reflect a freshly-committed
primary, the panel reads its primary Binding from the config snapshot for the
Input/Layer rather than from a fixed constructor argument — the same way it
already reads the deep Binding.

The panel's own bounded/scrollable container grows with its content toward the
screen edge before it starts scrolling: the monitor-relative height formula
keeps its shape but its chrome allowance shrinks from ~280px to roughly a
titlebar's worth (~96px), leaning on the container's natural-height
propagation so short editors stay compact and tall ones (deep stage + an
expanded key picker) reach near full screen height, then scroll.

Scope is grid keys. Non-grid Inputs (Mode key, thumbstick, wheel) and the
Chord binding dialog are untouched here.

**Blocked by:** None — can start immediately (tickets 01–08 resolved).

**Status:** resolved

- [x] Every grid key with dual-stage config present opens the swap-toggle
      panel — no more `existing is not None` gate on which layout appears.
- [x] Unbound grid key: synthetic primary stage, `Primary — <default label>`
      toggle, 2-marker Actuation bar backed by the Profile actuation defaults,
      identical in layout to a bound key that has no deep stage.
- [x] `+ Add deep stage` disabled with an explanatory tooltip while no primary
      Binding exists; `Clear Binding` disabled while there is nothing to clear.
- [x] The dim "Bind a primary Action first…" line and the separate unbound
      Trigger/Action + boxed Actuation layout are gone from the normal path.
- [x] The plain path + standalone Actuation section remain reachable *only* as
      the `deep_base`-absent version-skew fallback (and for an Axis-assigned
      key, which was already the case).
- [x] The panel reads its primary Binding from the config snapshot, not a
      fixed argument.
- [x] The panel's scroll container hugs content and only scrolls once content
      approaches the screen edge (chrome allowance reduced to ~96px); short
      editors are not forced to a tall minimum.
- [x] Non-grid Inputs and the Chord dialog render exactly as before.
- [x] GUI tests via `DaemonStub`: an unbound grid key builds the panel (not
      the old plain layout); its `+ Add deep stage` and `Clear Binding` are
      insensitive; the synthetic primary stage is a Keypress with the Input's
      default Trigger mode; the one-picker-mounted invariant still holds; the
      `deep_base`-absent fallback still renders the plain path.
- [x] `gui/tools/shot_binding_editor.py` still produces a representative
      screenshot (unchanged — grid Key 1 now renders the unified panel).

## Answer

Landed in `gui/acheron_gui/binding_editor.py`.

- **`build_binding_editor` gate.** Dropped the `existing is not None` clause —
  now `is_grid_input(inp) and current_axis_target is None and "deep_base" in
  profile`. Every non-Axis grid key on a dual-stage-aware Daemon takes
  `build_dual_stage_panel`; the plain Trigger/Action path + standalone
  `build_actuation_section` remain only for an Axis-assigned key or a
  `deep_base`-absent `GetConfig()` (version skew). The old
  `current_axis_target is None` sub-block that printed "Bind a primary Action
  first to add a deep stage." is deleted.
- **`build_dual_stage_panel` reads the primary from the snapshot.** The
  `primary_binding: dict` constructor argument is gone; the panel now has
  `primary_snapshot()` (`profile_dict[layer].get(inp)`), `has_primary()`, and
  `primary_binding()` (snapshot or the synthetic stage) — the same shape as
  the existing `deep_binding()`. Ticket 10's in-place rebuild after a fresh
  primary commit will reflect it for free.
- **Synthetic primary stage.** `synthetic_primary = {default_trigger_for(inp),
  keypress, KEY_A, []}`. The editor slot builds from it; the `Primary — …`
  toggle shows `action_summary(primary_snapshot(), …)` → the passthrough
  default label (`INPUT_DEFAULT_LABEL`) while unbound. `on_save_stage` commits
  the synthetic stage unconditionally (`not has_primary() or primary_target !=
  primary_binding()`), matching the old plain editor's "Save always calls
  `set_binding`".
- **Disabled affordances.** `rebuild()` sets `clear_btn.set_sensitive(
  has_primary())` and, on the `+ Add deep stage` button,
  `set_sensitive(has_primary())` + tooltip "Save a primary Action first"
  (ticket 10 widens this to "Save or Apply …" when it adds the Apply
  button — a bare "or Apply" now would name a control that isn't on screen).
  `on_add_deep` / `on_clear_stage` also guard on `has_primary()` so a
  directly-emitted `clicked` on the disabled button is inert (the
  already-unbound `Clear` still closes with no D-Bus call, like the old path).
- **Scroll container.** `_dual_stage_scroller_max_height()` chrome allowance
  280 → 96 (per the ticket); `propagate_natural_height` already lets a short
  editor stay compact. Code review flagged that the heading + Save/Clear row
  + margins sit in `box` *outside* `panel_scroller`, so a very tall panel
  (deep stage + fully-expanded key picker) on a small monitor could push the
  Save/Clear row under the shell's bottom chrome — worth a look during
  hardware verification; bump the allowance if it clips.

Tests: `gui/tests/test_binding_editor.py` — rewrote
`test_unbound_grid_key_shows_the_bind_primary_first_gate_and_no_deep_affordance`
into `…_shows_the_unified_panel_with_a_synthetic_primary_stage`, added
`…_add_deep_stage_is_inert_while_disabled` and
`…_save_with_no_edits_still_creates_the_placeholder_binding`, and updated
`test_clearing_the_primary_cascades_the_deep_stage_away`'s post-cascade
assertions (net +2 cases). 464 GUI tests pass. No daemon changes.
