# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Acheron's GTK4 GUI — edits the Daemon's config live over D-Bus.

See `.scratch/tartarus-keybinder/spec.md` ("GUI information architecture")
and `prototype/09-gui-information-architecture/prototype.py` for the design
this package implements against the real Daemon (ticket 16).
"""

from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Callable

# Ticket 99: the GUI's own SemVer, independent of the Daemon's
# (`daemon/Cargo.toml`) — the two install as separate units and are free to
# diverge after v1.0. The About dialog (ticket 102) shows this prominently
# and the Daemon's `GetState()` `daemon_version` as a secondary line.
_BASE_VERSION = "1.1.0"

# A git-probe callable: takes git args (no leading "git"), returns the
# trimmed stdout, or None on any non-zero exit / failure / empty output.
GitProbe = Callable[..., "str | None"]


def _assemble_version(base: str, git: GitProbe) -> str:
    """The pure `-dev` suffix rule, mirroring `daemon/src/build_version.rs`:
    a checkout sitting exactly on the `v<base>` release tag (or a probe that
    reports no git at all) yields the bare `base`; any other checkout yields
    `<base>-dev+<short-hash>`.
    """
    if git("describe", "--tags", "--exact-match", "HEAD") in (f"v{base}", base):
        return base
    short_hash = git("rev-parse", "--short", "HEAD")
    return f"{base}-dev+{short_hash}" if short_hash else base


def _repo_git_probe(repo_root: Path) -> GitProbe:
    """A `GitProbe` bound to `repo_root`, or one that always returns None
    when `repo_root` has no `.git` (an installed copy under
    `~/.local/lib/acheron`, or a source tarball)."""
    if not (repo_root / ".git").exists():
        return lambda *_args: None

    def probe(*args: str) -> str | None:
        try:
            result = subprocess.run(
                ["git", "-C", str(repo_root), *args],
                capture_output=True,
                text=True,
                timeout=5,
            )
        except (OSError, subprocess.SubprocessError):
            return None
        if result.returncode != 0:
            return None
        return result.stdout.strip() or None

    return probe


def _resolve_version(base: str) -> str:
    repo_root = Path(__file__).resolve().parent.parent.parent
    return _assemble_version(base, _repo_git_probe(repo_root))


__version__ = _resolve_version(_BASE_VERSION)
