# 21 — systemd packaging + launch safety net

**What to build:** The Daemon runs continuously without the user having to launch it by hand — it starts at login via `systemd --user`, and the GUI double-checks it's up (and clears any stuck failure) every time the GUI itself launches. See `.scratch/tartarus-keybinder/spec.md` ("Packaging and lifecycle") for the full design.

**Blocked by:** 20

**Status:** ready-for-agent

- [ ] A `systemd --user` unit at `~/.config/systemd/user/acheron-daemon.service`: `Type=simple`, `ExecStart=%h/.local/bin/acheron-daemon`, `After=graphical-session.target`, `WantedBy=default.target`, `Restart=on-failure`, `RestartSec=1`, `StartLimitIntervalSec=60`, `StartLimitBurst=5`. Stdout/stderr go to the default journal.
- [ ] A small idempotent `install.sh`: builds the release binary, copies it to `~/.local/bin/acheron-daemon`, copies the unit file into place, runs `systemctl --user daemon-reload`, then `systemctl --user enable --now acheron-daemon`. Safe to re-run on every rebuild.
- [ ] On its own launch, the GUI calls `org.freedesktop.systemd1.Manager.ResetFailed("acheron-daemon.service")` then `StartUnit("acheron-daemon.service", "replace")` over the same session D-Bus connection it already holds — no `systemctl` shell-out, no subprocess. `StartUnit` is a no-op if the Daemon is already running.
- [ ] Live demo: run `install.sh`, log out and back in, and confirm (`systemctl --user status acheron-daemon` or the GUI's status chip from ticket 20) the Daemon is already running with no manual start. Separately, `kill -9` the Daemon process to force it into `systemd`'s `failed` state, then launch the GUI, and confirm the Daemon comes back automatically (visible via ticket 20's status chip flipping from not-running to running) with no terminal interaction.
- [ ] Automated coverage is limited to what doesn't require a real systemd user session (e.g. `install.sh`'s idempotency, unit-file content) — the login-autostart and crash-recovery demos above are manual/live-hardware verification, not unit tests.
