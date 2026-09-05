# 12 — "Reset to Profile default" also returns the deep band

**What to build:** In the dual-stage grid-key editor, **Reset to Profile
default** resets the primary Actuation/Release markers (clears the per-key
override so it falls back to `default_actuation`) but leaves the deep markers
untouched. On a key that has a deep stage it should return the deep band too.

The deep band has no daemon-side override/default split — every deep stage
carries its own `deep_stages[inp]` entry, and `Profile.default_deep_actuation`
(ticket 08) is the only Profile-level deep band. So "return to Profile
default" for the deep stage means: re-seed from `default_deep_cfg()` (the
remembered `default_deep_actuation` clamped disjoint from the just-reset
primary, else the `+20 / +35` offset) and push it with
`client.set_deep_actuation`.

Post-ship follow-up from Charon's testing. Behaviour-only, GUI-only, no
daemon change (reuses the existing `SetDeepActuation` D-Bus method).

**Status:** resolved

- [x] With a deep stage present, **Reset to Profile default** clears the
      primary override *and* pushes `set_deep_actuation` with
      `default_deep_cfg()`'s band, recomputed against the reset primary.
- [x] The deep band's disjoint-from-primary clamp still holds (the reset
      primary may be shallower/deeper than the old override).
- [x] With no deep stage, the button is unchanged — primary only, no
      `set_deep_actuation` call.
- [x] Staging mode is not touched (the report is about the sliders).
- [x] The rebuilt bar shows the reset deep band, not the pre-reset one.
- [x] GUI tests cover the deep-band reset and the no-deep-stage no-op.

## Answer

`binding_editor.build_dual_stage_panel.on_reset_primary()` gains a
`has_deep()` branch after the existing primary clear + `on_commit()`: it reads
`default_deep_cfg()["actuation"]`, calls
`client.set_deep_actuation(inp, band["actuation"], band["release"])`, mutates
`profile_dict["deep_stages"][inp]["actuation"]` in place (the "+ New Macro"
snapshot precedent, matching `on_marker_drag_end`'s own deep push), and then
`rebuild()`s — a Daemon rejection surfaces via `show_error` + `rebuild()` and
leaves the primary already reset. `default_deep_cfg()` reads
`resolved_primary()`, which reflects the just-popped override, so the reseeded
band is clamped against the reset primary rather than the old one.

`"Reset all keys to Profile default"` is left alone — it hits
`reset_actuation_points()` (primary overrides only) and there is no bulk deep
equivalent on the Daemon; out of scope for this report.

Tests: `test_binding_editor.py` gains
`test_reset_to_profile_default_also_returns_the_deep_band` (remembered band
distinct from the dragged one; asserts the `set_deep_actuation` call, the
snapshot, and the rebuilt markers) and
`test_reset_to_profile_default_with_no_deep_stage_touches_only_the_primary`.
Full GUI suite 483 pass.
