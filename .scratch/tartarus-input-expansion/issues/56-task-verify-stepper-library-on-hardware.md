Type: task
Blocked by: 55
Status: resolved

## Question

Live-verify [ticket 55](./55-task-build-stepper-library-gui.md)'s Stepper library GUI against the real Daemon and Tartarus Pro — split off [ticket 41](./41-task-build-stepper-macro-library-ux.md), matching this map's discipline that a ticket resolves only once actually tested against the real, connected hardware, and matching the task/verify pairing precedent set by tickets 42/44 and 48/49.

Checklist:

- [ ] Create a new Stepper list, add/reorder/remove items using the real key/mouse-button picker.
- [ ] Assign the list's forward/backward Bindings to the scroll wheel's Up/Down Inputs (the primary intended use case), confirm each notch advances/retreats the cursor and fires the newly-selected item in one motion.
- [ ] Confirm wrap-around at both ends of the list.
- [ ] Confirm Hold-to-repeat fast-advance works via the existing repeat machinery, and that Toggle is correctly unavailable/rejected for a Stepper Binding.
- [ ] Assign a second Input pair to the same list; confirm it silently steals the pair from the first assignment and the GUI surfaces the "Moved off '<name>'" toast.
- [ ] Restart the Daemon; confirm the cursor resets to the list's first item (never persisted).
- [ ] Delete a Stepper list; confirm no delete gate blocks it even while assigned (ticket 03/31's settled no-gate behavior).

Fix any real bugs found live before considering this resolved, per this map's standing discipline.

## Answer

**Correction, found immediately after this ticket was first marked resolved**: not actually a clean pass — see the end of this Answer. Before starting, found and fixed a real live-infra gap the ticket itself doesn't cover: the installed `~/.local/bin/acheron-daemon` binary predated [ticket 54](./54-task-land-stepper-library-daemon.md)'s commit entirely (built 00:37, ticket 54 landed 01:35), so the live daemon had no Stepper support at all going into this session — not a config problem like ticket 53's, a stale-binary one. Rebuilt via `cargo build --release`, reinstalled, restarted `acheron-daemon.service` (udev rule and systemd unit were already byte-identical to source, so the `sudo` step in `install.sh` wasn't needed); confirmed the fresh binary via a live `GetConfig()` D-Bus call reporting a `steppers` map before any hardware testing began.

All seven checklist items then live-verified by the user directly against the real Daemon/Tartarus Pro/GUI:

- Create/add/reorder/remove list items via the real key/mouse-button picker — confirmed.
- Forward/backward wired to the scroll wheel's Up/Down — each notch advances/retreats the cursor and fires the newly-selected item in one motion, confirmed.
- Wrap-around at both ends — confirmed.
- Hold-to-repeat fast-advance via the existing repeat machinery, Toggle rejected for a Stepper Binding — confirmed.
- The cross-list steal case — confirmed, but needed unpacking mid-session since the checklist's own wording ("assign a second Input pair to the same list") doesn't match what ticket 55 actually built. The toast only fires when a *different* list's Input is stolen (`library_view.py`'s `build_stepper_assignment_row`, `existing.get("stepper_id") != stepper_id`), not when the same list is simply reassigned to a new pair (that path is silent by design, handled server-side via `take_stepper_direction_elsewhere`). Verified with two lists: assigning list B's Forward onto list A's existing wheel-up Input succeeded silently on the Daemon side and produced `Moved off 'A' (it no longer has an assigned pair)` in the GUI.
- Restart the Daemon — cursor resets to the list's first item, confirmed never persisted.
- Delete a Stepper list — the checklist's literal "no delete gate" premise is superseded by [ticket 55](./55-task-build-stepper-library-gui.md)'s own resolution (the user directed Stepper delete to match Macro's gated-delete UX exactly, mid-ticket-55, overriding ticket 03/31's original no-gate design). What was actually confirmed live is the *current* behavior: delete is disabled with a "Used by N Binding(s)" tooltip while referenced, and succeeds once cleared — matching Macro's precedent exactly, no bug found.

Ticket 55's build itself is fully live-hardware-verified.

**Real bug found and fixed, mid-Toggle-rejection check**: saving a Stepper Binding with Trigger mode set to Toggle correctly failed (no save), but instead of the Daemon's real rejection message the GUI's error banner showed the literal text `"True"`. Root cause was general, not Stepper-specific: `gui/acheron_gui/daemon_client.py`'s `_translate_error()` built every Daemon-error exception as `exc_type(Gio.DBusError.strip_remote_error(err))` — but `strip_remote_error` returns a **bool** (whether it stripped the D-Bus name prefix), not the stripped string, and its in-place mutation of `err.message` doesn't survive PyGObject's `GLib.Error` wrapper either. So *every* real Daemon rejection, across every D-Bus method, has always rendered as `"True"` in the GUI — confirmed by reproducing it live against a Macro-unknown-`macro_id` rejection and a Profile-Switch/non-Fire-once rejection too, not just Stepper/Toggle. Invisible to the 235-test GUI suite because `daemon_client.py` is the one module with zero test coverage (it needs a real bus; `daemon_stub.py` is the tested fake stand-in), and no prior live-verification session had apparently read the literal error text rather than just confirming *an* error was raised.

Fixed by parsing the known `"GDBus.Error:<name>: "` prefix off `err.message` directly instead of relying on `strip_remote_error`. Verified against the real Daemon: Stepper/Toggle, Macro/unknown-id, and Profile-Switch/non-Fire-once rejections all now show their real messages. Attempted a regression test for `_translate_error` (a synthetic `GLib.Error`, no real bus needed) but the mock behaved inconsistently across Python environments — worked standalone, failed under the project's `gui/.venv` — so it was dropped rather than land something flaky; this module stays in the "only live/manual verification can confirm it" bucket its own missing-test-coverage precedent already implies. Full suite still green (235 passed) since this module isn't part of it. GUI restarted against the fix and re-confirmed live by the user.
