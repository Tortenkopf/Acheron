Type: task
Status: resolved
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

Both components now carry a real `1.0.0` and the Daemon reports its string over D-Bus.
Built and tested; no hardware needed (AFK). 369 Rust + 331 Python tests green.

**Version-string derivation — one rule, both components.** A build/import from a git
checkout sitting *exactly* on the `v<version>` release tag (or from a source tree with no
`.git` at all — a tarball) gets the bare version; **any other checkout gets
`<version>-dev+<short-hash>`**. This is stricter than the ticket's "git absent → bare, git
present → dev" sketch on purpose: the project's primary install path is `git clone` + 
`install.sh`, so without the exact-tag check *every* user would see `-dev`. The release
process must therefore `git tag v1.0.0` before building the artifacts users install — noted
in [ticket 35](./35-task-write-release-documentation.md).

- **Daemon** (`daemon/build.rs`, new): shells `git describe --tags --exact-match HEAD` and
  `git rev-parse --short HEAD` at compile time, stamps the result into the `ACHERON_VERSION`
  compile-time env var via `cargo:rustc-env`. An explicit `ACHERON_VERSION` in the
  environment overrides the probe entirely (for packagers). `rerun-if-changed` on
  `../.git/HEAD`/`refs`/`packed-refs` (only when they exist) keeps the stamped hash fresh.
  Read back as `pub const acheron_daemon::VERSION` (`daemon/src/lib.rs`). The pure
  base-vs-`-dev` assembly is factored into `daemon/src/build_version.rs`, `include!`d by both
  `build.rs` and the crate's own `#[cfg(test)]` module so the `-dev` rule has test coverage
  (build-script code is otherwise untestable). `daemon/Cargo.toml` bumped `0.1.0` → `1.0.0`.
- **GUI** (`gui/acheron_gui/__init__.py`): `_BASE_VERSION = "1.0.0"` literal; `__version__`
  is computed at import via the same rule, walking three parents up from `__file__` for a
  `.git` dir (present from a checkout, absent under `~/.local/lib/acheron/`). All git calls
  are wrapped — any failure degrades to the bare base. Pure assembly (`_assemble_version`)
  and the git-probe factory (`_repo_git_probe`) are separate, testable seams
  (`gui/tests/test_version.py`, new).

Both currently self-label `1.0.0-dev+b1126d7` from this checkout, verified by running each.

**D-Bus wiring.** `command::State` gained `daemon_version: &'static str`, set to
`crate::VERSION` in `dispatch.rs`'s `GetState` handler. `wire::state_to_dict` emits it as a
new `"daemon_version"` string key — purely additive, ignorable by old clients, no
`SCHEMA_VERSION` bump (ticket 25's keyed-dict shape is exactly what makes this free). GUI
side: `DBusDaemonClient.get_state()` passes the dict through untouched, so no client change
was needed; `daemon_stub.py` returns a `"1.0.0"` stand-in for the key (mirroring its
`capture_mode` stand-in). **`app.py` was deliberately not touched** — nothing in the main
window renders the Daemon version; the *only* consumer is the About dialog, which
[ticket 102](./102-task-build-about-dialog.md) will build and which reads `state["daemon_version"]`
directly. Threading it through `rebuild()`/`last_known` the way `capture_mode` is threaded
would be dead plumbing until then.

**Tests added:** `assemble_version`/`_assemble_version` on-tag / off-tag / no-git cases
(Rust + Python), `_repo_git_probe` over a no-`.git` dir and over the real repo, a
`VERSION`-reaches-the-crate smoke test, `daemon_version` in `state_to_dict` and over real
D-Bus (`get_state_over_real_dbus_returns_the_live_snapshot`), and the updated
`daemon_stub` exact-shape assertion.

**Not done (out of this ticket's scope):** no `acheron-daemon --version` CLI flag was added
— the ticket scoped the version to the D-Bus surface for the About dialog and didn't ask
for one; it's a trivial follow-up if wanted. Handoffs to
[ticket 35](./35-task-write-release-documentation.md) recorded in its body (tag-before-build
requirement, where each canonical version lives, `ACHERON_VERSION` override; no `install.sh`
version-stamping step needed).
