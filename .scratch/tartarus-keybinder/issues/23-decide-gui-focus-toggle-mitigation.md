Type: grilling
Status: resolved

## Question

Ticket 22 root-caused and then deliberately parked a GUI freeze: any Python+GTK4 window on this machine freezes while it has keyboard focus and the Daemon has an active Toggle (a Macro looping, or a Keypress held down). The decision there was to make no Daemon-side change, since the Daemon holding a Toggle key raw-down is its documented, correct behavior for a Toggle's real use case (e.g. holding a movement key while some *other* application has focus) — cycling that hold to dodge the race, as a *global* change, wasn't judged worth the tradeoff for a narrow edge case.

This ticket asks a narrower, adjacent question raised afterward: rather than changing the Daemon's Toggle behavior globally, could the *GUI* defensively stop all active Toggles specifically while its own window has focus — i.e. only in the moment the user is looking at the Acheron GUI itself, not while any other application (including whatever the Toggle is meant to affect) has focus? Two shapes were proposed:

- **(a)** On some risky action (e.g. right before opening a grid button's Binding-editor popover), tell the Daemon to stop all Toggles and wait a few ms before proceeding, as a precaution.
- **(b)** Stop all running Toggles as soon as the GUI window gains focus.

## What's already known (don't re-derive — read ticket 22 first)

- The freeze's precondition is just: (1) the window has keyboard focus, (2) a Toggle is actively running. **No focus transition is required** — ticket 22 explicitly disproved that: starting a Toggle while the window is *already* focused reproduces the freeze too, confirmed live and repeatedly (including on a bare-window minimal repro with no Acheron widget tree at all). This directly undercuts shape (b) if implemented naively as a one-shot `notify::is-active` hook: that only fires when focus is *gained*, so it would miss "GUI already has focus, user starts a Toggle from the hardware afterward" — a case ticket 22 specifically confirmed reproduces.
- The freeze self-heals (window becomes responsive again) as soon as the Toggle stops — **except** if a popover/menu was opened (or attempted) while the window was already frozen, which leaves a separate, non-self-clearing wedge that needs a real process restart. A popover opened and closed *before* a freeze doesn't cause this.
- Given the above, **shape (a) (stop-and-delay only right before opening a menu) only guards the compounding, unrecoverable wedge case** — it does nothing for the base (self-healing) freeze, which doesn't require any popover interaction at all. It's a partial mitigation for the worse failure mode, not a fix for the freeze itself.
- **Shape (b), done right, is more complete**: if no Toggle is ever running while the GUI has focus, neither precondition co-occurs, so neither the base freeze nor the compounding popover-wedge case can happen at all. But per the point above, it needs to cover both directions — window gains focus while a Toggle is already running, *and* a Toggle starts while the window already has focus — not just a one-shot focus-in hook.

## What it would take to close the "already focused" gap (shape b, done completely)

- `com.acheron.Daemon` has no bulk "stop all Toggles" method today. The only existing precedent is `SwitchProfile`'s side effect (`daemon/src/dispatch.rs`, the `for (_, toggle) in toggles.drain() { toggle.stop().await; }` loop run as part of `Command::SwitchProfile`) — a new `Command::StopAllToggles` + `com.acheron.Daemon.StopAllToggles()` method would factor out the same drain-loop. No changes needed inside `executor.rs`'s `run_toggle_loop`/`ActiveToggle` itself — cancellation + force-release is already what `.stop().await` does per-Toggle.
- `active_toggles_changed` (`daemon/src/dbus/mod.rs`) is **defined but not wired to fire anywhere yet** (its own doc comment says so). Closing the "already-focused, Toggle starts later" gap needs this signal actually emitted on every Toggle start/stop, plus the GUI subscribing to it (same pattern as `subscribe_layer_changed`/`subscribe_profile_changed` in `app.py`) and — whenever its own window `is_active` — immediately calling the new `StopAllToggles` back in response. Net effect: "no Toggle survives being started while the Acheron GUI has focus," enforced from both directions (focus arriving, and a Toggle arriving), not just one.
- `app.py` has no focus-tracking at all today (confirmed: no `notify::is-active`/`Gtk.EventControllerFocus` wired) — shape (b) needs this added regardless of which direction(s) end up covered.
- `gui/acheron_gui/daemon_client.py`'s `DaemonClient`/`DBusDaemonClient` would need a new no-arg `stop_all_toggles()`, following `switch_profile`'s call shape (`self._call("StopAllToggles", None)`).

## The open tension worth grilling, not assuming

- `spec.md`'s "Toggle behavior across Layer/Profile switches" section currently documents exactly two ways a Toggle stops: pressing the same physical key again, or a Profile switch. Shape (b) adds a **third**, novel one — "the GUI having focus" — which isn't just an implementation detail; it's a behavior change to when a user's Macro/held-key stops, the same category of decision ticket 04 (`04-decide-toggle-behavior-across-switches.md`) already made deliberately and explicitly. It would need the same kind of `spec.md` update if adopted, not a silent addition.
- `spec.md`'s "Out of Scope" section already rules out one adjacent kind of automatic, focus-based behavior — "Automatic, focused-application-based Profile switching — Profile switching is always manual" — for a different feature (Profile switching), but it signals the project has previously preferred manual, predictable behavior over implicit focus-based automation. Shape (b) is arguably different in kind (a safety/defensive measure to protect the GUI's own usability, not a convenience automation like auto-switching Profiles), but that distinction is worth stating explicitly and getting agreement on, not assumed.
- This also directly revisits ticket 22's own "Decision (2026-08-15)" to leave the Daemon's raw-hold behavior untouched for this "narrow edge case." Worth being explicit that shape (b) does **not** change that global behavior — a Toggle still holds a key down for as long as some *other* window has focus, exactly as documented — it only adds a GUI-focus-scoped exception. That's a materially narrower and different change than the Daemon-side mitigation ticket 22 already declined, which is why it's being raised as a fresh question rather than treated as already answered by that decision.

## Suggested next step

A short grilling session to settle: is shape (b) (with both directions wired) worth doing given the `spec.md` documentation change it requires, or does the user still prefer to leave ticket 22 exactly as decided? If yes, this ticket's `## Answer` should record the exact mechanism (both-directions `StopAllToggles` as scoped above) so an implementation ticket can be split off cleanly, and note the required `spec.md`/`CONTEXT.md` addition to the Toggle-stop-conditions list.

## Answer

Grilling session, 2026-08-15.

**Build something — but not shape (b) as originally sketched.** The user reframed the requirement mid-session, and it changed the design:

> "The GUI should make sure that none of the daemon's output ever reaches it while its window has focus."

Two things drove this, beyond the ticket 22 freeze itself: (1) the GUI is definitionally more likely to hit this bug than any other GTK app on the machine, since its whole purpose is to sit next to a Daemon that emits synthetic input; (2) some of the GUI's own popovers (the Binding editor) contain text entry fields — a running Macro/Toggle's output landing in one of those while it's focused isn't just a freeze risk, it's silent data corruption (stray characters typed into a field the user is editing).

**Decision: suppress, not stop.**

- Reject the ticket's original `StopAllToggles`/`active_toggles_changed` design. Instead: the injector task in `executor.rs` (the single task that already serializes every `uinput` write, per `spec.md`'s "Daemon event loop and concurrency" section) gates writes behind a "GUI is focused" flag. Firing logic, Macro looping, and Toggle's "on" state are all unaffected — only the actual write to the virtual device is withheld while the flag is set.
- **This does not touch ticket 04's Toggle-stop-condition model at all.** A Toggle never stops because of GUI focus — it keeps running internally and resumes emitting the instant focus leaves the GUI window. `spec.md`'s "Toggle behavior across Layer/Profile switches" list (same-key press, Profile switch) gets no third entry. This directly resolves the "is this a new stop condition in ticket 04's category" tension this ticket raised — it isn't one.
- It's also distinct in kind from the Out-of-Scope "no automatic, focused-application-based Profile switching" rule: that rule bans implicit *convenience* automation over which Profile is active. This suppression never touches Profile/Layer state — it's an I/O-hygiene safety guard scoped purely to output delivery.
- **Scope: all Daemon output**, not just Toggle — Fire-once and Hold-to-repeat are suppressed too while the GUI is focused. The injector doesn't distinguish Trigger mode at the write level, so this costs nothing extra and matches the user's stated requirement exactly (a single stray Fire-once character landing in a focused text field is the same class of bug as a Toggle flooding one).
- **Trigger shape: level-triggered**, not edge-triggered. The flag reflects the GUI's *current* focus state, not a one-shot "focus gained" event — this is what closes the "GUI already focused, output starts afterward" gap ticket 22 confirmed live, without needing two separately-wired signal directions the way the original `active_toggles_changed`-based sketch would have.
- **Disconnect safety (required, not optional):** the suppression flag must auto-clear if the GUI disappears while it's set — crash, `kill -9`, any ungraceful exit — via the existing `zbus`/`com.acheron.Daemon` connection's peer-disconnect detection, in addition to an explicit focus-out call. Without this, "suppress" would trade ticket 22's rare, self-healing, single-window freeze for a rare-but-total, silent, whole-device output outage with no on-screen way to explain why (the GUI that would show the problem is the thing that's gone) — strictly worse than the bug being fixed.

**Documentation implication:** this needs a new `spec.md`/`CONTEXT.md` section describing Daemon-output suppression while the GUI has focus — separate from, and not an amendment to, the Toggle-stop-conditions list.

**Next step:** implementation ticket split off (see ticket 24 and ticket 25).

**2026-08-15 (cross-reference) — the "suppress, not stop" decision above was partially revised during ticket 25's live-hardware verification.** Nothing here is wrong as a description of what `SetOutputSuppressed` itself does — it still never stops a Toggle, only gates output, exactly as decided above. What changed: live testing found pure suppression cannot prevent ticket 22's freeze when a Toggle is already running *before* the GUI gains focus, since the freeze's precondition (a real, OS-armed held key) is already set by the time suppression gets a chance to act — suppression only gates *future* writes. The user decided live to add a second, separate, one-directional mechanism on top: the GUI now also force-stops every running Toggle on its own window's focus-*gain* (`StopAllToggles`, never resumed on focus-loss) — narrower than this ticket's originally-declined "shape (b)," and layered alongside suppression rather than replacing it. See ticket 25's comments for the full live-testing narrative and root cause.
