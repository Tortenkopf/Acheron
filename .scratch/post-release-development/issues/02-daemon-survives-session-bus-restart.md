<!--
SPDX-License-Identifier: GPL-3.0-or-later
Copyright © 2026 Justin Milatz
-->

# 02 — Daemon reports healthy but silently drops off D-Bus after a session bus restart

**What to build:** When the user's session D-Bus (`dbus.service`) restarts while
the Daemon is running, the Daemon must not be left `active (running)` with no
D-Bus surface. Today the zbus connection is established once at startup and never
re-established; a bus restart severs it and drops the `com.acheron.Daemon` name
for good. The Daemon process stays up because `main` only treats the capture,
injector, and dispatch tasks exiting as fatal — a dead bus connection stops none
of them — so `systemctl --user status` looks healthy while every GUI call fails
with `ServiceUnknown` and the bus has no activation file to recover from.

Fix via the systemd unit: order the Daemon after `dbus.service` and bind its
lifecycle to it so that a bus restart restarts the Daemon, which then re-requests
its name cleanly on the new bus. Keep `Restart=no` from ticket 57 — this is
stop/restart propagation from the bus unit, not failure-restart, so it does not
reintroduce the crash-loop that ticket guarded against. `install.sh` must emit
the updated unit and stay idempotent, and `packaging/test_install.sh` must cover
the new unit content.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Restarting the session D-Bus while the Daemon runs results in the Daemon
      owning `com.acheron.Daemon` again shortly after, with no manual
      intervention.
- [ ] The GUI, launched after such a restart, reaches the Daemon normally.
- [ ] The systemd unit orders itself after the session bus and propagates the
      bus unit's stop/restart; `Restart=no` is unchanged.
- [ ] `install.sh` writes the updated unit, is safe to re-run, and
      `packaging/test_install.sh` asserts the new unit content.
- [ ] Packaging, Daemon, and GUI test suites stay green.
