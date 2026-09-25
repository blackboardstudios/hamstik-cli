// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Gate guard for CLI-78 webhook management.
//!
//! The feature is deliberately gated on a documented Hamstik Public API webhook
//! or subscription-management surface. Until that exists the CLI must not grow
//! a `webhook` command — not even a passthrough over `api request`. These tests
//! fail loudly if the gate is crossed without the design/documentation work
//! that a real implementation requires.

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
fn gated_design_stub_documents_scope_and_non_goals() {
    let design = repo_file("design/WEBHOOKS.md");
    for required in [
        "Gate condition",
        "Outcome",
        "Scope",
        "Non-goals",
        "Activation checklist",
        "webhook list",
        "webhook create",
        "webhook delete",
        "subscription",
        "no command surface",
        "passthrough-only",
        "api request",
        "server-owned",
    ] {
        assert!(
            design.to_lowercase().contains(&required.to_lowercase()),
            "design/WEBHOOKS.md lost `{required}`; the gated stub must \
             document scope and non-goals"
        );
    }
    // The gate is explicitly tied to the Public API contract.
    assert!(
        design.contains("openapi/hamstik-v1.json") && design.contains("/api/v1"),
        "the stub must name the Public API contract that gates the feature"
    );
    assert!(
        design.contains("openapi/api-parity.json"),
        "the stub must require the parity ledger before the feature ships"
    );
}

/// No `hamstik webhook` command may exist while the gate is closed.
#[test]
fn no_webhook_command_before_the_gate_opens() {
    let command = Cli::command();
    assert!(
        command.find_subcommand("webhook").is_none(),
        "`hamstik webhook` must not exist until the Public API exposes \
         documented webhook/subscription operations (see design/WEBHOOKS.md)"
    );
}

/// The design docs and root README cross-reference the gated stub.
#[test]
fn gated_stub_is_referenced_from_authoritative_docs() {
    for (path, marker) in [
        ("design/SPEC.md", "design/WEBHOOKS.md"),
        ("design/PRD.md", "design/WEBHOOKS.md"),
        ("README.md", "design/WEBHOOKS.md"),
    ] {
        let text = repo_file(path);
        assert!(
            text.contains(marker),
            "{path} must link the gated webhook-management design stub"
        );
    }
}
