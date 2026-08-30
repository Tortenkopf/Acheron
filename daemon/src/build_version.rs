// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

// The pure half of ticket 99's runtime-version derivation. `include!`d by
// both `build.rs` (which feeds it real `git` output at compile time) and
// the crate's own test module, so the `-dev` suffix rule has one home and
// actual test coverage. Kept to std `format!` only — `build.rs` runs it
// before the crate is compiled.

/// Assembles the version string `build.rs` stamps into `ACHERON_VERSION`.
///
/// `base` is `CARGO_PKG_VERSION` (`daemon/Cargo.toml`'s `version`). A dev
/// checkout — a git working tree whose `HEAD` is *not* sitting exactly on
/// the `v<base>` release tag — yields `<base>-dev+<short-hash>`. A checkout
/// of the release tag itself, or a source tarball with no git at all
/// (`git_short_hash` is `None`), yields the bare `<base>`.
fn assemble_version(base: &str, git_short_hash: Option<&str>, head_on_release_tag: bool) -> String {
    match git_short_hash {
        Some(hash) if !head_on_release_tag => format!("{base}-dev+{hash}"),
        _ => base.to_string(),
    }
}
