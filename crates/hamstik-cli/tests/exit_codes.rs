// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Exit-code reference tests (CLI-73).
//!
//! `hamstik commands --exit-codes` and the manifest's `exitCodes` block must
//! render the same stable table the error renderer maps onto.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

fn base(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.current_dir(dir.path());
    cmd
}

/// The machine-readable document carries the versioned envelope with all
/// eleven codes, contiguous from 0, each with name/meaning/remediation.
#[test]
fn exit_codes_json_is_versioned_and_complete() {
    let dir = TempDir::new().unwrap();
    let output = base(&dir)
        .args(["--json", "--no-input", "commands", "--exit-codes"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["exitCodesVersion"], serde_json::json!(1));
    let codes: Vec<i64> = body["exitCodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_i64().unwrap())
        .collect();
    assert_eq!(codes, (0..=10).collect::<Vec<_>>());
    for entry in body["exitCodes"].as_array().unwrap() {
        assert!(!entry["name"].as_str().unwrap().is_empty());
        assert!(!entry["meaning"].as_str().unwrap().is_empty());
        assert!(!entry["remediation"].as_str().unwrap().is_empty());
    }
    // Stable names, in documented order.
    let names: Vec<&str> = body["exitCodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "success",
            "general",
            "usage",
            "authentication",
            "authorization",
            "not_found",
            "conflict",
            "rate_limited",
            "network",
            "server",
            "configuration",
        ]
    );
}

/// The human table renders the meaning and remediation columns.
#[test]
fn exit_codes_human_table_renders() {
    let dir = TempDir::new().unwrap();
    base(&dir)
        .args(["--no-input", "commands", "--exit-codes"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("authentication"))
        .stdout(predicate::str::contains("Conflict / concurrency"))
        .stdout(predicate::str::contains("hamstik auth login"));
}

/// The exit-code table rides the shared output contract: `--format tsv`
/// projects the same four columns.
#[test]
fn exit_codes_follows_the_output_contract() {
    let dir = TempDir::new().unwrap();
    let output = base(&dir)
        .args(["--no-input", "--format", "tsv", "commands", "--exit-codes"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("0\tsuccess"));
    assert!(stdout.contains("6\tconflict"));
}

/// The `commands --json` manifest embeds the same `exitCodes` block, so a
/// single manifest fetch yields both the command surface and the failure
/// schema.
#[test]
fn manifest_embeds_the_exit_codes_block() {
    let dir = TempDir::new().unwrap();
    let output = base(&dir)
        .args(["--json", "--no-input", "commands"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let manifest_codes: Vec<i64> = body["exitCodes"]
        .as_array()
        .expect("manifest carries exitCodes")
        .iter()
        .map(|e| e["code"].as_i64().unwrap())
        .collect();
    assert_eq!(manifest_codes, (0..=10).collect::<Vec<_>>());
    // Same document as the dedicated flag, modulo the version envelope.
    let dedicated: Value = {
        let output = base(&dir)
            .args(["--json", "--no-input", "commands", "--exit-codes"])
            .output()
            .unwrap();
        serde_json::from_slice(&output.stdout).unwrap()
    };
    assert_eq!(body["exitCodes"], dedicated["exitCodes"]);
}
