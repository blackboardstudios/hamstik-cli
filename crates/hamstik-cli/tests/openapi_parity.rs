// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! CLI-side OpenAPI parity guard: every `cli` path in `openapi/api-parity.json`
//! must resolve to a real command (or command alias) in the built clap tree.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;

use clap::CommandFactory;

/// Every command path and alias path in the CLI, space-joined.
fn cli_paths() -> BTreeSet<String> {
    fn walk(command: &clap::Command, prefix: &str, paths: &mut BTreeSet<String>) {
        let path = if prefix.is_empty() {
            command.get_name().to_string()
        } else {
            format!("{prefix} {}", command.get_name())
        };
        paths.insert(path.clone());
        for alias in command.get_all_aliases() {
            paths.insert(if prefix.is_empty() {
                alias.to_string()
            } else {
                format!("{prefix} {alias}")
            });
        }
        for subcommand in command.get_subcommands() {
            walk(subcommand, &path, paths);
        }
    }

    let mut paths = BTreeSet::new();
    let root = hamstik_cli::args::Cli::command();
    for subcommand in root.get_subcommands() {
        walk(subcommand, "", &mut paths);
    }
    paths
}

#[test]
fn every_manifest_cli_path_resolves() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../../../openapi/api-parity.json"))
            .expect("parse parity manifest");
    let available = cli_paths();

    let mut unresolved = Vec::new();
    for operation in manifest["operations"]
        .as_array()
        .expect("manifest operations array")
    {
        for command in operation["cli"].as_array().expect("cli array") {
            let path = command.as_str().expect("cli path string");
            if !available.contains(path) {
                unresolved.push(format!(
                    "{}: `{path}`",
                    operation["operationId"].as_str().unwrap_or("?")
                ));
            }
        }
    }
    assert!(
        unresolved.is_empty(),
        "manifest CLI paths do not resolve in the command tree: {unresolved:#?}"
    );
}
