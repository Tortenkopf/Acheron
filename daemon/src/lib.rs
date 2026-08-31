// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright © 2026 Justin Milatz

/// The Daemon's runtime version string (ticket 99), stamped at compile time
/// by `build.rs`: `1.0.1` for a release-tag or tarball build, or
/// `1.0.1-dev+<short-hash>` for any other git checkout. Reported to the GUI
/// as `GetState()`'s `daemon_version` key so the About dialog (ticket 102)
/// can show it, and any drift from the GUI's own `__version__` shows up in
/// bug reports.
pub const VERSION: &str = env!("ACHERON_VERSION");

pub mod capture;
pub mod command;
pub mod config;
pub mod dbus;
pub mod dispatch;
pub mod edit;
pub mod executor;
pub mod injector;
pub mod input;

/// The GUI-mirror contract fixture generator (post-release ticket 06) — a
/// single golden-file `#[test]` deriving the daemon's device catalogs and
/// `config::validate` verdicts and checking `daemon/contract/daemon-schema.json`
/// against them. Test-only: it drives `config::validate` and nothing links
/// it into the running daemon.
#[cfg(test)]
pub(crate) mod schema;

#[cfg(test)]
mod build_version_tests {
    include!("build_version.rs");

    #[test]
    fn a_git_checkout_off_the_release_tag_gets_a_dev_suffix() {
        assert_eq!(
            assemble_version("1.0.0", Some("abc1234"), false),
            "1.0.0-dev+abc1234"
        );
    }

    #[test]
    fn a_checkout_of_the_release_tag_gets_the_bare_version() {
        assert_eq!(assemble_version("1.0.0", Some("abc1234"), true), "1.0.0");
    }

    #[test]
    fn a_tarball_with_no_git_gets_the_bare_version() {
        assert_eq!(assemble_version("1.0.0", None, false), "1.0.0");
    }

    #[test]
    fn the_stamped_version_reaches_the_crate_and_starts_from_the_manifest_version() {
        let base = env!("CARGO_PKG_VERSION");
        assert!(
            super::VERSION == base || super::VERSION.starts_with(&format!("{base}-dev+")),
            "unexpected stamped VERSION: {:?}",
            super::VERSION
        );
    }
}
