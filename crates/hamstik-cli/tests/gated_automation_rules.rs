// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Gate guard for CLI-77 workflow automation rules.
//!
//! The feature is deliberately gated on a documented Hamstik Public API event
//! or subscription surface. Until that exists the CLI must not grow a `rule`
//! command or any event-stream behavior. These tests fail loudly if the gate
//! is crossed without the design/documentation work that a real implementation
//! requires.

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
fn gated_design_stub_documents_scope_triggers_and_non_goals() {
    let design = repo_file("design/AUTOMATION_RULES.md");
    for required in [
        "Gate condition",
        "Outcome",
        "Scope",
        "Triggers",
        "Non-goals",
        "Activation checklist",
        "rule list",
        "rule create",
        "rule delete",
        "event subscription",
        "manual trigger",
        "no background daemon",
        "polling loop",
        "server event subscription",
    ] {
        assert!(
            design.to_lowercase().contains(&required.to_lowercase()),
            "design/AUTOMATION_RULES.md lost `{required}`; the gated stub must \
             document scope, triggers, and non-goals"
        );
    }
    // The gate is explicitly tied to the Public API contract.
    assert!(
        design.contains("openapi/hamstik-v1.json") && design.contains("/api/v1"),
        "the stub must name the Public API contract that gates the feature"
    );
}

/// No `hamstik rule` command may exist while the gate is closed.
#[test]
fn no_rule_command_before_the_gate_opens() {
    let command = Cli::command();
    assert!(
        command.find_subcommand("rule").is_none(),
        "`hamstik rule` must not exist until the Public API exposes a documented \
         event/subscription surface (see design/AUTOMATION_RULES.md)"
    );
}

/// The design docs and root README cross-reference the gated stub.
#[test]
fn gated_stub_is_referenced_from_authoritative_docs() {
    for (path, marker) in [
        ("design/SPEC.md", "design/AUTOMATION_RULES.md"),
        ("design/PRD.md", "design/AUTOMATION_RULES.md"),
        ("README.md", "design/AUTOMATION_RULES.md"),
    ] {
        let text = repo_file(path);
        assert!(
            text.contains(marker),
            "{path} must link the gated automation-rules design stub"
        );
    }
}
