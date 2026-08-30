# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright © 2026 Justin Milatz

"""Ticket 99: the GUI's `__version__` derivation — a bare release string
from an install or a release-tag checkout, a `-dev+<hash>` suffix from any
other checkout, and a clean degrade to bare on any git failure."""

from pathlib import Path

import acheron_gui
from acheron_gui import _assemble_version, _repo_git_probe


def _fake_git(responses):
    return lambda *args: responses.get(args)


def test_checkout_on_the_release_tag_gets_the_bare_version():
    git = _fake_git(
        {
            ("describe", "--tags", "--exact-match", "HEAD"): "v1.0.0",
            ("rev-parse", "--short", "HEAD"): "abc1234",
        }
    )
    assert _assemble_version("1.0.0", git) == "1.0.0"


def test_checkout_off_the_release_tag_gets_a_dev_suffix():
    git = _fake_git(
        {
            ("describe", "--tags", "--exact-match", "HEAD"): None,
            ("rev-parse", "--short", "HEAD"): "abc1234",
        }
    )
    assert _assemble_version("1.0.0", git) == "1.0.0-dev+abc1234"


def test_no_git_at_all_gets_the_bare_version():
    assert _assemble_version("1.0.0", lambda *_a: None) == "1.0.0"


def test_probe_over_a_dir_with_no_dot_git_never_shells_out(tmp_path):
    probe = _repo_git_probe(tmp_path)
    assert probe("rev-parse", "--short", "HEAD") is None


def test_probe_over_the_real_repo_root_reports_a_hash():
    repo_root = Path(acheron_gui.__file__).resolve().parent.parent.parent
    probe = _repo_git_probe(repo_root)
    # This checkout has a .git, so a short hash comes back (40 hex -> ~7+).
    short_hash = probe("rev-parse", "--short", "HEAD")
    assert short_hash and all(c in "0123456789abcdef" for c in short_hash)


def test_the_real_module_version_starts_from_the_base():
    assert acheron_gui.__version__ == "1.0.0" or acheron_gui.__version__.startswith("1.0.0-dev+")
