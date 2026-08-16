Label: wayfinder:map

## Destination

Acheron reaches **v1.0**: feature-complete and released as a public, open source Linux tool for the Linux gaming community — still scoped to the Razer Tartarus Pro specifically (the only hardware owned/tested), but built without gratuitous obstacles to someone later adapting it for other Tartarus variants.

- **Feature-complete** means rough parity with Razer Synapse's remap/macro feature set for this device, minus cloud sync and lighting (already out of scope), plus deliberate extras already underway (Stepper). The v1.0 feature list is locked (see [Lock the v1.0 feature list](./issues/08-decide-v1-feature-list.md)):
  - **Required floor**: the shipped MVP (Profiles, Layers, Keypress/Macro Actions, Fire-once/Hold-to-repeat/Toggle Trigger modes) + **Chord** Bindings + **mouse-button/full-keyboard output** with a GUI picker (now widened to include multimedia/consumer-control keys) + **Stepper** + **Profile Switch** + two polish tickets (context-menu cleanup, GUI sizing) + **reusable/named Macro entities** (a macro library, replacing today's inline-only Macro).
  - **Non-blocking, welcomed if ready in time**: two open-ended R&D strands — **grid-key analog capture** (a genuine Linux-ecosystem gap, feasibility TBD, may be dropped if infeasible) and **Controller/Joystick output emulation** (userspace-emulated, explicitly not a kernel-level virtual device). Neither gates v1.0: the destination is reached once the required floor's frontier is empty, whether or not these two have landed; either fast-follows into a later release if it isn't ready.
- **Release-ready** means a clean, final git repo, built and installed from source (`install.sh`, no deeper packaging for v1.0 — the Daemon runs unprivileged and the GUI is pure userspace, so nothing forces packaging complexity yet) with documentation sufficient for a stranger to build, install, and use it, under a chosen open source license.

Like the previous map, this one carries execution.

## Notes

**This map carries execution** — resolving a ticket means actually building and testing against the real, connected Tartarus Pro, not only deciding (same discipline as the previous map). This is also the de facto quality bar for "feature-complete": every ticket's own live-hardware verification plus the existing daemon/GUI test suites (72+28 tests as of the last MVP ticket) *are* the stability bar — no separate quality-bar ticket exists or is needed.

**Grounding facts found before charting this map:**
- `Action::Keypress`'s `key` field is already a bare `evdev::KeyCode`, not a curated keyboard-only enum — nothing in the Daemon validates it against a keyboard allow-list.
- The virtual `uinput` device (`daemon/src/injector.rs::build_device`) already declares the *entire* standard `EV_KEY` range via `input::all_injectable_key_codes()`, and `BTN_*` codes (`BTN_LEFT`, `BTN_RIGHT`, `BTN_MIDDLE`, …) share the same numeric space as `KEY_*` in evdev — they're already advertised.
- evdev's `KeyCode::FromStr`/`Display` (the `evdev_enum!` macro) parses `"BTN_LEFT"` the same way it parses `"KEY_A"` — so `config.toml` can already represent a mouse-button target with zero Daemon changes.
- The GUI's key field (`gui/acheron_gui/binding_editor.py`) is a bare `Gtk.Entry` — no picker, no validation. This is the actual gap for mouse-button support, not the Daemon.
- Given the above, mouse-button *output* likely already works Daemon-side and mostly needs verification + a GUI picker, not new Daemon capability — confirm this empirically rather than assuming, since untested claims from reading code have been wrong before on this project (see the `ResetFailed`/`ResetFailedUnit` correction on the last map).

**Terminology (settled during charting, see CONTEXT.md):**
- **Chord** is reserved for the new simultaneous-Input concept. Keypress's existing modifier combination (Ctrl+Shift+T) is now called exactly that — "a modifier combination" — not "a chord," to avoid collision. CONTEXT.md's Keypress entry has been updated; full Chord/Stepper glossary entries are deliberately *not* added yet — each is a bare reserved name until its own ticket below settles the actual model, per domain-modeling's "update lazily, only when resolved" discipline.

**Skills to consult**: default to `/grilling` and `/domain-modeling` for decision tickets. `/prototype` is likely warranted for the mouse-button/key GUI picker and the Stepper list-editing UX — each ticket's own grilling session should decide whether to spin one up, per the "how should it look/behave" test.

**Scope boundary volunteered directly by the user, not re-litigated in a ticket**: mouse-wheel motion is *not* an output Action — the Tartarus Pro's own wheel already passes through scroll natively, so nothing needs to synthesize scroll output. Mouse-button output is clicks only, never cursor movement.

**Standing discipline — keep the door open for other Tartarus variants**: this map's destination stays scoped to the Tartarus Pro only (no other hardware to test against), but avoid casually adding new hardcoded Tartarus-Pro-only assumptions while building the remaining features where a small amount of care would keep the door open for someone else to fork/adapt for other Tartarus variants later (V1/Chroma). Mirrors the previous map's "keep the capture layer swappable" discipline for analog input — a light habit to hold, not a reason to build or audit anything now. No ticket exists for this and none is expected before v1.0.

## Decisions so far

- [Determine GNOME/Wayland-specific assumptions](./issues/10-research-de-display-server-compatibility.md) — the Daemon is DE/display-server-agnostic by construction (kernel evdev/uinput + system D-Bus only, no compositor in the path); the GUI's real tray icon doesn't exist yet (only an in-window mock), but the already-decided design (`AppIndicator3`/`AyatanaAppIndicator3`) is itself the portable choice — it needs the `ubuntu-appindicators` extension only under GNOME specifically, and works natively on KDE/XFCE. Spawned [Decide the tray icon's look and behavior](./issues/11-decide-tray-icon-look-and-behavior.md) — the library choice is settled, but its look/feel and menu behavior are not, so it's a design ticket first, not a direct build.
- [Lock the v1.0 feature list](./issues/08-decide-v1-feature-list.md) — settled against the Synapse catalog. Required additions: reusable/named Macro entities (new ticket). Non-blocking additions: grid-key analog capture, Controller/Joystick emulation (both new tickets, open-ended). Corrections folded into existing tickets: Chord gets a thumbstick-diagonal worked example (ticket 01); the key/mouse-button picker widens to multimedia keys + a canned-text note (ticket 02). Ruled out: Macro shell commands, Launch-a-program, an intrinsic Macro loop primitive.

## Not yet specified

- **Composition between Chord/mouse-button/Stepper/Profile-Switch** — e.g. can a Chord's Action be a Stepper step; can a Stepper's forward/backward pair include a Chord as one side. Not sharp enough to ticket until those tickets have settled their own shape.
- **Design/prototype/build tickets** for each of Chord, mouse-button-output, Stepper, Profile Switch, and reusable Macro entities — expected to graduate once their respective grilling ticket resolves (decide first, then design/prototype, then build against real hardware).
- **Grid-key analog trigger points** (the actual feature, beyond raw signal feasibility) — fog past [Standalone analog-capture prototype](./issues/13-task-standalone-analog-capture-prototype.md); not specifiable until that prototype confirms the signal is reachable from Linux at all.
- **Controller/Joystick research and prototype tickets** — fog past [Design Controller/Joystick output emulation](./issues/14-decide-controller-joystick-output-emulation.md); expected to graduate once that grilling session narrows the shape (device advertising, axis behavior, GUI picker).
- **Release documentation** (README, install instructions, CONTRIBUTING, etc.) — content depends on both the final feature list ([Lock the v1.0 feature list](./issues/08-decide-v1-feature-list.md), now settled) and the chosen license ([Choose an open source license](./issues/09-decide-open-source-license.md)); not sharp enough to ticket until the license lands.
- **What ships in the public repo** — `.scratch/`, `prototype/`, `docs/adr/`, and `CONTEXT.md` are all currently assumed to ship as-is (they're a legible build record, not sensitive), but the user flagged a concern that this much process detail might overwhelm a user who just wants to game. Deliberately deferred rather than decided now; revisit closer to release.

## Out of scope

- Mouse cursor movement (pointer X/Y motion) as an output Action — mouse-button support is clicks only.
- A synthetic mouse-scroll-wheel output Action — the Tartarus Pro's own wheel already passes through scroll natively (see Notes).
- Capturing input from a real external mouse device — Acheron's capture surface remains the Tartarus Pro's three evdev nodes only; "mouse buttons" here is output-side exclusively.
- Distro packaging (AUR, `.deb`, Flatpak, etc.) for v1.0 — release means a clean git repo built from source via `install.sh`; packaging is a plausible fast-follow if the tool ever needs deeper system integration, not a v1.0 requirement.
- Distribution/promotion after release (posting to r/linux_gaming, submitting to package repos, etc.) — this map is about the tool being releasable, not about promoting it afterward.
- Generalizing hardware support to other Tartarus variants or other devices — Tartarus Pro is the only hardware owned and tested; see Notes for the lighter "keep the door open" discipline this map does carry.
- A capstone "cut v1.0" ticket — reaching v1.0 is this map's frontier going empty, not a decision to ticket.
- Shell-command execution as a Macro action (surfaced by the Synapse catalog) — security-sensitive, no articulated use case; see [Lock the v1.0 feature list](./issues/08-decide-v1-feature-list.md).
- A Launch-a-program Action (surfaced by the Synapse catalog) — judged not worth having; see [Lock the v1.0 feature list](./issues/08-decide-v1-feature-list.md).
- An intrinsic Macro loop/repeat-forever primitive distinct from Toggle (Synapse's "loop forever after an optional setup sequence") — a confirmed but narrow gap; Toggle already covers looping for the general case. See [Lock the v1.0 feature list](./issues/08-decide-v1-feature-list.md).
