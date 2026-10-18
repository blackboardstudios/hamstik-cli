// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Activation guard for CLI-77 server-side workflow automation rules.

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

/// The activated design records server ownership and non-goals.
#[test]
fn activated_design_documents_scope_and_non_goals() {
    let design = repo_file("design/AUTOMATION_RULES.md");
    for required in [
        "Status:",
        "Implemented",
        "Gate activation",
        "Command surface",
        "Non-goals",
        "rule list",
        "rule create",
        "rule delete",
        "rule preview",
        "rule accept-owner",
        "no background daemon",
        "polling loop",
        "server remains authoritative",
    ] {
        assert!(
            design.to_lowercase().contains(&required.to_lowercase()),
            "design/AUTOMATION_RULES.md lost `{required}`"
        );
    }
    // The gate is explicitly tied to the Public API contract.
    assert!(
        design.contains("openapi/hamstik-v1.json") && design.contains("/api/v1"),
        "the design must name the activated Public API contract"
    );
}

/// The first-class command family exists now that the gate is open.
#[test]
fn rule_command_exists_after_gate_activation() {
    let command = Cli::command();
    let rule = command.find_subcommand("rule").expect("rule command");
    for child in [
        "list",
        "view",
        "create",
        "edit",
        "delete",
        "runs",
        "preview",
        "toggle",
        "accept-owner",
    ] {
        assert!(
            rule.find_subcommand(child).is_some(),
            "missing rule {child}"
        );
    }
}

/// The design docs and root README cross-reference the activated design.
#[test]
fn activated_design_is_referenced_from_authoritative_docs() {
    for (path, marker) in [
        ("design/SPEC.md", "design/AUTOMATION_RULES.md"),
        ("design/PRD.md", "design/AUTOMATION_RULES.md"),
        ("README.md", "design/AUTOMATION_RULES.md"),
    ] {
        let text = repo_file(path);
        assert!(
            text.contains(marker),
            "{path} must link the automation-rules design"
        );
    }
}
