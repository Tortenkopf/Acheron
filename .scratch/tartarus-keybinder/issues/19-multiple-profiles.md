# 19 — Multiple Profiles

**What to build:** Named, manually-switched Profiles — each a complete, independent Base+Held Binding set — with switching that cleanly kills any in-progress Toggle. See `.scratch/tartarus-keybinder/spec.md` ("Toggle behavior across Layer/Profile switches", D-Bus method list) for the full design.

**Blocked by:** 18

**Status:** ready-for-agent

- [ ] `CreateProfile`, `DeleteProfile`, `RenameProfile`, and `SwitchProfile` D-Bus methods exist, each atomic/immediately-applied/immediately-persisted to `config.toml`, per the conventions already established by `SetBinding`/`ClearBinding` (ticket 15) and `SetModeKeyRole` (ticket 18).
- [ ] Profiles are keyed by name (`HashMap<String, Profile>`); each independently carries its own Base/Held Binding maps and `mode_key_role`.
- [ ] `SwitchProfile` force-stops every currently-active Toggle immediately as part of the switch — force-releasing each one's tracked held keys via the injector, using the mechanism from ticket 17 — before the new Profile's state becomes active.
- [ ] `ActiveProfileChanged(name: s)` fires correctly on every switch.
- [ ] The GUI's left-hand Profile sidebar (from the prototype) is wired to real `CreateProfile`/`DeleteProfile`/`RenameProfile`/`SwitchProfile` and `ActiveProfileChanged`.
- [ ] The tray mock's "Quick switch" popover lists real Profiles and calls `SwitchProfile`.
- [ ] Live demo: create a second Profile, bind a different set of keys in it, switch between the two Profiles via both the GUI sidebar and the tray quick-switch, and confirm a Toggle left running in the first Profile is force-stopped (no stuck keys, no continued looping) the instant you switch away from it.
- [ ] Automated tests cover: independent Binding sets per Profile, `SwitchProfile` killing all active Toggles with exact-key release, and Profile CRUD persisting correctly to `config.toml`.
