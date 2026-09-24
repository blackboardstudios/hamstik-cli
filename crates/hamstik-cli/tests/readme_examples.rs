// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! README example verification harness tests.
//!
//! The Python harness (`scripts/readme_examples.py`) is what CI runs; these
//! tests keep it honest: the README must contain the required executable
//! example families, and the harness itself must exist and parse Markdown
//! structurally.

use std::path::Path;

fn repo_file(relative: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../")
            .join(relative),
    )
    .unwrap_or_else(|error| panic!("{relative} must be part of the repository: {error}"))
}

fn readme_source() -> String {
    repo_file("README.md")
}

fn harness_source() -> String {
    repo_file("scripts/readme_examples.py")
}

/// Extracts fenced shell blocks with structural (not grep) parsing.
fn shell_blocks(text: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut language: Option<String> = None;
    let mut index = 0usize;
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if language.is_none() {
                language = Some(rest.to_string());
                index += 1;
                lines.clear();
            } else {
                let lang = language.take().unwrap_or_default();
                if matches!(lang.as_str(), "bash" | "sh" | "shell") {
                    blocks.push((index, lines.join("\n")));
                }
            }
        } else if language.is_some() {
            lines.push(line.to_string());
        }
    }
    let _ = index;
    blocks
}

/// The harness script exists, parses fenced blocks structurally, and never
/// contacts the network.
#[test]
fn readme_examples_harness_is_in_place_and_structural() {
    let text = readme_source();
    assert!(
        !shell_blocks(&text).is_empty(),
        "README must keep its executable shell examples"
    );

    let source = harness_source();
    for required in [
        "def extract_blocks",
        "def logical_commands",
        "def parse_check",
        "shlex.split",
        "--help",
        "FENCE_PATTERN",
    ] {
        assert!(
            source.contains(required),
            "readme_examples.py lost `{required}`; the harness must stay structural"
        );
    }
    // No network access: the harness must not spawn curl or open sockets.
    assert!(
        !source.contains("urllib") && !source.contains("curl") && !source.contains("requests"),
        "the harness must remain fully offline"
    );
}

/// The README documents the harness workflow for contributors.
#[test]
fn readme_documents_the_example_workflow() {
    let text = readme_source();
    assert!(
        text.contains("readme_examples.py"),
        "README must document the README example verification workflow"
    );
}

/// The required example surfaces are present in the README.
#[test]
fn readme_covers_required_example_sections() {
    let text = readme_source();
    for required in [
        "hamstik auth login --with-token",
        "hamstik context init",
        "hamstik me --json",
        "hamstik org list",
        "hamstik project create",
        "hamstik sprint transition",
        "hamstik work list --status",
        "hamstik work mine",
        "hamstik squeakql validate",
        "hamstik work create",
        "hamstik work transition",
        "hamstik work bulk create",
        "hamstik label create",
        "hamstik attribute list",
        "hamstik attribute create",
        "hamstik attribute option add",
        "hamstik attribute project enable",
        "--attribute-boolean verified=false",
        "attribute_customer IS NULL",
        "attribute_has_any('product_area', 'search', 'mobile')",
        "--fields key,title,attributes",
        "hamstik work link add",
        "hamstik work comment add",
        "hamstik work attachment upload",
        "hamstik user view",
        "hamstik work edit HAM-42 --priority high --dry-run",
        "hamstik --no-input --json auth status",
    ] {
        assert!(
            text.contains(required),
            "README lost the required example `{required}`; restore it or update this fixture deliberately"
        );
    }
}

/// No README example may contain credential-shaped content; the documented
/// env-var indirection is the only allowed secret shape.
#[test]
fn readme_examples_contain_no_secrets() {
    let text = readme_source();
    let forbidden = [
        "Bearer ",
        "Authorization:",
        "HAMSTIK_TOKEN=hs_pat_",
        "ghp_",
        "xoxb-",
    ];
    for marker in &forbidden {
        assert!(
            !text.contains(marker),
            "README must not embed credential-shaped content: {marker}"
        );
    }
    assert!(
        text.contains("$HAMSTIK_PAT"),
        "the CI example should demonstrate the secret indirection pattern"
    );
}
