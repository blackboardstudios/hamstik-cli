// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Build identity for release/version surfaces (CLI-27).
//!
//! Emits two compile-time constants consumed by `hamstik version` via
//! `crates/hamstik-cli/src/build_info.rs`:
//!
//! - `HAMSTIK_BUILD_TARGET`: the Rust target triple being compiled for;
//! - `HAMSTIK_BUILD_COMMIT`: the source commit, from the
//!   `HAMSTIK_BUILD_COMMIT` override (release CI) or `git rev-parse HEAD`.
//!
//! Both values are deterministic functions of the source tree and target —
//! no timestamps — so release builds stay reproducible from repository
//! contents. A missing git database (e.g. a source tarball build) degrades to
//! `unknown` and never fails the build.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=HAMSTIK_BUILD_COMMIT");
    // The whole git database (a directory, or a gitdir pointer file inside a
    // worktree) changes with the checked-out commit; cargo re-runs this
    // script but only recompiles dependents when the emitted values change.
    println!("cargo:rerun-if-changed=../../.git");

    println!(
        "cargo:rustc-env=HAMSTIK_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap_or_else(|_| "unknown-target".to_string())
    );
    println!("cargo:rustc-env=HAMSTIK_BUILD_COMMIT={}", resolve_commit());
}

/// The source commit this build is produced from.
///
/// Precedence: an explicit non-empty `HAMSTIK_BUILD_COMMIT` override (used by
/// release automation), then `git rev-parse HEAD` when a git checkout is
/// available, else `unknown`. A failed lookup must never fail the build.
fn resolve_commit() -> String {
    if let Ok(commit) = std::env::var("HAMSTIK_BUILD_COMMIT") {
        let commit = commit.trim();
        if !commit.is_empty() {
            return commit.to_string();
        }
    }
    match Command::new("git").args(["rev-parse", "HEAD"]).output() {
        Ok(output) if output.status.success() => {
            let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if commit.is_empty() {
                "unknown".to_string()
            } else {
                commit
            }
        }
        _ => "unknown".to_string(),
    }
}
