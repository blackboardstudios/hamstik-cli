// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Gate guard for CLI-79 self update with stable/prerelease channels.
//!
//! The feature is deliberately gated on the packaging/release milestone and on
//! the verified-replacement prerequisites recorded in design/SELF_UPDATE.md.
//! Until that gate opens the CLI must not grow a `self` command, must not make
//! silent update network calls, and must not emit update notices. These tests
//! fail loudly if the gate is crossed without the design/documentation work a
//! real implementation requires.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use clap::CommandFactory;
use hamstik_cli::args::Cli;

fn repo_file(relative: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../")
            .join(relative),
    )
    .unwrap_or_else(|error| panic!("{relative} must be part of the repository: {error}"))
}

/// The gated design stub exists and covers the acceptance criteria.
#[test]
fn gated_design_stub_documents_scope_and_safety_constraints() {
    let design = repo_file("design/SELF_UPDATE.md");
    for required in [
        "Gate condition",
        "Outcome",
        "Scope",
        "Non-goals",
        "Activation checklist",
        "hamstik self update",
        "--channel stable",
        "--channel prerelease",
        "stable",
        "prerelease",
        "verified package replacement",
        "opt-in",
        "opt-out",
        "no telemetry",
        "no silent network",
        "packaging/release milestone",
    ] {
        assert!(
            design.to_lowercase().contains(&required.to_lowercase()),
            "design/SELF_UPDATE.md lost `{required}`; the gated stub must \
             document scope, channels, and the safety constraints"
        );
    }
    // The gate is explicitly tied to the packaging/release milestone and its
    // integrity documents, not to the Public API.
    for required in [
        "design/RELEASE.md",
        "design/SIGNING.md",
        "design/INSTALL.md",
    ] {
        assert!(
            design.contains(required),
            "the stub must name `{required}` as a prerequisite for verified replacement"
        );
    }
}

/// No `hamstik self` and no `hamstik update` command may exist while the gate
/// is closed.
#[test]
fn no_self_update_command_before_the_gate_opens() {
    let command = Cli::command();
    assert!(
        command.find_subcommand("self").is_none(),
        "`hamstik self` must not exist until the packaging/release milestone \
         opens the gate (see design/SELF_UPDATE.md)"
    );
    assert!(
        command.find_subcommand("update").is_none(),
        "a root `hamstik update` command must not exist; the gated surface is \
         `hamstik self update` (see design/SELF_UPDATE.md)"
    );
}

/// The design docs and root README cross-reference the gated stub.
#[test]
fn gated_stub_is_referenced_from_authoritative_docs() {
    for (path, marker) in [
        ("design/SPEC.md", "design/SELF_UPDATE.md"),
        ("design/PRD.md", "design/SELF_UPDATE.md"),
        ("README.md", "design/SELF_UPDATE.md"),
    ] {
        let text = repo_file(path);
        assert!(
            text.contains(marker),
            "{path} must link the gated self-update design stub"
        );
    }
}
