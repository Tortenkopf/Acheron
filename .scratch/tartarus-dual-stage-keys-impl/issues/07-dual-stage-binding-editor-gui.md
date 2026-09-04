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

**Status:** ready-for-agent

- [ ] The Actuation section becomes a shared **4-marker** bar — primary green/amber
      + deep blue (`#3498db`) actuation / purple release, fixed-width sized to
      match the real key-picker row's own natural width (not `hexpand`), marker
      order left-to-right: primary release, primary actuation, deep release, deep
      actuation. Legend uses real colour swatches in the same order. Greys with a
      "No depth — analog capture unavailable" note in Digital mode (deep markers
      grey too).
- [ ] A Primary/Deep toggle row: `[Primary — <summary>]` toggle, and in the same
      slot next to it, `+ Add deep stage` until one exists, replaced by
      `[Deep — <summary>]` plus a square red `✕` (tooltip "Remove deep stage") once
      it does. Both share one mutually-exclusive toggle group — at most one stage
      selected for editing at a time.
- [ ] A staging-mode row (Handoff/No-Return/Additive/Quick-Skip, one-line tooltip
      each) below the Primary/Deep row, rendered only once a deep stage exists.
      Greys with a "Requires analog capture" note in Digital mode.
- [ ] One editor slot below holding the real Trigger-mode dropdown, Action-kind
      dropdown, and the real, unmodified `key_picker`/`controller_picker` widgets
      for whichever stage is currently toggled — structurally, only one stage's
      fields (and so only one picker) are ever mounted in the tree at once (the
      hard "never two pickers on screen at once" constraint, satisfied
      structurally, not via CSS hiding). The deep stage's picker highlights its
      current selection in the deep-actuation marker's blue (`#3498db`, with
      `background-image: none` to override the theme's `.suggested-action` accent)
      rather than the theme's generic accent.
- [ ] Bind-primary-first gate: with no primary Binding, the whole panel below the
      Actuation bar collapses to one line ("Bind a primary Action first…") — no
      Add-deep-stage affordance, no editor.
- [ ] The panel sits in its own bounded/scrollable container the way the existing
      `actuation_scroller` already does, so a deep stage's addition growing the
      panel stays bounded by the popover's own sizing.
- [ ] Wired to the real `client.set_deep_stage`/`clear_deep_stage`/
      `set_deep_actuation`/`set_staging_mode` (ticket 05) — every edit rebuilds
      from `GetConfig`/stub config on the shared `on_change`, like the rest of the
      editor.
- [ ] GUI tests via `DaemonStub`: the bind-primary-first gate collapses correctly
      with no primary Binding; adding/removing a deep stage toggles the Primary/
      Deep row and staging-mode row correctly; only one stage's picker is ever
      present in the built widget tree at once; deleting the primary (ticket 06's
      cascade) clears the deep-stage display too; Digital-mode greying on both the
      bar and the staging-mode row; the seven `daemon_stub.py` validation rules
      surface as errors in the editor the same way existing rejection paths do.
- [ ] README gains a "Dual-stage keys" section (a driving-sim half-throttle/
      full-throttle framing example, per spec.md's Out-of-Scope note that this
      copy is left to the implementation effort), written and placed consistent
      with how other feature sections read (e.g. the Status LEDs section added
      post-ticket-04).
