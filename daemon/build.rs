// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

//! Stamps the Daemon's runtime version string into the `ACHERON_VERSION`
//! compile-time env var (ticket 99), read back by `acheron_daemon::VERSION`.
//!
//! A build inside a git checkout that is *not* sitting exactly on a
//! `v<version>` release tag is a dev build and gets `<version>-dev+<short-hash>`;
//! a release-tag checkout or a source tarball with no git gets the bare
//! `CARGO_PKG_VERSION`. This keeps drift between a maintainer's working copy
//! and a real release visible in bug reports without a separate build
//! profile. An explicit `ACHERON_VERSION` in the environment overrides the
//! probing entirely, so a packager can pin the string without patching source.

use std::path::Path;
use std::process::Command;

include!("src/build_version.rs");

fn main() {
    let base = std::env::var("CARGO_PKG_VERSION").expect("Cargo always sets CARGO_PKG_VERSION");

    let version = match std::env::var("ACHERON_VERSION") {
        Ok(explicit) if !explicit.is_empty() => explicit,
        _ => assemble_version(
            &base,
            git_short_hash().as_deref(),
            git_head_on_release_tag(&base),
        ),
    };
    println!("cargo:rustc-env=ACHERON_VERSION={version}");

    // Re-run when the checked-out commit moves so the stamped hash keeps up.
    // Only emitted when the paths exist — a tarball build has no `.git`, and
    // a `rerun-if-changed` on a missing path forces an unconditional rebuild.
    for path in ["../.git/HEAD", "../.git/refs", "../.git/packed-refs"] {
        if Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!("cargo:rerun-if-changed=src/build_version.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=ACHERON_VERSION");
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn git_short_hash() -> Option<String> {
    git(&["rev-parse", "--short", "HEAD"])
}

/// True only when `HEAD` is *exactly* the `v<base>` tag (a checkout of the
/// release commit, not N commits past it). A bare `<base>` tag is accepted
/// too, in case the project ever drops the `v` prefix.
fn git_head_on_release_tag(base: &str) -> bool {
    match git(&["describe", "--tags", "--exact-match", "HEAD"]) {
        Some(tag) => tag == format!("v{base}") || tag == base,
        None => false,
    }
}
