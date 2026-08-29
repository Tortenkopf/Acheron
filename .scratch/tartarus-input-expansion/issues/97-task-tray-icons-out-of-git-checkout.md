Type: task
Status: resolved

## Question

The three tray-indicator status-dot SVGs
(`gui/acheron_gui/icons/acheron-{running-connected,running-disconnected,not-running}.svg`,
ticket 11/36) are read by the SNI host (GNOME Shell's `ubuntu-appindicators` extension)
straight out of the running GUI's package directory —
`tray.ICON_THEME_PATH = <package dir>/icons`. When the GUI runs from a git checkout
(`python3 gui/main.py`, the normal dev path) that directory *is* the working tree. The
desktop shell keeps a live file-watch on `IconThemePath`; overwriting one of those SVGs
in place while the GUI was running hard-crashed the whole session (observed 2026-08-29
while re-drawing the icons).

Get the tray icons out of the git checkout: the SNI host must only ever read them from a
stable per-user location that nothing edits in place.

### Scope

- **Runtime resolution (`gui/acheron_gui/tray.py`).** `IconThemePath` resolves to a fixed
  per-user data dir — `$ACHERON_TRAY_ICON_DIR`, else `$XDG_DATA_HOME/acheron/tray-icons`,
  else `~/.local/share/acheron/tray-icons` — **never** the package dir, whether the GUI is
  run from a checkout or from the `install.sh` copy at `~/.local/lib/acheron/`. The
  package's own `icons/*.svg` stay in the tree as the read-only bundled source.
- **Self-heal on startup.** `TrayIcon` construction syncs the three bundled SVGs into that
  dir when a target file is missing or its bytes differ from the bundled copy — written via
  a temp file + `os.replace` (atomic; never a truncate-in-place on a file the shell is
  watching, which is what crashed). Steady-state launches (files already current) do zero
  writes. Failure is caught + logged, not fatal.
- **`install.sh`.** New step, `$HOME`-only, idempotent like the rest: install
  `gui/acheron_gui/icons/*.svg` → `~/.local/share/acheron/tray-icons/`. Puts them in place
  before the GUI's first launch and makes `install.sh` the authority on the location.
- **`packaging/test_install.sh`.** Assert the three SVGs land in
  `~/.local/share/acheron/tray-icons/` with content matching the source.
- **`gui/tests/test_tray.py`.** Cover: `IconThemePath` is the configured dir and not under
  the repo; construction populates the three files; a stale/missing target is refreshed; an
  unwritable target dir doesn't raise. Point `$ACHERON_TRAY_ICON_DIR` at a tmp dir for the
  suite (conftest).
- **Docs.** Add the new installed path to ticket 90's installed-paths table / the note for
  ticket 35.

### Out of scope

- The freedesktop app icon (`hicolor/.../acheron.png`, ticket 90) — different asset,
  already installed outside the checkout, untouched.
- Redrawing the placeholder dots themselves (ticket 11's "later commissioned icon").

## Answer

Done. The SNI host is now only ever pointed at a stable per-user data dir; the package's
`icons/*.svg` are bundled read-only source that gets synced out on GUI launch.

Not live-verified on hardware — this is a resolution-path + install change with no runtime
behaviour change for the user (same three icons, same panel rendering). Folded into
[ticket 50](./50-task-verify-tray-icon-on-hardware.md)-style checks if a tray pass is
re-run, but not gated on it.

### What changed

- **`gui/acheron_gui/tray.py`**
  - Removed the module-level `ICON_THEME_PATH = <package>/icons`. Added `BUNDLED_ICON_DIR`
    (the same path, now explicitly just the *source*) and two helpers:
    - `_resolve_icon_theme_path()` → `$ACHERON_TRAY_ICON_DIR`, else
      `$XDG_DATA_HOME/acheron/tray-icons`, else `~/.local/share/acheron/tray-icons`. Never
      the package dir — so a checkout run (`python3 gui/main.py`) and an installed run
      (`~/.local/lib/acheron/`) resolve identically.
    - `_sync_bundled_icons(dest)` → copies the three bundled SVGs in, but only those missing
      or byte-different from the bundled copy (steady-state launch writes nothing). Each
      write is temp-file + `os.replace` — atomic, never a truncate-in-place on a file the
      shell is watching (the ticket's crash trigger). Wrapped in `try/except OSError`:
      logs to stderr, never raises.
  - `TrayIcon.__init__` resolves + syncs once, stores `self._icon_theme_path`, exposed as
    the `icon_theme_path` property. `_StatusNotifierItemService.IconThemePath` now reads
    that instead of the module constant.
- **`install.sh`** — new step after the app-icon install, `$HOME`-only and idempotent like
  the rest: `cp gui/acheron_gui/icons/*.svg` → `~/.local/share/acheron/tray-icons/`. Puts
  them in place before the first GUI launch and makes `install.sh` the authority on the
  path; the GUI's own sync is then just a self-heal.
- **`packaging/test_install.sh`** — asserts the three SVGs land at
  `~/.local/share/acheron/tray-icons/` with content matching `gui/acheron_gui/icons/`.
- **`gui/tests/conftest.py`** — autouse fixture points `$ACHERON_TRAY_ICON_DIR` at a
  per-test tmp dir, so the suite never writes into the real user data dir.
- **`gui/tests/test_tray.py`** — 5 new tests: path is the configured dir and never
  `BUNDLED_ICON_DIR`; construction syncs all three files with matching bytes; a stale
  synced file is refreshed; an unwritable target dir logs and doesn't raise; `IconThemePath`
  on the published item matches.
- **Docs** — note added to `35-task-write-release-documentation.md` for the new installed
  path / uninstall list.

### Installed path added (for ticket 35)

| Path | What |
|---|---|
| `~/.local/share/acheron/tray-icons/acheron-{running-connected,running-disconnected,not-running}.svg` | tray indicator status-dot icons — the SNI host's `IconThemePath` |

### Verification (this session)

- Full GUI suite: `317 passed`.
- `packaging/test_install.sh`: all green, exit 0 (idempotent across two runs).
- Manual: `_resolve_icon_theme_path()` from a checkout with a clean `XDG_DATA_HOME`
  resolves outside the tree; first `_sync_bundled_icons` writes all three, second call
  rewrites nothing (mtimes unchanged).

### Follow-up (post-resolve)

Running `packaging/test_install.sh` from the checkout to verify the new tray-icon
assertion exposed a **pre-existing** footgun: the test's fake `cargo` wrote its stub
binary into the real `daemon/target/release/`, and because cargo's fingerprints stayed
fresh, a subsequent real `install.sh` copied that stub into `~/.local/bin/acheron-daemon`
(daemon "started" then exited 0 printing `fake-acheron-daemon`). Fixed in a separate
commit: `test_install.sh` now runs `install.sh` from a throwaway repo copy under `$work`,
and the fake cargo refuses to write outside `$SANDBOX_ROOT`. Real binary rebuilt and
reinstalled; daemon confirmed active and answering D-Bus.

Status: resolved
