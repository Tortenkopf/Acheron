Type: task
Status: resolved

## Question

On a cold boot, `acheron-daemon.service` reliably fails its *first* start attempt with
`Error: Os { code: 13, kind: PermissionDenied, message: "Permission denied" }`, then succeeds
~1-2s later on systemd's automatic restart. Fix the startup ordering so the daemon's first
attempt succeeds instead of relying on `Restart=on-failure` to paper over the race.

Observed identically on two separate cold boots, both with the physical Tartarus Pro already
connected at boot: 2026-08-15 20:06:39-44 and 2026-08-18 21:15:07-09. Same shape both times:

```
systemd[...]: Started acheron-daemon.service - Acheron Daemon — Tartarus Pro keybinding/macro engine.
acheron-daemon[...]: Error: Os { code: 13, kind: PermissionDenied, message: "Permission denied" }
systemd[...]: acheron-daemon.service: Main process exited, code=exited, status=1/FAILURE
systemd[...]: acheron-daemon.service: Failed with result 'exit-code'.
systemd[...]: acheron-daemon.service: Scheduled restart job, restart counter is at 1.
systemd[...]: Started acheron-daemon.service - Acheron Daemon — Tartarus Pro keybinding/macro engine.
kernel: input: Acheron Virtual Tartarus Pro as /devices/virtual/input/input39
```

The likely cause: `packaging/acheron-daemon.service` only orders itself against
`After=graphical-session.target` — nothing orders it against udev having actually finished
applying `packaging/60-acheron-tartarus-pro.rules` (`MODE="0660"`/`GROUP="plugdev"` on the
Tartarus Pro's `hidraw` nodes) to the device node the daemon opens on startup. On a cold boot
the graphical session can reach `graphical-session.target` before udev has processed that
device's add event, so the daemon's first open attempt loses the race and hits
`PermissionDenied`; `RestartSec=1` happens to be enough for udev to catch up by the second
attempt, which is why this currently self-heals rather than sticking.

This is currently harmless — the daemon recovers every time and the map's Notes already
document `PermissionDenied` as something the capture layer's absence-retry bucket treats like
"not yet plugged in," not a hard failure — but it's a real, reproducible bug: a cold boot
should not require a failed attempt and a systemd-scheduled restart to reach a working state.

Investigate and fix the ordering (candidates: `After=`/`Wants=` on a udev-settle target,
`ExecStartPre=udevadm settle`, or a udev `TAG+="systemd"` binding on
`60-acheron-tartarus-pro.rules` so the unit only starts once the device is actually ready —
check what's actually available and effective on this system's systemd/udev version rather than
assuming) so the first start attempt succeeds cleanly. Verify by rebooting cold with the
Tartarus Pro connected and confirming the journal shows exactly one successful
`acheron-daemon.service` start, no `PermissionDenied`, no restart — needs a real reboot, can't
be verified by `systemctl --user restart` alone since that doesn't reproduce the boot-time race
window.

## Answer

**The ticket's own working hypothesis was wrong, corrected by checking this system's real
systemd/udev instead of assuming.** It is not a race against `60-acheron-tartarus-pro.rules`
being applied to the Tartarus's own `hidraw` nodes — the capture layer already treats a missing
grant there as soft/retryable absence (`capture/analog.rs`'s existing `PermissionDenied`
handling), so that path was never the one crashing the process. The actual failure is
`injector::build_device()` (`main.rs`, called before capture/dispatch/D-Bus exist at all) opening
`/dev/uinput`, whose access comes from an entirely different mechanism: `udev`'s `TAG+="uaccess"`
on `KERNEL=="uinput"` (`/usr/lib/udev/rules.d/60-steam-input.rules` on this system — Steam's rule,
not ours), which grants a per-user ACL (`getfacl /dev/uinput` showed `user:justin:rw-`) applied
when the login session becomes *active*. That activation isn't ordered against
`graphical-session.target`, so the daemon's very first open of `/dev/uinput` in a fresh session
can lose the race and hit `PermissionDenied` before propagating unhandled straight out of
`main()` — matching the bare, contextless `Error: Os { code: 13, ... }` in the journal (no
"task exited with a fatal error" prefix, so it's not coming from any of the three supervised
tasks). `journalctl --user -u acheron-daemon.service` also shows this recurring on the first
start after several different `systemd --user` manager PIDs (i.e. every fresh login session),
not only literal cold boots — consistent with a session-activation race, not a boot-order race
against one specific rules file.

Given that, none of the ticket's three candidate systemd/udev-ordering fixes
(`After=`/`Wants=` on a udev-settle target, `ExecStartPre=udevadm settle`, `TAG+="systemd"` on
our rule) would reliably have fixed this — none of them touch `/dev/uinput`'s ACL timing, and
this system's `systemd-udev-settle.service` is `static` (not pulled into the boot by default,
and cross-boundary ordering from a `--user` unit onto it isn't straightforward anyway). No
changes were made to `packaging/acheron-daemon.service`, the udev rule, or `install.sh`.

**Fix, in code instead**: added `injector::retry_on_permission_denied` — generic over the
open call's return type so the retry/backoff decision is unit-testable without real hardware
(three new tests: succeeds once the fake open stops failing, gives up after the attempt bound
and preserves the original error, does not retry a non-`PermissionDenied` error kind) — and
wired `main()`'s `injector::build_device()` call through it: 25 attempts, 200ms apart, ~5s
bounded total. Mirrors the codebase's existing precedent (`capture/analog.rs`,
`capture/evdev_source.rs`'s absence-retry bucket) of treating a startup `PermissionDenied` as
soft/retryable rather than fatal, just applied to the one call site that wasn't already doing
that. Bounded rather than unbounded so a genuine misconfiguration (not just a slow ACL) still
surfaces and lets the existing `Restart=on-failure` safety net recover it, instead of hanging
the unit's start indefinitely. 180 Rust tests green (177 + 3 new).

**Not yet verified live** — per this ticket's own note, a real cold reboot is required and
`systemctl --user restart` cannot substitute (the session is already active and `/dev/uinput`'s
ACL already granted by then, so the race this fix targets can't reproduce that way; restarting
the currently-running daemon on this machine would also interrupt real keybindings for near-zero
verification value). Spawned [Verify the udev-startup-race fix on a real cold reboot]
(./29-task-verify-udev-startup-race-fix-on-hardware.md).
