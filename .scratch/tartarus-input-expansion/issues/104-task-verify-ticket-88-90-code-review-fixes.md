Type: task
Status: resolved
Blocked by: 96

## Question

Close out the two live checks [ticket 96](./96-task-fix-code-review-findings-tickets-88-90.md)
couldn't run on the build machine. Ticket 96 fixed both `/code-review` findings and
verified what it could — the chord-member tooltip is fully verified there (screenshot
harness + unit test); the launcher's sys.path sanitization works under the system
`python3` (3.14) and `packaging/test_install.sh` now executes the installed launcher.
What's left needs an older interpreter and/or the user's own live desktop:

### 1. Launcher under a pre-3.11 `python3` (the actual regression)

The finding was that `packaging/acheron-gui` used `python3 -P`, which is CPython 3.11+
only — on Ubuntu 22.04 (3.10), Debian 11 / RHEL 9 (3.9) it aborts with
`Unknown option: -P` and the GUI never opens. Ticket 96 replaced it with
`cd "$acheron_lib"; exec python3 -m acheron_gui "$@"` (no version-specific flags). The
build machine only has `python3` 3.14, so confirm in a container:

- In an `ubuntu:22.04` (or `debian:11`) container with GTK4/PyGObject installed, lay
  down the installed layout (`~/.local/lib/acheron/acheron_gui/` + the launcher), run
  `acheron-gui --version` and `acheron-gui --help` → both exit 0, no `Unknown option`.
- From inside a directory containing a decoy `./acheron_gui/`, confirm the launcher
  still imports the *installed* package (the sanitization `-P` used to provide).
- A full `acheron-gui` launch reaching the GTK window is a bonus if the container has a
  display; not required — `--version`/`--help` exercise the interpreter-flag path, which
  is the whole point.

### 2. Real app-grid `.desktop` launch on this machine

Only the launcher's internals changed; `packaging/acheron.desktop` (`Exec=acheron-gui`)
is untouched. Re-run `install.sh` (rebuilds the daemon — coordinate with the user, same
as ticket 95's mid-session service restart), then repeat ticket 90's checks:

- `gtk-launch acheron.desktop` exits 0 and reaches the GUI.
- The **Acheron** entry in the app grid still launches with the icon, focuses the
  running window, no second window / no error.
- `acheron-gui` from a terminal and the grid entry reach the same process.
- `packaging/test_install.sh` green on this machine.

### 3. Optional: tooltip on real Yaru theme

Ticket 96's harness runs under default Adwaita, where key 1's face wraps rather than
ellipsizes. If convenient during the app-grid check, hover a grid key that is both a
Chord member and individually bound on the user's real Yaru theme and confirm the
combined tooltip (`<face text>` ⏎⏎ `Part of Chord: …`) shows both. Low priority — the
tooltip *string* is already locked by `test_chord_member_with_its_own_binding_shows_both_in_the_tooltip`
and the harness dump.

## Answer

All three checks done. Both ticket 88/90 code-review fixes are verified sound.
One pre-existing bug was found in the app-grid re-launch path and spawned as
[ticket 105](./105-task-fix-gui-reactivation-crash.md) — it is not a regression
from the ticket 88/90 fixes.

### 1. Launcher under a pre-3.11 `python3` — verified against real CPython 3.10

The build machine has no docker/podman/pyenv. Installed `uv` (user-local, no
sudo — removable via `rm ~/.local/bin/{uv,uvx} ~/.local/share/uv`) and pulled
real **CPython 3.10.21** to test the actual regression instead of a container:

- **Regression reproduced**: `python3 -P -m acheron_gui` under 3.10 →
  `Unknown option: -P`, exit 2. The finding was real.
- **New launcher, real `acheron_gui` package, real 3.10**: flags parse clean,
  `cd "$acheron_lib"` works, `runpy` loads the *installed* `__main__.py`, and it
  fails only later at `app.py:67 import gi` (uv's 3.10 can't see the system
  PyGObject — an environment gap, not a flag gap). **No `Unknown option`.**
- **New launcher, stub package, real 3.10**, run from inside a checkout `gui/`
  holding a decoy `./acheron_gui/__init__.py` that `SystemExit`s on import:
  `--version` / `-V` / `--help` all exit 0 with correct output, arg-passthrough
  and no-args paths work, and **the decoy is never imported** — the installed
  copy wins, which is exactly what `-P` used to guarantee via `cd`.
- `packaging/test_install.sh` green (incl. the two new launcher checks); 369
  Rust + 332 Python green.

### 2. Real app-grid `.desktop` launch on this machine

The user re-ran `install.sh` (daemon was already stopped, so no live-session
interruption; udev step a no-op — rule already on disk). After it:

- Installed `~/.local/bin/acheron-gui` now byte-identical to `packaging/acheron-gui`
  (no `-P`, `cd "$acheron_lib"` present); `acheron-daemon` restarted, active.
- `gtk-launch acheron.desktop` → exits 0, spawns `python3 -m acheron_gui`
  (PPID = systemd user manager, no `-P`), stays up, **clean stderr on cold start**.
- `acheron-gui` from a terminal while running → **no second process**; reaches
  the same primary instance.
- **App-grid click (user, HITL)**: the **Acheron** entry shows with its icon,
  and clicking it **focuses the existing window — no second window, no visible
  error.** (GNOME Shell raises the window itself here without calling D-Bus
  `activate`, so the ticket-105 path is not hit.)
- `packaging/test_install.sh` — all PASS.

**Bug found (→ ticket 105, pre-existing, not a 88/90 regression):** a second
`acheron-gui` / `gtk-launch` while an instance is running re-enters
`do_activate`, which unconditionally rebuilds the window + `TrayIcon`; the tray
`publish_object` collides with the already-exported SNI object and throws
`GLib.GError: An object is already exported … org.kde.StatusNotifierItem`,
aborting before `win.present()` so the running window is never raised.
Reproduced cleanly (cold start clean, every re-activation throws). Dates to
ticket 36. Contradicts ticket 90's "second launch just presents the existing
window" note, which held only because the app-grid *click* is shell-serviced.

### 3. Tooltip on real Yaru theme (optional)

User confirmed live: a grid key that is both a Chord member and individually
bound shows both the face text and `Part of Chord: …` on hover — "works as
expected."

### Environment note

Left running for the user: one clean `acheron-gui` instance (started for the
visual check) and `acheron-daemon` (started by their `install.sh`). `uv` +
CPython 3.10 remain installed user-local.

