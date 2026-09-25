// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik commands --cookbook` contract tests.
//!
//! The cookbook is the marker-delimited section of the bundled Agent Skill, so
//! these tests keep the CLI output, the skill, and the command manifest in
//! agreement: safe automation defaults, placeholder identifiers, no
//! credential-shaped content, and the manifest advertising the new flag.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;

fn plain(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.current_dir(dir.path());
    cmd
}

fn skill_source() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/hamstik/SKILL.md"),
    )
    .expect("bundled skill")
}

fn cookbook_json(dir: &TempDir) -> Value {
    let output = plain(dir)
        .args(["commands", "--cookbook", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The default surface is copy-pasteable text covering the whole loop.
#[test]
fn cookbook_prints_the_common_loop_with_safe_defaults() {
    let dir = TempDir::new().unwrap();
    let output = plain(&dir)
        .args(["commands", "--cookbook"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "Hamstik API cookbook",
        "work view <ITEM-KEY>",
        "work search",
        "work edit <ITEM-KEY>",
        "work transitions <ITEM-KEY>",
        "work transition",
        "work comment add",
        "--json --no-input",
    ] {
        assert!(stdout.contains(expected), "cookbook misses {expected}");
    }
    assert!(!stdout.contains("```"), "cookbook must not print fences");
}

/// `--json` carries the same steps with a stable envelope.
#[test]
fn cookbook_json_is_versioned_and_uses_safe_defaults() {
    let dir = TempDir::new().unwrap();
    let body = cookbook_json(&dir);
    assert_eq!(body["cookbookVersion"], 1);
    assert_eq!(body["source"], "skills/hamstik/SKILL.md");
    let steps = body["steps"].as_array().unwrap();
    assert!(!steps.is_empty());
    for step in steps {
        assert!(
            step["title"]
                .as_str()
                .is_some_and(|title| !title.is_empty())
        );
        let commands = step["commands"].as_array().unwrap();
        assert!(!commands.is_empty());
        for command in commands {
            let command = command.as_str().unwrap();
            assert!(command.starts_with("hamstik "), "{command}");
            assert!(command.contains("--json"), "unsafe output mode: {command}");
            assert!(
                command.contains("--no-input"),
                "prompting allowed: {command}"
            );
        }
    }
}

/// The structured document names every placeholder a caller must replace.
#[test]
fn cookbook_json_lists_the_placeholders() {
    let dir = TempDir::new().unwrap();
    let body = cookbook_json(&dir);
    let placeholders: Vec<&str> = body["placeholders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    for expected in ["<ORG>", "<KEY>", "<ITEM-KEY>", "<COMMENT.md>"] {
        assert!(
            placeholders.contains(&expected),
            "missing placeholder {expected}"
        );
    }
}

/// Every printed example line comes verbatim from the bundled skill.
#[test]
fn cookbook_matches_the_bundled_agent_skill() {
    let dir = TempDir::new().unwrap();
    let output = plain(&dir)
        .args(["commands", "--cookbook"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let skill = skill_source();
    let body = stdout
        .split_once("defaults.\n\n")
        .map(|(_, body)| body)
        .expect("cookbook header");
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        assert!(skill.contains(line), "cookbook line not in skill: {line}");
    }
}

/// Examples never embed credentials or concrete identifiers.
#[test]
fn cookbook_contains_no_credentials_or_real_identifiers() {
    let dir = TempDir::new().unwrap();
    let output = plain(&dir)
        .args(["commands", "--cookbook", "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    for forbidden in [
        "Bearer ",
        "Authorization:",
        "HAMSTIK_TOKEN=",
        "hs_pat_",
        "ghp_",
    ] {
        assert!(!stdout.contains(forbidden), "cookbook leaks {forbidden}");
    }
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!body["placeholders"].as_array().unwrap().is_empty());
}

/// The manifest advertises the new flag for completions and reference docs.
#[test]
fn manifest_still_lists_the_cookbook_flag() {
    let dir = TempDir::new().unwrap();
    let output = plain(&dir).args(["commands", "--json"]).output().unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let entry = body["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["command"] == json!("hamstik commands"))
        .expect("commands entry");
    assert!(
        entry["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .any(|argument| argument.get("long") == Some(&json!("--cookbook"))),
        "manifest misses --cookbook"
    );
}
