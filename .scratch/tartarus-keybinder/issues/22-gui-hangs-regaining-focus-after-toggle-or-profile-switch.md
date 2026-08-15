# 22 — GUI hangs regaining focus after a Toggle/Profile-switch interaction

**What to build:** Diagnose and fix a GUI freeze discovered during ticket 19's live hardware demo, in the tray quick-switch path of the multiple-Profiles feature. The Daemon side is confirmed healthy throughout every repro; this is a GUI-process hang.

**Blocked by:** 19

**Status:** ready-for-agent

## Repro

1. Real Daemon + real GUI running against the actual Tartarus Pro (not a `DaemonStub`-backed test).
2. Two Profiles exist (`Default`, `Gaming`), `Gaming` active. `Gaming`'s `grid_r1c2` is bound to a Toggle Macro (`KeyDown(KEY_A)`, `Delay(1000)`).
3. Focus a scratch text field (not the GUI window). Physically press grid key "2" once — the Toggle starts (`a`s repeat into the scratch field, confirmed via a separate `GetState()` call showing `active_toggles: ["grid_r1c2"]`).
4. Switch focus back to the GUI window (before touching the tray's "Quick switch" popover at all) — **the GUI window stops responding to input.**

This reproduced twice, on a freshly-restarted GUI process both times (including after the fix below was applied and both processes were rebuilt/restarted).

## What's already ruled out / fixed along the way

- **The Daemon is not stuck.** Independent `gdbus`/`GetState()` calls against the running Daemon succeeded throughout every hang, including while the GUI was frozen — the single dispatch task that would need to be stuck for a Daemon-side hang was clearly still servicing requests.
- **A real, separate bug was found and fixed in `SwitchProfile`**, though it does not appear to be what's reproducing here (the hang above reproduces before any `SwitchProfile` call is even made — the user hadn't yet opened the tray popover): `dispatch.rs`'s `SwitchProfile` handler used to emit `ActiveProfileChanged` *before* sending the method's own reply. Since `DBusDaemonClient`'s calls are synchronous (`Gio.DBusProxy.call_sync`), a client's own in-flight `SwitchProfile` call could see its own subscribed signal arrive — and run the signal callback — before that same call had returned: a reentrant blocking D-Bus call nested inside another one still unwinding, on the same connection. Fixed by sending the reply first, signal after (`dispatch.rs`, ticket 19's `SwitchProfile` arm). As defense in depth, `app.py`'s `on_layer_changed`/`on_profile_changed` signal callbacks were also changed to defer their `rebuild()` via `GLib.idle_add` rather than calling it inline from within the signal dispatch — this protects against the same class of hazard regardless of ordering on the Daemon side, and is now also the pattern the demo above was tested against (i.e., it still hung with both fixes in place).
- `gdb -p <pid>` is not usable in this sandboxed session (`ptrace_scope` blocks attaching, even as the owning user) — no Python-level stack trace of the hung GUI process was obtainable here. `py-spy` isn't installed either.
- `/proc/<pid>/task/*/wchan` for the hung GUI process showed the main thread in `poll_schedule_timeout` (i.e., idle-waiting in `poll()`/`epoll_wait`) both times — consistent with either a genuinely idle main loop *or* a nested `call_sync` wait-loop stuck waiting for a reply that will never come; `wchan` alone can't distinguish the two.
- No output/errors in either process's log (`stdout`/`stderr` redirected to file) at the time of the hang, both times.

## What to try next

- Get `py-spy dump --pid <pid>` (or run the GUI itself with `python3 -X faulthandler` and send it a signal `faulthandler` is registered against) to get a real Python + native stack of the hung process — the ptrace restriction blocking `gdb` in the session that found this bug may not apply in whatever session picks this up.
- Reproduce outside of a background-job session against a real interactive desktop, to rule out anything specific to this session's display/process-spawning setup (the GUI was launched via `nohup ... &` from a non-interactive shell, not a normal terminal/desktop launcher — worth ruling in/out).
- Given the repro doesn't require touching the tray popover at all, focus first on what a plain window-focus change does in this codebase — GTK4 focus-in/out signals, any `Gio.DBusProxy` re-subscription or `NameOwnerChanged` watching that might fire on window state changes elsewhere in `app.py`/`daemon_client.py` (none is wired yet as of ticket 19, but worth double-checking nothing GTK does implicitly).
- Consider whether an *already-running* Toggle (a background `tokio` task on the Daemon side looping every 1000ms) interacting with the GUI's own D-Bus proxy in some liveness-check/keepalive way could be relevant — this repro's distinguishing factor from every previously-working live demo (tickets 16–18) is specifically that a Toggle is actively running at the moment of the hang.

## Automated tests

None possible yet — this is a real-process/real-focus-event hang not reachable through the `DaemonStub`-backed GUI test seam or the Daemon's own fake-`CaptureSource` seam. Once root-caused, add regression coverage at whichever seam the fix actually lives at.
