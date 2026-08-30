Type: task
Status: resolved

## Notice:
While this Ticket is not technically blocked by anything, it should still deferred towards the end
of 1.0 development, as we cannot say for certain what else will need to go into this documentation,
that might still surface while the full feature set is being implemented.

Run this ticket's final pass *after* [ticket 90](./90-task-desktop-app-launcher.md) lands — it
adds a GUI launcher (`acheron-gui`), a `.desktop` entry, an app icon, and new `install.sh`
steps + installed paths that the install/usage docs must cover (the GUI has no documented
launch path today beyond `python3 gui/main.py` from a checkout).

Ticket 90 is now **resolved** — its Answer carries the full installed-path table (for the
install/usage docs and any uninstall section) and two things worth folding in here:
(1) the app-grid `Exec=acheron-gui` needs `~/.local/bin` on `PATH` — worth a doc line;
(2) `install.sh` uses `cp` (not `install`) for the daemon binary, so it fails
`Text file busy` if `acheron-daemon` is already running — the clean-checkout end-to-end
check should either stop the unit first or this should be fixed to `install`.

Ticket 97 adds one more installed path: `install.sh` now also copies the three tray
status-dot SVGs to `~/.local/share/acheron/tray-icons/` (the GUI reads them only from
there, never the checkout). Add it to the installed-path / uninstall list.

Ticket 99 (component version strings) hands two things here:
(1) Document where each component's canonical version lives — `daemon/Cargo.toml`'s
`version` and `gui/acheron_gui/__init__.py`'s `_BASE_VERSION` — so a release cut bumps
both. Both are `1.0.0` at v1.0.
(2) Both versions render as bare `1.0.0` only from a checkout of the `v1.0.0` git tag (or
a no-git tarball); any other checkout shows `1.0.0-dev+<short-hash>`. So the release
process must **tag `v1.0.0` before building** the artifacts users install, and the
README's build instructions should mention that a plain `main` checkout will self-label
`-dev`. `daemon/build.rs` also honours an explicit `ACHERON_VERSION` env override if a
packager ever needs to pin the string. `install.sh` did **not** gain a version-stamping
step — the git-tag/tarball detection made one unnecessary.

Ticket 102 (About dialog) hands one installed-path addition here:
`install.sh` now also copies the repo-root `LICENSE` to
`~/.local/lib/acheron/acheron_gui/LICENSE` (the About dialog's "View Licence"
button reads it there; a dev checkout falls back to the repo-root file). Add
it to the installed-path / uninstall list. Also worth a doc line: the About
screen is where a user finds the version, the connected device's
firmware/serial, the acknowledgements, and the GPLv3 notice — reached from
the main window's header-bar menu ("About Acheron").

## Question

Write the release documentation a stranger needs to build, install, and use Acheron from a clean
git checkout: `README.md` (what it is, feature list, screenshots/description of the GUI),
install/build instructions matching `install.sh`'s real steps (including its privileged udev-rule
step, per [Wire live source-swap, udev rule, and install.sh](./23-task-wire-analog-supervisor-and-install.md)),
and a `CONTRIBUTING.md` if warranted. Both blocking prerequisites are now settled: the v1.0 feature
list ([Lock the v1.0 feature list](./08-decide-v1-feature-list.md)) and the license
([Choose an open source license](./09-decide-open-source-license.md) — GPLv3-or-later, `LICENSE`
already at the repo root).

Settle at least:

- **Structure and scope**: README sections (feature list, hardware requirement, build/install,
  usage basics, license) vs. a fuller docs split (separate INSTALL.md, USAGE.md); match the
  project's actual size rather than over-building.
- **Per-file copyright headers**: [ticket 09](./09-decide-open-source-license.md) deferred the
  "or (at your option) any later version" GPL header convention to this ticket as a repo-wide
  mechanical pass — decide the exact header text and where it lands (every source file? just
  new/touched ones going forward?).
- **How much process detail to carry into the public repo** — `.scratch/`, `prototype/`,
  `docs/adr/`, `CONTEXT.md` are all currently assumed to ship as-is, but the user flagged a
  concern this might overwhelm someone who just wants to game. This ticket's README should at
  least *not make that worse* (e.g. don't lead with process); whether those directories ship at
  all is tracked separately in the map's Not yet specified and isn't this ticket's call to make.
- Live-check: does `install.sh` actually work end-to-end on a clean checkout, following only the
  written instructions?

## Answer

The three open decisions were put to the user directly and all three landed on the
recommended option:

- **Structure and scope**: a single `README.md` + a short `CONTRIBUTING.md`, no
  `INSTALL.md`/`USAGE.md` split — matches the project's real size (one Daemon, one GUI,
  one `install.sh`).
- **Per-file headers**: an SPDX one-liner (`SPDX-License-Identifier: GPL-3.0-or-later`
  + `Copyright © 2026 Justin Milatz`) on **every source file of the shipped program** —
  `//` for Rust, `#` for Python/shell, after any shebang. Scoped to `daemon/` and
  `gui/` (incl. tests and tools) plus `install.sh` / `packaging/test_install.sh` /
  `packaging/acheron-gui`: **62 files** headed by a one-shot mechanical pass. Deliberately
  **excluded**: `prototype/` and `.scratch/` (throwaway process artifacts, not the
  program; whether they ship at all is still the map's open "what ships in the public
  repo" question), and non-source config/manifest files (`Cargo.toml`, the `.service` /
  `.rules` / `.desktop` files, `.gitignore`) which don't conventionally carry GPL
  headers. `README.md`/`CONTRIBUTING.md` carry the SPDX lines in an HTML comment.
- **Install check**: automated + dry review this session; a manual checklist (below)
  for the parts needing real `sudo`/`systemd`/a clean clone — same build→verify split as
  every other ticket on this map.

### What was written

- **`README.md`** — what it is (+ the verbatim Acheron-river note with a Wikipedia link),
  the feature list in `CONTEXT.md`'s vocabulary, a "what it is not" section (lighting,
  auto-profile-switching, generic remapping — all out of scope), the Tartarus-Pro-only
  hardware requirement, system requirements (Linux + systemd user instance + session bus;
  Ubuntu/GNOME/Wayland as the tested target, KDE/XFCE expected-not-tested, the GNOME
  AppIndicator extension called out; `plugdev`; Rust ≥ 1.85 for the build; Python 3.9+ /
  PyGObject / GTK 4 / dasbus for the GUI; an Ubuntu `apt` one-liner), the `install.sh`
  build/install steps **including the privileged udev step** and the `~/.local/bin`-on-
  `PATH` requirement, a release-tagging note (tag `v1.0.0` before building or both
  components self-label `-dev+<hash>`; canonical versions in `daemon/Cargo.toml` +
  `gui/acheron_gui/__init__.py` `_BASE_VERSION`; `ACHERON_VERSION` override), an
  installed-files table, a hand-rolled uninstall recipe, usage basics (Device Overview,
  Base/Held, Chords, Library, tray, About dialog, the edit-blocked-when-disconnected
  behavior), a config-file section (Daemon owns it; stop it before hand-editing;
  refuse-to-start on a bad file), an "after a rebuild → `systemctl --user restart`" note,
  and a troubleshooting list. Licence + acknowledgements (ultramonaka's
  open-tartarus-driver, Matt Pocock's skills) close it. Process history is **not** in the
  README — it's a one-paragraph "Design record" pointer in `CONTRIBUTING.md` only.
- **`CONTRIBUTING.md`** — repo layout table, the `.scratch/` design-record pointer, the
  "use `CONTEXT.md`'s vocabulary / flag ADR conflicts" rule, build commands, all three
  test suites with exact invocations (`cargo test`/`fmt`/`clippy`; a
  `python3 -m venv --system-site-packages` + `pip install pytest` + `pytest gui/tests`
  recipe, since there's no committed Python dep manifest and `.venv/` is gitignored;
  `bash packaging/test_install.sh`), the hardware-verification discipline, the licence-
  header convention + a DCO-style "you license your contribution GPL-3.0-or-later" line.
- **`install.sh` fix** (ticket 90 hand-off #2): the daemon binary is now installed with
  `rm -f` + `install -m 755` instead of a plain `cp` over the top, so a rebuild while the
  Daemon is running no longer fails `ETXTBSY` ("Text file busy"). Comment added noting the
  running process keeps the old build until `systemctl --user restart`.
- **`Device Picture.jpg` removed from the repo entirely** (user's call — no device photo in
  the README or the tree). The two stale references in the archived `tartarus-keybinder`
  records (`map.md`, `issues/09`) repointed to `layout.md`, which documents the same physical
  layout and still exists.

All hand-offs folded in: ticket 90 (`PATH` note ✔, `cp`→`install` ✔), ticket 97
(tray-icons path in the installed-files/uninstall lists ✔), ticket 99 (version locations
✔, tag-before-build ✔, `ACHERON_VERSION` ✔), ticket 102 (bundled `LICENSE` path ✔, About
dialog documented ✔).

### Verification done this session

- `cargo build --release --manifest-path daemon/Cargo.toml` — **exit 0** (the real
  `install.sh` build step).
- `cargo test` **380 passed**, `cargo fmt --check` clean, `cargo clippy --all-targets -D
  warnings` clean — after the 62-file header pass, no regressions.
- `pytest gui/tests` — **355 passed** — after the header pass, no regressions.
- `bash packaging/test_install.sh` — **all PASS** (idempotency, unit/udev/desktop
  content, the launcher smoke-run, tray icons), after the `install.sh` binary-install
  change.
- Line-by-line dry review of `install.sh` against the README's written steps — they
  match.

### Manual checklist handed to the user (needs a clean clone + real sudo/systemd)

1. Fresh `git clone` to an empty dir on a machine with the Tartarus Pro connected;
   follow **only** the README's Install section.
2. `./install.sh` completes; the `sudo` udev prompt appears once; declining it still
   leaves a working (digital-capture) Daemon with the printed manual recovery commands.
3. `systemctl --user status acheron-daemon` → active; `acheron-gui` launches from the
   app grid and from the shell.
4. On a checkout **not** on the `v1.0.0` tag, About shows `1.0.0-dev+<hash>` for both
   components; on a `git checkout v1.0.0`, both show bare `1.0.0`.
5. Re-run `./install.sh` with the Daemon running — the binary install no longer errors
   `Text file busy`; `systemctl --user restart acheron-daemon` picks up the new build.
6. Follow the uninstall recipe; confirm nothing Acheron-owned is left except (optionally)
   `~/.config/acheron`.

### Not this ticket's call (left open)

Whether `.scratch/`, `prototype/`, `docs/adr/`, `CONTEXT.md` ship in the public repo at
all — still tracked in the map's **Not yet specified**. The README simply doesn't lead
with process, per this ticket's own scope note.
