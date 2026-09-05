# 07 — Dual-stage binding-editor GUI

**What to build:** A user configures a grid key's deep stage directly from the
binding editor — adding one via `+ Add deep stage`, editing its Trigger/Action/
Actuation/Staging-mode exactly as fluidly as the primary, and removing it — using
the confirmed "swap toggle" layout, wired to the real daemon (ticket 05) and torn
down correctly by the daemon's own runtime behavior (ticket 06). Plus the README
gains its "Dual-stage keys" section. This is the last ticket — the feature is fully
usable end-to-end after it lands.

Source of truth: [`spec.md`](../../tartarus-dual-stage-keys/spec.md) §"GUI
binding-editor layout", prototyped at
[`prototype/04-dual-stage-binding-editor-layout/prototype.py`](../../../prototype/04-dual-stage-binding-editor-layout/prototype.py)
(Variant A — swap toggle — won, confirmed live against the real Key/Controller-button
pickers; carry forward the *intent* of its bounded/scrollable container, not its
standalone-test-harness pixel numbers).

**Blocked by:** 05, 06

**Status:** resolved

- [x] The Actuation section becomes a shared **4-marker** bar — primary green/amber
      + deep blue (`#3498db`) actuation / purple release, fixed-width sized to
      match the real key-picker row's own natural width (not `hexpand`), marker
      order left-to-right: primary release, primary actuation, deep release, deep
      actuation. Legend uses real colour swatches in the same order. Greys with a
      "No depth — analog capture unavailable" note in Digital mode (deep markers
      grey too).
- [x] A Primary/Deep toggle row: `[Primary — <summary>]` toggle, and in the same
      slot next to it, `+ Add deep stage` until one exists, replaced by
      `[Deep — <summary>]` plus a square red `✕` (tooltip "Remove deep stage") once
      it does. Both share one mutually-exclusive toggle group — at most one stage
      selected for editing at a time.
- [x] A staging-mode row (Handoff/No-Return/Additive/Quick-Skip, one-line tooltip
      each) below the Primary/Deep row, rendered only once a deep stage exists.
      Greys with a "Requires analog capture" note in Digital mode.
- [x] One editor slot below holding the real Trigger-mode dropdown, Action-kind
      dropdown, and the real, unmodified `key_picker`/`controller_picker` widgets
      for whichever stage is currently toggled — structurally, only one stage's
      fields (and so only one picker) are ever mounted in the tree at once (the
      hard "never two pickers on screen at once" constraint, satisfied
      structurally, not via CSS hiding). The deep stage's picker highlights its
      current selection in the deep-actuation marker's blue (`#3498db`, with
      `background-image: none` to override the theme's `.suggested-action` accent)
      rather than the theme's generic accent.
- [x] Bind-primary-first gate: with no primary Binding, the whole panel below the
      Actuation bar collapses to one line ("Bind a primary Action first…") — no
      Add-deep-stage affordance, no editor.
- [x] The panel sits in its own bounded/scrollable container the way the existing
      `actuation_scroller` already does, so a deep stage's addition growing the
      panel stays bounded by the popover's own sizing.
- [x] Wired to the real `client.set_deep_stage`/`clear_deep_stage`/
      `set_deep_actuation`/`set_staging_mode` (ticket 05) — every edit rebuilds
      from `GetConfig`/stub config on the shared `on_change`, like the rest of the
      editor.
- [x] GUI tests via `DaemonStub`: the bind-primary-first gate collapses correctly
      with no primary Binding; adding/removing a deep stage toggles the Primary/
      Deep row and staging-mode row correctly; only one stage's picker is ever
      present in the built widget tree at once; deleting the primary (ticket 06's
      cascade) clears the deep-stage display too; Digital-mode greying on both the
      bar and the staging-mode row; the seven `daemon_stub.py` validation rules
      surface as errors in the editor the same way existing rejection paths do.
- [x] README gains a "Dual-stage keys" section (a driving-sim half-throttle/
      full-throttle framing example, per spec.md's Out-of-Scope note that this
      copy is left to the implementation effort), written and placed consistent
      with how other feature sections read (e.g. the Status LEDs section added
      post-ticket-04).

## Answer

Landed as `binding_editor.build_dual_stage_panel` (`gui/acheron_gui/binding_editor.py`),
the "Variant A — swap toggle" prototype made real. Decisions taken where the ticket /
prototype left room:

- **Panel integration.** For a Grid key that *already carries a primary Binding*, the
  panel is the whole editor — it owns the sole Trigger/Action editor slot (primary or
  deep, swapped by the toggle, so only one `key_picker`/`controller_picker` is ever
  mounted) plus the shared 4-marker bar and the primary-actuation profile-default
  controls carried over from `build_actuation_section`. An *unbound* Grid key keeps the
  plain editor and gains a dim "Bind a primary Action first to add a deep stage." line
  where `build_actuation_section` used to sit alone — the ticket's "no editor" gate
  state. `build_binding_editor` branches on `is_grid_input(inp) and existing is not None
  and current_axis_target is None`.
- **Refresh model.** Structural deep edits (`+ Add deep stage`, `✕`, the staging-mode
  row) hit the Daemon, mutate the passed-in `config` snapshot in place, and rebuild the
  panel locally — the window stays open (the `+ New Macro` in-place-snapshot precedent,
  and the same "this edit only touches this one key" reasoning that keeps
  `set_actuation_point` from popping the popover down). Marker drags persist to the
  snapshot on drag-end so a later stage-toggle rebuild doesn't snap them back. Save /
  Clear still go through the shared `on_saved` (close + app rebuild) like the rest of
  the editor; clearing the primary rides ticket 06's stub cascade back to the
  bind-primary-first gate.
- **`+ Add deep stage`** seeds a disjoint band (`deep.release = primary.actuation + 20`,
  `deep.actuation = deep.release + 35`, clamped) via `set_deep_actuation` *then*
  `set_deep_stage` with a Hold-to-repeat `KEY_A` default — order matters
  (`DeepStageMissingConfig`). A `set_deep_stage` rejection (Chord member,
  `analog_repeat` primary, …) surfaces on the shared error label and leaves the inert
  `deep_stages` entry, which spec.md permits.
- **Deep Action-kind menu** is Keypress / Controller Button / Switch Profile only
  (`_DEEP_ACTION_TYPES`); the deep picker carries `.deep-picker` so its selection paints
  in the deep-actuation blue (`#3498db`, `background-image: none` to beat the theme
  accent).
- **4-marker bar** is `DepthTrack` with a new `fixed_width` mode (the prototype's
  finding that a live `hexpand` width made markers jump mid-drag); `_enforce_ascending`
  is the N-marker generalisation of the existing 2-marker anti-cross clamp. Digital mode
  greys the bar (`depth-track-dim` + insensitive, so the deep markers grey with it) and
  the staging-mode row, each with its own centred note.

New CSS in `app.py::CSS`: `.marker-deep-actuation` / `.marker-deep-release`,
`.deep-picker …`, `.staging-mode-row button`, `.icon-btn`. `STAGING_MODES` and
`_DEEP_ACTION_TYPES` live in `binding_editor.py`. README gains a **Dual-stage keys**
Features bullet and a `### Dual-stage keys` Usage subsection (driving-sim
half-throttle / full-throttle framing, per spec.md's Out-of-Scope note).

Tests: `gui/tests/test_binding_editor.py` gained 20 cases covering the bind-primary-first
gate, add/remove toggling the Deep row + staging row + marker count, the
one-picker-mounted invariant across every toggle, the `.deep-picker` class, staging-mode
wiring, deep-stage Save, the primary-clear cascade, Digital-mode greying, the
Chord-member / `analog_repeat` rejections surfacing on the error label, marker-drag
persistence, and no depth stream at construction. 449 GUI tests pass; 463 daemon tests,
`cargo clippy`/`fmt` unchanged and clean.
