// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Activation guard for CLI-78 webhook management.

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

/// The activated design records the contract boundary and non-goals.
#[test]
fn activated_design_documents_scope_and_non_goals() {
    let design = repo_file("design/WEBHOOKS.md");
    for required in [
        "Status:",
        "Implemented",
        "Gate activation",
        "Command surface",
        "Non-goals",
        "webhook list",
        "webhook create",
        "webhook delete",
        "webhook redeliver",
        "api request",
        "server remains authoritative",
    ] {
        assert!(
            design.to_lowercase().contains(&required.to_lowercase()),
            "design/WEBHOOKS.md lost `{required}`"
        );
    }
    // The gate is explicitly tied to the Public API contract.
    assert!(
        design.contains("openapi/hamstik-v1.json") && design.contains("/api/v1"),
        "the design must name the activated Public API contract"
    );
    assert!(
        design.contains("openapi/api-parity.json"),
        "the design must name the parity ledger"
    );
}

/// The full first-class command family exists now that the gate is open.
#[test]
fn webhook_command_exists_after_gate_activation() {
    let command = Cli::command();
    let webhook = command.find_subcommand("webhook").expect("webhook command");
    for child in [
        "list",
        "view",
        "create",
        "edit",
        "delete",
        "rotate",
        "test",
        "deliveries",
        "redeliver",
    ] {
        assert!(
            webhook.find_subcommand(child).is_some(),
            "missing webhook {child}"
        );
    }
}

/// The design docs and root README cross-reference the activated design.
#[test]
fn activated_design_is_referenced_from_authoritative_docs() {
    for (path, marker) in [
        ("design/SPEC.md", "design/WEBHOOKS.md"),
        ("design/PRD.md", "design/WEBHOOKS.md"),
        ("README.md", "design/WEBHOOKS.md"),
    ] {
        let text = repo_file(path);
        assert!(
            text.contains(marker),
            "{path} must link the webhook-management design"
        );
    }
}
