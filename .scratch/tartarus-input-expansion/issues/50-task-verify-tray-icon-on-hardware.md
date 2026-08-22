Type: task
Status: resolved

## Question

Finish live-verifying [Build the real system tray icon](./36-task-build-tray-icon.md)'s remaining
checklist items against the real GNOME Shell panel — the items its own build/verify session didn't
reach. **Read the safety context below before doing this at the machine.**

Remaining checklist:

- Click "Show Window" from the tray menu and confirm it raises the hidden main window.
- Physically unplug/replug the Tartarus Pro and confirm the icon transitions through
  `acheron-running-disconnected` (orange) correctly, alongside the already-verified green/red.
- Click "Quit" and confirm it exits the GUI process while leaving `acheron-daemon.service` running.
- Close the main window via its titlebar close button (not the tray's Quit) and confirm it hides
  rather than quitting — the tray icon should still be present and the GUI process still running
  afterward.
- Open the "Switch Profile" submenu by hovering/clicking it in the real panel (not by a scripted
  D-Bus `Event`/`GetLayout` call) and confirm it displays correctly.

**Safety context — read this first**: ticket 36's live-verification session hit two full-system
hard freezes on this machine, both requiring a manual reboot/power cycle. The first followed a
scripted `Event` "Show Window" D-Bus call shortly after the tray icon first registered (coinciding
with repeated real "icon lookup failed" errors from GNOME Shell — a genuine bug, since fixed: see
ticket 36's Answer). The second happened when the user hovered the real "Switch Profile" submenu
open in the actual panel — notably *not* one of the assistant's scripted D-Bus calls; the same
Switch Profile action, driven programmatically via `Event`, had already round-tripped cleanly
moments earlier. Both freezes left no crash/panic/OOM signature in `journalctl` (system or user,
both boots) — the log simply stops, consistent with a hard freeze rather than an application
crash.

Code review of the D-Bus menu implementation (`tray.py`'s `_DBusMenuService`, `tray_menu.py`'s
`MenuModel`) turned up nothing that blocks or reenters — `Event`/`GetLayout`/`AboutToShow` are all
small, synchronous, non-blocking. The more likely explanation is environmental, not a bug in this
ticket's code: this machine is an NVIDIA+AMD hybrid-GPU laptop running GNOME Shell 50.1/mutter on
Wayland — a combination with a well-documented history of unrelated hard freezes when a new surface
renders (mutter#1310, mutter#2924, mutter#1891, Ubuntu bug #1970043 — all NVIDIA/AMD-hybrid+Wayland
mutter freezes, though none is an exact match for a tray-icon/popup trigger specifically). This
isn't proven; treat it as the leading theory, not a confirmed root cause.

Given that, whoever does this ticket should:

- Do it in person, not by asking an unattended agent to script it — same reasoning ticket 26/42's
  precedent already established for physical-device actions, extended here to "anything that
  renders a new popup/texture in this desktop session."
- Save any open work first, same as before any operation with real freeze risk.
- Go one item at a time; if anything freezes, that itself is useful data — note which specific
  action preceded it before rebooting.

## Answer

The user started the GUI manually and drove the full remaining checklist themselves (Show Window,
device unplug/replug's orange state, Quit, window-close-hides, and opening the Switch Profile
submenu live in the panel) — all worked as expected, no bugs found. Ticket 36's build is now fully
live-hardware-verified.

Root cause of the second freeze also settled, from the user's own read of the machine's history:
it wasn't a hang (screen frozen, unresponsive) like the first one — it was a black screen followed
by an automatic reboot, a pattern the user has independently seen before after suspending this
NVIDIA-Optimus+Wayland machine. Likely aggravated by the first incident's unclean shutdown leaving
the session in a bad state going into the second. Confirms the leading theory in ticket 36's
Answer: an environmental GPU/compositor issue on this specific machine, unrelated to the tray
icon's D-Bus/menu code, which held up under direct code review and now under full live use.

