Type: task
Status: claimed
Assignee: Charon (2026-08-29)

## Question

Establish real version strings for both components and make the Daemon's readable over D-Bus,
so the About dialog (ticket 102) has a "current version" to show and the v1.0 release has a
version at all. Acheron carries **no version anywhere today**: `daemon/Cargo.toml` says
`0.1.0`, nothing uses `CARGO_PKG_VERSION`, there is no `build.rs`, the GUI has no version
constant and no Python packaging files, and there are no git tags.

Settled in the charting grilling (2026-08-29) — do not re-litigate:

- **Independent per-component SemVer.** The Daemon and GUI ship as separately-installable
  units (a `systemd --user` service vs. a `~/.local` GUI package) with a D-Bus protocol
  between them, so they get independent versions in their own manifests, free to diverge
  after v1.0. Both are `1.0.0` at first release.
- **Daemon version** lives in `daemon/Cargo.toml` (`version = "1.0.0"`).
- **GUI version** lives in a new `__version__` in `gui/acheron_gui/__init__.py`.
- **The Daemon reports its version over D-Bus** as one new string key in `GetState()`'s keyed
  dict (`state_to_dict` in `daemon/src/dbus/wire.rs`, the shape ticket 25 moved to). The
  About dialog shows the GUI's own `__version__` prominently and the Daemon's as a secondary
  detail line — drift between them is then visible in bug reports.
- **Dev-checkout builds** show `1.0.0-dev+<short-hash>` rather than a bare `1.0.0`.

Decide and implement the mechanical parts left open:

- **How the Daemon derives its runtime version string.** Options: a `build.rs` that shells
  `git describe`/`git rev-parse --short HEAD` and emits `1.0.0` vs `1.0.0-dev+<hash>` as a
  compile-time env var (falls back to bare `CARGO_PKG_VERSION` when git is absent, e.g. a
  release tarball); or plain `CARGO_PKG_VERSION` with the `-dev` suffix logic in code. Pick
  the simpler one that still degrades cleanly outside a git checkout.
- **How the GUI derives its version string.** The GUI is installed as plain files to
  `~/.local/lib/acheron/` (ticket 90), so there is no wheel metadata to read. Either
  `install.sh` stamps a `_version.py` at install time, or `__version__` is a plain literal
  in `__init__.py` with dev-checkout detection (a `.git` dir above the package) adding the
  `-dev+<hash>` suffix at import. Keep it working both from a checkout (`python3 gui/main.py`)
  and from an install.
- Wire the new `GetState()` key through every consumer: `daemon/src/dbus/wire.rs` +
  `daemon/src/dbus/mod.rs`, `gui/acheron_gui/daemon_client.py`, `daemon_stub.py`, and
  `app.py`'s state handling — mirror how `capture_mode` was threaded in tickets 21/25.
- Bump `daemon/Cargo.toml` to `1.0.0`; add `__version__` to `gui/acheron_gui/__init__.py`.

AFK — no hardware needed. Purely additive to the D-Bus surface (a new optional-to-ignore
key), no `SCHEMA_VERSION`/protocol-version concerns. Daemon + GUI test suites must stay green;
add coverage for the `-dev` suffix logic and the new state key.

Hand to [ticket 35](./35-task-write-release-documentation.md): the final versioning decision
(where the canonical version lives per component) belongs in the release/CONTRIBUTING docs,
and `install.sh` may gain a version-stamping step.

## Answer
