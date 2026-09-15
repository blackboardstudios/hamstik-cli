// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Compile-time build identity for the release/version surfaces.
//!
//! Every value is a deterministic function of the source tree and the build
//! target: no timestamps, no environment-dependent release names. `COMMIT`
//! degrades to [`UNKNOWN_COMMIT`] when the build ran outside a git checkout
//! (e.g. from a source tarball), keeping the `version` surfaces stable.
//!
//! Consumed by the `hamstik version` command (`--json` fields and the human
//! build-identity lines) and by `hamstik --version`/`-V` (version only). The
//! `User-Agent` deliberately stays `hamstik-cli/<VERSION>`: the API contract
//! identifies the CLI by semantic version, not by build identity.

/// The build constant emitted by `build.rs` when no commit is known.
pub const UNKNOWN_COMMIT: &str = "unknown";

/// The CLI semantic version from the single version source
/// (`[workspace.package]` in the root `Cargo.toml`, design/VERSIONING.md §1).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The Rust target triple the binary was compiled for (e.g.
/// `x86_64-unknown-linux-gnu`).
pub const TARGET: &str = env!("HAMSTIK_BUILD_TARGET");

/// The full source commit the binary was built from, or [`UNKNOWN_COMMIT`].
pub const COMMIT: &str = env!("HAMSTIK_BUILD_COMMIT");

/// The short (12-character) commit for human surfaces, or [`UNKNOWN_COMMIT`].
#[must_use]
pub fn short_commit() -> &'static str {
    if COMMIT == UNKNOWN_COMMIT {
        return UNKNOWN_COMMIT;
    }
    COMMIT.get(..12).unwrap_or(COMMIT)
}

/// True when a real source commit is embedded in this build.
#[must_use]
pub fn commit_known() -> bool {
    COMMIT != UNKNOWN_COMMIT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_is_a_nonempty_triple() {
        assert!(!TARGET.is_empty());
        assert!(!TARGET.contains('\n'));
    }

    #[test]
    fn version_matches_cargo_package_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn short_commit_is_twelve_characters_or_unknown() {
        if commit_known() {
            assert_eq!(short_commit().len(), 12, "full commit: {COMMIT}");
            assert!(COMMIT.starts_with(short_commit()));
            assert!(COMMIT.bytes().all(|b| b.is_ascii_hexdigit()));
        } else {
            assert_eq!(COMMIT, UNKNOWN_COMMIT);
            assert_eq!(short_commit(), UNKNOWN_COMMIT);
        }
    }
}
