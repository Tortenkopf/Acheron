Type: task
Status: resolved

## Question

Give Acheron's GUI a real desktop-app launch path (user's `/wayfinder` Round 1, Q1). Today the
GUI is only runnable as `python3 gui/main.py` from a git checkout; `install.sh` installs the
Daemon binary + systemd unit + udev rule only, and there is no `.desktop` entry or app icon
anywhere. A user who ran `install.sh` still has no way to start the GUI from their desktop
environment's app grid.

Build and **live-verify in the same session** (display always available on this machine).

### Scope (settled in the charting grill)

- **`acheron-gui` launcher script.** A thin shell wrapper installed to `~/.local/bin/acheron-gui`
  that runs the in-place package: `exec python3 -m acheron_gui "$@"` with `PYTHONPATH` pointed
  at the installed GUI source. **No Python packaging / venv / `pip install` work** — the
  `.desktop` file and launcher resolve to a fixed installed location for the GUI source, they
  do not point back into an arbitrary git checkout.
  - This needs `acheron_gui` to be runnable as `python3 -m acheron_gui` — add a
    `gui/acheron_gui/__main__.py` (`from .app import main; main()`) mirroring the existing
    `gui/main.py`, if it isn't already.
  - Decide and document where the GUI source installs to (e.g. `~/.local/share/acheron/` or
    `~/.local/lib/acheron/`) — `install.sh` copies `gui/acheron_gui/` there.
- **`packaging/acheron.desktop`** — a standard freedesktop entry:
  `Type=Application`, `Name=Acheron`, `Exec=acheron-gui`, `Icon=acheron`,
  `Categories=Utility;Settings;` (roughly), `Comment=` one line. Installs to
  `~/.local/share/applications/acheron.desktop`.
- **App icon.** The user is supplying an **SVG** (square canvas, legible down to ~22–24px).
  Installs to `~/.local/share/icons/hicolor/scalable/apps/acheron.svg`. If PNG rasters are
  also supplied (48/64/128/256), install each to `hicolor/<size>x<size>/apps/acheron.png`.
  Keep the source asset(s) under `packaging/` (e.g. `packaging/icons/`). This is the app's
  identity mark — distinct from the three tray state-dot SVGs in `gui/acheron_gui/icons/`
  (ticket 11/36), which stay where they are.
- **`install.sh`** — a new section that installs the launcher, the `.desktop` file, and the
  icon(s) to the paths above (`mkdir -p` each, plain overwrite, idempotent like the rest of
  the script), then runs `update-desktop-database ~/.local/share/applications` and
  `gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor` **best-effort** (guarded like the
  udev step — a failure prints a note and does not abort the install; the app still launches
  from the terminal via `acheron-gui`). Update `packaging/test_install.sh` to cover the new
  files.
- **Uninstall note.** If `install.sh` has (or the docs describe) an uninstall path, extend it
  to remove these files too. If not, don't invent one here — just list the installed paths in
  the ticket answer for ticket 35 to document.

### Coordination with ticket 35 (release documentation)

Ticket 35 documents `install.sh`'s real steps and does a clean-checkout end-to-end check, so it
should run its final pass **after** this ticket lands. Add a one-line note to
`35-task-write-release-documentation.md` pointing at this ticket for the GUI-launch/`.desktop`
step. Don't write the user-facing prose here — just make `install.sh` correct and leave the
installed-path list in this ticket's answer.

### Verification (fold into this session)

- Run `install.sh` on this machine; confirm `acheron-gui` appears on `PATH`, launches the GUI,
  and the entry shows in the desktop environment's app grid with the icon rendered (no
  re-login needed after the cache refresh — or note if one is).
- Launch once from the app grid, once from `acheron-gui` in a terminal — both reach the same
  running GUI against the live Daemon.
- `packaging/test_install.sh` green; full GUI suite green.

## Answer

Built and live-verified on this machine (GNOME/Wayland, Ubuntu). The GUI now has a
real desktop-app launch path; nothing points back into a git checkout.

### What was built

- **`gui/acheron_gui/__main__.py`** — a 3-line trampoline (`from acheron_gui.app import
  main` under an `if __name__ == "__main__"` guard) mirroring `gui/main.py`, so the
  package runs as `python3 -m acheron_gui`. `gui/main.py` is unchanged and still works
  from a checkout.
- **`packaging/acheron-gui`** — thin bash launcher. Puts the installed package dir on
  `PYTHONPATH` and `exec python3 -P -m acheron_gui "$@"`. The `-P` flag keeps the current
  directory off `sys.path`, so running `acheron-gui` from inside a checkout's `gui/` dir
  still imports the *installed* package, not `./acheron_gui`. Guards with a clear error if
  the package isn't installed. Honors `ACHERON_GUI_LIB` for an override (used by nothing
  in normal operation; handy for testing). No venv / pip / Python packaging — uses the
  system `python3`, same GTK4/PyGObject requirement as `python3 gui/main.py`.
- **`packaging/acheron.desktop`** — freedesktop entry: `Type=Application`, `Name=Acheron`,
  `Exec=acheron-gui`, `Icon=acheron`, `Categories=Settings;HardwareSettings;`,
  `Terminal=false`, plus `GenericName`, `Comment`, `Keywords`, `StartupNotify=true`, and
  **`StartupWMClass=com.acheron.gui`** (matches the GTK `application_id` so GNOME
  associates the running window with this entry — window icon + name in the dash).
  `desktop-file-validate` clean.
- **App icons** — the supplied `Acheron.svg` turned out **not to be a true vector**: it's a
  single 1254×1254 PNG wrapped in `<svg><image href="data:image/png;base64,…">` (1.6 MB).
  Per the user's call (asked in-session), the wrapped SVG is **not** installed to
  `scalable/`. Instead, PNG rasters at 16/24/32/48/64/128/256/512 px are generated from
  `Acheron.png` (via GdkPixbuf, HYPER scaling) and committed under
  `packaging/icons/hicolor/<size>x<size>/apps/acheron.png`. This covers every realistic
  app-grid/panel size including 2× HiDPI. Source assets kept at `packaging/icons/acheron.svg`
  (as supplied) and `packaging/icons/acheron-master.png` (the master raster). The root-level
  `Acheron.svg` / `Acheron.png` the user had dropped in were byte-identical and were removed
  (now homed under `packaging/icons/`). The three tray state-dot SVGs in
  `gui/acheron_gui/icons/` are untouched. **If a genuine vector is ever supplied**, drop it
  at `packaging/icons/acheron.svg` and add one `install` line to `install.sh` for
  `scalable/apps/acheron.svg`.
- **`install.sh`** — new "GUI desktop-app launch path" section after the daemon steps, all
  under `$HOME`, no sudo:
  - copies `gui/acheron_gui/` → `~/.local/lib/acheron/acheron_gui/` (`rm -rf` the old copy
    first for idempotency; strips `__pycache__`)
  - `install -m 755 packaging/acheron-gui` → `~/.local/bin/acheron-gui`
  - `install -m 644 packaging/acheron.desktop` → `~/.local/share/applications/acheron.desktop`
  - `cp -r packaging/icons/hicolor/.` → `~/.local/share/icons/hicolor/`
  - best-effort `update-desktop-database` + `gtk-update-icon-cache -f -t` (guarded like the
    udev step — a failure prints a note, doesn't abort; seeds `index.theme` from the system
    hicolor theme first so the icon-cache call can succeed for a per-user theme dir)
- **`packaging/test_install.sh`** — new section asserts the installed package (with
  `__main__.py`, no `__pycache__` leak), the launcher (executable, content matches), the
  desktop entry (content matches, has `Exec=`/`Icon=`, `desktop-file-validate` passes), all
  8 icon sizes, and that `update-desktop-database` runs once per install run.
  `update-desktop-database`/`gtk-update-icon-cache` are stubbed for hermeticity.

### Decision: GUI source install location

**`~/.local/lib/acheron/`** (the importable `acheron_gui/` package lives at
`~/.local/lib/acheron/acheron_gui/`; the launcher sets `PYTHONPATH=~/.local/lib/acheron`).
Chosen over `~/.local/share/acheron/` because this is code, not data. Not a formal XDG dir
but a widely-used convention (pipx, Meson `--libdir`).

### Installed paths (for ticket 35 to document; also the uninstall list)

`install.sh` has no uninstall path and none was invented. Files this ticket adds:

| Path | What |
|---|---|
| `~/.local/lib/acheron/acheron_gui/` | GUI package (whole tree) |
| `~/.local/bin/acheron-gui` | launcher script |
| `~/.local/share/applications/acheron.desktop` | desktop entry |
| `~/.local/share/icons/hicolor/{16,24,32,48,64,128,256,512}x…/apps/acheron.png` | app icons |
| `~/.local/share/icons/hicolor/index.theme` | seeded from `/usr/share/icons/hicolor/` if absent, so the icon cache can build |
| `~/.local/share/icons/hicolor/icon-theme.cache` | written by `gtk-update-icon-cache` |

Requires `~/.local/bin` on `PATH` for the app-grid `Exec=acheron-gui` to resolve (true on
this machine and standard on modern desktop distros; worth a line in the install docs).

### Verification (done this session)

- `install.sh` ran to completion on this machine (`EXIT=0`). Pre-existing wrinkle unrelated
  to this ticket: `cp` of the daemon binary fails `Text file busy` if the daemon is running
  — had to `systemctl --user stop acheron-daemon` first (install.sh uses `cp`, not
  `install`, for that step; not in scope here — flagged for ticket 35 / a future fix). The
  udev step degraded gracefully (`sudo` non-interactive → printed the manual-recovery note,
  continued); the rule was already on disk from ticket 23/29 anyway.
- `acheron-gui` resolves on `PATH` → `~/.local/bin/acheron-gui`.
- Launcher targets the **installed** package (verified with `-P` from multiple cwds), and
  its missing-package guard fires with a clear message + exit 1.
- GTK icon theme resolves `acheron` → the installed hicolor PNGs at 48 px and 256 px.
- **Cold start**: with no instance running, `acheron-gui` from `$HOME` starts
  `python3 -P -m acheron_gui`, owns `com.acheron.gui`, exports a `window` node on the bus,
  stays up, no errors.
- **App-grid launch**: `gtk-launch acheron.desktop` (exactly what GNOME Shell runs on an
  app-grid click) exits 0 and reaches the GUI. Single-instance (`application_id`) means a
  second launch just presents the existing window — verified both directions
  (`acheron-gui` ↔ grid) reach the same process against the live Daemon.
- **User visual pass (HITL)**: user confirmed the window icon renders (not a placeholder),
  the **Acheron** entry appears in the GNOME app grid with the icon, and clicking it
  focuses the running window with no second window / no error. **No re-login was needed** —
  the entry and icon showed immediately after `install.sh`'s cache refresh.
- `packaging/test_install.sh` — all green (6 PASS lines).
- Full GUI suite — `295 passed`. No daemon code changed (no Rust test run needed; the
  release build in `install.sh` compiled clean).

### Coordination with ticket 35

The forward-pointing note already exists at the top of
`35-task-write-release-documentation.md` (added during charting). Expanded it with the
concrete new artifacts. The installed-path table above is what ticket 35 documents for
install/usage + any uninstall section.

Status: resolved
