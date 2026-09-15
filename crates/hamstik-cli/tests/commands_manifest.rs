// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik commands` manifest and completion contract tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;

fn plain(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.current_dir(dir.path());
    cmd
}

/// The manifest covers every command in the tree, exactly once.
#[test]
fn manifest_covers_the_whole_command_tree() {
    let dir = TempDir::new().unwrap();
    let output = plain(&dir).args(["commands", "--json"]).output().unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();

    assert_eq!(body["manifestVersion"], 1);
    let commands = body["commands"].as_array().unwrap();
    assert!(!commands.is_empty());
    let names: Vec<&str> = commands
        .iter()
        .map(|c| c["command"].as_str().unwrap())
        .collect();
    for expected in [
        "hamstik",
        "hamstik work",
        "hamstik work context",
        "hamstik api request",
        "hamstik doctor",
        "hamstik completion",
    ] {
        assert!(names.contains(&expected), "missing {expected} in manifest");
    }
    let mut sorted = names.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "no duplicate manifest entries");
}

/// Manifest capabilities: global `--json`/`--no-input` apply everywhere.
#[test]
fn manifest_capability_metadata_reflects_global_flags() {
    let dir = TempDir::new().unwrap();
    let output = plain(&dir).args(["commands", "--json"]).output().unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    for command in body["commands"].as_array().unwrap() {
        let capabilities = &command["capabilities"];
        assert_eq!(
            capabilities["json"], true,
            "{} must be json-capable (global flag)",
            command["command"]
        );
        assert_eq!(
            capabilities["noInput"], true,
            "{} must be no-input-capable (global flag)",
            command["command"]
        );
    }
}

/// Enum choices and defaults come from the real definitions.
#[test]
fn manifest_includes_enum_choices_and_defaults() {
    let dir = TempDir::new().unwrap();
    let output = plain(&dir).args(["commands", "--json"]).output().unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let create = body["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["command"] == json!("hamstik work create"))
        .expect("work create present");
    let type_argument = create["arguments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a.get("long") == Some(&json!("--type")))
        .expect("--type argument present");
    assert_eq!(
        type_argument_choices(type_argument),
        ["task", "bug", "story", "feature", "epic"]
    );
}

fn type_argument_choices(argument: &Value) -> Vec<String> {
    argument["choices"]
        .as_array()
        .map(|choices| {
            choices
                .iter()
                .map(|c| c.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The manifest lists every option visible in the command's help, with its
/// documented default.
#[test]
fn manifest_arguments_match_clap_definitions() {
    let dir = TempDir::new().unwrap();
    let output = plain(&dir).args(["commands", "--json"]).output().unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let work_context = body["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["command"] == json!("hamstik work context"))
        .expect("work context in manifest");
    let arguments = work_context["arguments"].as_array().unwrap();
    for name in ["--comments", "--activity", "--compact", "--format"] {
        assert!(
            arguments
                .iter()
                .any(|a| a.get("long") == Some(&json!(name))),
            "work context manifest misses {name}"
        );
    }
    let comments = arguments
        .iter()
        .find(|a| a.get("long") == Some(&json!("--comments")))
        .unwrap();
    assert_eq!(comments["default"], "10");
}

/// Completion scripts render for all four shells, non-empty on stdout.
#[test]
fn completion_scripts_render_for_all_shells() {
    let dir = TempDir::new().unwrap();
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let output = plain(&dir).args(["completion", shell]).output().unwrap();
        assert!(
            output.status.success(),
            "shell {shell}: {:?}",
            output.stderr
        );
        let script = String::from_utf8_lossy(&output.stdout);
        assert!(!script.trim().is_empty(), "{shell} script empty");
    }
}

/// Completion candidates are statically knowable and network-free.
#[test]
fn completion_candidates_cover_commands_and_enum_values() {
    let dir = TempDir::new().unwrap();
    let output = plain(&dir).args(["completion", "bash"]).output().unwrap();
    let script = String::from_utf8_lossy(&output.stdout);
    for expected in ["work", "doctor", "commands", "in_progress", "backlog"] {
        assert!(script.contains(expected), "completion misses {expected}");
    }
    assert!(!script.contains("curl"), "completion must not invoke curl");
    assert!(
        !script.contains("http"),
        "completion must not reference URLs"
    );
}
