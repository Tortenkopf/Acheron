# 05: Stop the remaining per-rebuild leak in the main view

Spec: [`../spec.md`](../spec.md). Testing Decisions, "Rebuild soak test".

**What to build:** A full Device Overview rebuild retains nothing, in every destination. After
02–04, the remaining known survivor is the main tree's small residual (two Popovers, two Entries,
two Labels per rebuild, the shared name-prompt popover shape). Fix it and anything else the full
soak test shows, and land that test as the permanent regression guard.

**Blocked by:** 02, 03, 04.

**Status:** done (2026-10-15)

- [x] Full rebuild soak test over the Grid destination, the Library destination (Macro and
      Stepper kinds), and the Lighting tab. After N rebuilds, no widget from build 1 is alive,
      and the live `Gtk.Widget` wrapper count doesn't grow between build K and build N.
- [x] Top-level window count is 0 after the soak when no editor is open.
- [x] N is kept small (tens) so the suite stays fast. Today's growth is about 558 wrappers per
      rebuild, so a flat count at N ≈ 10–20 is decisive.
- [x] The Profile sidebar's new/rename prompts and the Library's prompts still work (existing
      tests stay green).

## Comments

- 2026-10-15: Done. `test_rebuild_soak.py` has two new tests, each run against Grid, Library (Macros
  tab and Steppers tab, each with one entry so its editor columns are built) and Lighting. The first
  checks that after 15 rebuilds no widget from build 1 is alive (GObject weak refs) and only the host
  window is a top-level. The second checks that the live `Gtk.Widget` wrapper count after 3 + 15
  rebuilds is no higher than after 3. Grid and Lighting already passed: ticket 04's name-prompt
  popover fix had removed the main tree's Popover/Entry residual (reverting that fix makes the Grid
  tests fail again). Only the Library leaked, about 7 wrappers and its whole tree per rebuild. The
  shared `_mount_picker_mode_switch` `rerender` captured `switch_slot`, which is reached from the
  switch buttons inside it. Fixed with `switch_slot.weak_ref()`. 648 GUI tests pass.
