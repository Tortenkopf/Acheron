Type: grilling
Status: resolved

## Question

Decide how the Daemon is packaged and run as a systemd service: user unit vs system unit (weighing that `/dev/uinput` and the device nodes already work for this user via the `plugdev` group and an existing ACL, per the map's Notes, with no extra permission setup needed), unit file contents, install location/process, and whether/how it autostarts (login vs GUI launch) and restarts on failure.

## Answer

Grilling session, 2026-08-13.

**Unit scope** — `systemd --user` unit (`~/.config/systemd/user/acheron-daemon.service`), not a system unit. The permission story is already fully solved at the user level (`plugdev` + existing `/dev/uinput` ACL, per the map's Notes) — a system unit would need to re-derive that access for root or a dedicated system user for no benefit, and would need `sudo` to install on what is a single-user personal MVP.

**Unit file** — `Type=simple`, `ExecStart=%h/.local/bin/acheron-daemon`, `After=graphical-session.target`, `WantedBy=default.target` (so `systemctl --user enable` makes it come up at login), `Restart=on-failure`, `RestartSec=1`, `StartLimitIntervalSec=60`, `StartLimitBurst=5`. Stdout/stderr go to the default journal — no separate log file; `journalctl --user -u acheron-daemon -f` covers development and troubleshooting.

**Install process** — a small idempotent `install.sh`: build the release binary, copy it to `~/.local/bin/acheron-daemon`, copy the unit file to `~/.config/systemd/user/`, `systemctl --user daemon-reload`, `systemctl --user enable --now acheron-daemon`. No distro packaging (`.deb`/AUR) — one install target, re-run on every rebuild during development.

**Autostart is two-layered**:
1. **Login-enabled** (`WantedBy=default.target`) — the Daemon comes up with the user session and runs continuously, independent of the GUI, matching CONTEXT.md's definition of the Daemon. This is the primary trigger; D-Bus service-activation was rejected as the primary mechanism because the Daemon's job (evdev capture → uinput inject) doesn't wait on anyone calling a D-Bus method.
2. **GUI-ensures, as a safety net** — on its own launch, the GUI calls `org.freedesktop.systemd1.Manager.ResetFailed("acheron-daemon.service")` then `StartUnit("acheron-daemon.service", "replace")` over the same session D-Bus connection it already holds for the Daemon's own interface (per [Decide D-Bus interface surface](./08-decide-dbus-interface-surface.md)) — no subprocess, no `systemctl` shell-out. `StartUnit` is idempotent (a no-op if already running) so the GUI doesn't need to check state first; `ResetFailed` clears a latched `failed` substate from a genuine crash so the GUI recovers the Daemon without the user touching a terminal.

**Device-not-present is not a restart-policy problem** — this is the correction to [Design Daemon capture/injection event loop](./07-design-daemon-capture-event-loop.md)'s "any capture failure is fatal" model. Booting before the Tartarus Pro is plugged in, or unplugging it mid-run, is an expected, recoverable condition for a desktop peripheral — treating it as a crash and outsourcing recovery to `Restart=on-failure` with a burst limit means the unit lands in `failed` after 5 attempts in 60s and just sits there, which is the opposite of "stay around and pick the device up once it's plugged in." So the `CaptureSource` now splits failure into two classes:
- **Device absent** (nodes don't exist, at startup or after a mid-run unplug): non-fatal. Poll for the known `/dev/input/by-id/...` paths (stable across replugs, per [Enumerate physical inputs](./01-enumerate-physical-inputs.md)) every ~2s until they open cleanly, then resume normal capture. One poll loop handles both the boot-before-plugin and the unplug/replug case — no separate code path. Chosen over udev/netlink hotplug monitoring: a couple seconds of latency plugging in a macro pad is imperceptible, and polling needs no new dependency or unverified non-root netlink-read permissions.
- **Genuine capture errors** (e.g. a `uinput` write failure, an unexpected fd error): still fatal-exit, still deferred to systemd's `Restart=on-failure` — this class is a real bug, not "the device isn't there," and staying fatal keeps a systemd-visible failure signal plus journal history for it.

**Device-presence indicator in the GUI** — this needs a fact only the Daemon has (whether its poll loop currently sees the nodes), so it's a small correction to [Decide D-Bus interface surface](./08-decide-dbus-interface-surface.md): `GetState()` gains a `device_connected: b` field alongside `profile`/`layer`/`active_toggles`, plus a new `DeviceConnectionChanged(connected: b)` signal, same live-push rationale as the other three signals.

**Daemon-presence indicator in the GUI** — live via watching `NameOwnerChanged` for `com.acheron.Daemon` on the session bus, not a one-shot check on window open — the same pattern already used for the Daemon's own state signals, since the Daemon can go away while the window is sitting open (a genuine crash).

No new tickets — this closes out the packaging branch of the map. One item graduates into the map's fog: where the daemon-running/device-connected indicators actually surface in the GUI (header badge vs tray icon state vs both) — a small addition to the Device Overview IA from [Design GUI information architecture](./09-design-gui-information-architecture.md), not sharp enough yet to ticket on its own and out of this ticket's scope to decide.
