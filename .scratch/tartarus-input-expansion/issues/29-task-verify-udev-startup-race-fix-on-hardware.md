Type: task
Blocked by: 28
Status: resolved

## Question

[Fix the acheron-daemon udev startup race](./28-task-fix-acheron-daemon-udev-startup-race.md)
added a bounded retry (`injector::retry_on_permission_denied`) around `main()`'s
`/dev/uinput` open, on the theory (confirmed by reading `getfacl /dev/uinput` and this
system's udev rules, not just asserted) that the cold-start `PermissionDenied` is a race
against udev's `uaccess`-tag ACL grant on `/dev/uinput` being applied when the login session
activates — not a race against `60-acheron-tartarus-pro.rules` being applied to the Tartarus's
own `hidraw` nodes, which the capture layer already handled softly. This can't be verified by
`systemctl --user restart` (the session is already active and the ACL already granted by
then, so the race doesn't reproduce) — it needs a real cold reboot, per ticket 28's own
verification note.

Steps:

1. `cd daemon && cargo build --release`, then reinstall the binary (`install.sh` or a manual
   `cp target/release/acheron-daemon ~/.local/bin/`) — no packaging/udev-rule changes this
   time, so the `sudo` udev step in `install.sh` is a no-op either way.
2. Reboot cold with the Tartarus Pro connected.
3. `journalctl --user -u acheron-daemon.service -b` — confirm exactly one `Started
   acheron-daemon.service` line, no `PermissionDenied`, no `Scheduled restart job`. If the
   retry logs its one-time diagnostic (`/dev/uinput not accessible yet ... retrying`), that's
   still a pass — it means the race was hit and self-healed *within* the first attempt, which
   is the actual ask (no failed/restarted unit), not that the race window shrank to zero.
4. Repeat across at least one more cold boot (ticket 28's own investigation used two, on two
   different days, to establish the race as reliably reproducible in the first place).

HITL — needs a real, physical cold reboot of the user's machine, which the agent should not
initiate unasked.

## Answer

**The first attempted verification (boot at 23:02:49) was invalid, not a fix failure**: the
installed `~/.local/bin/acheron-daemon` was stale — built at 22:27, before ticket 28's own
source edits to `injector.rs`/`main.rs` at 22:51 that actually added
`retry_on_permission_denied`. Confirmed via `nm --demangle` and `strings`: the binary running
at that boot had no `retry_on_permission_denied` symbol and no `"not accessible yet"`
diagnostic string at all. That boot reproduced the exact pre-fix failure shape (`PermissionDenied`
14ms after start, process exit, systemd restart) — consistent with unpatched code, not a
disproof of the fix. Rebuilt (`cargo build --release`), confirmed the new binary *does* contain
the retry symbol/string, and installed it via `install` (plain `cp` hit `Text file busy` against
the still-running old process; `install`'s rename-based replace avoided that).

**Two subsequent cold boots against the correctly-installed fix both passed**, matching ticket
28's own two-boot reproducibility bar:

- Boot at 23:08:04 — one `Started acheron-daemon.service` line, retry diagnostic fired once
  (`/dev/uinput not accessible yet (permission denied), retrying`), no `PermissionDenied` crash,
  no `Scheduled restart job`. Service ran continuously afterward.
- Boot at 23:10:54 — identical shape: one `Started` line, one retry-diagnostic line, no crash,
  no restart.

Both match the ticket's own definition of success (step 3): the race is still hit on a fresh
session (confirmed by the retry diagnostic firing), but now self-heals *within* the first
attempt instead of crashing the unit and relying on `Restart=on-failure`. Ticket 28's fix is
live-hardware-verified.
