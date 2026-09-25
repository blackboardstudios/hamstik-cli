// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `hamstik agent validate` harness checks.

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::MockServer;

/// The canonical bundled skill shipped in the repository.
fn canonical_skill() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../skills/hamstik/SKILL.md")
        .canonicalize()
        .expect("canonical skill path")
}

/// The bundled OpenAPI snapshot shipped in the repository.
fn bundled_openapi() -> Value {
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../openapi/hamstik-v1.json"),
    )
    .expect("bundled snapshot");
    serde_json::from_str(&text).expect("valid snapshot JSON")
}

fn base(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env_remove("HAMSTIK_HOST");
    cmd.env_remove("HAMSTIK_TOKEN");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.env_remove("NO_COLOR");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn check<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == id)
        .unwrap_or_else(|| panic!("missing check {id} in {:?}", body["checks"]))
}

/// A clean harness with the canonical skill exits 0 and skips the network
/// comparison under `--offline`.
#[test]
fn clean_harness_exits_zero() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        "version = 2\n[settings]\neditor = \"vim\"\n",
    )
    .unwrap();
    let output = base(&dir)
        .args(["--no-input", "--json", "agent", "validate", "--offline"])
        .arg(canonical_skill())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "clean harness: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["command"], "agent validate");
    assert_eq!(body["ok"], true);
    assert_eq!(body["exitCode"], 0);
    assert_eq!(check(&body, "skill.metadata")["status"], "pass");
    assert_eq!(check(&body, "skill.commands")["status"], "pass");
    assert_eq!(check(&body, "config.credentials")["status"], "pass");
    assert_eq!(check(&body, "openapi.freshness")["status"], "skipped");
}

/// An installed skill that requires a newer CLI fails with exit 1.
#[test]
fn outdated_skill_warns_with_general_exit_code() {
    let dir = TempDir::new().unwrap();
    let skill = dir.path().join("SKILL.md");
    std::fs::write(
        &skill,
        "---\nname: hamstik\nmetadata:\n  skill-version: \"9.9.9\"\n  minimum-cli-version: \"99.0.0\"\n---\n# stale\n",
    )
    .unwrap();
    let output = base(&dir)
        .args(["--no-input", "--json", "agent", "validate", "--offline"])
        .arg(&skill)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "outdated skill exit code");
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["exitCode"], 1);
    let metadata = check(&body, "skill.metadata");
    assert_eq!(metadata["status"], "fail");
    assert!(
        metadata["detail"]
            .as_str()
            .unwrap()
            .contains("requires CLI"),
        "clear warning: {metadata}"
    );
}

/// Credential-shaped content in the config directory fails the check and is
/// reported without ever printing the value.
#[test]
fn credential_shaped_content_is_reported_without_printing() {
    let dir = TempDir::new().unwrap();
    let secret = "super-secret-token-value-1234567890";
    std::fs::write(
        dir.path().join("queries.toml"),
        format!("version = 1\ntoken = \"{secret}\"\n"),
    )
    .unwrap();
    let output = base(&dir)
        .args(["--no-input", "--json", "agent", "validate", "--offline"])
        .arg(canonical_skill())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "credential finding exit code"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains(secret) && !stderr.contains(secret),
        "credential value must never be printed"
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let credentials = check(&body, "config.credentials");
    assert_eq!(credentials["status"], "fail");
    assert!(
        credentials["detail"]
            .as_str()
            .unwrap()
            .contains("queries.toml:2"),
        "finding names the location: {credentials}"
    );
}

/// The documented `agent doctor` alias invokes the same validation.
#[test]
fn agent_doctor_alias_runs_validation() {
    let dir = TempDir::new().unwrap();
    let output = base(&dir)
        .args(["--no-input", "--json", "agent", "doctor", "--offline"])
        .arg(canonical_skill())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["command"], "agent validate");
}

/// When online and the live contract matches the snapshot, the freshness
/// check passes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_contract_matching_snapshot_passes() {
    let server = MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/v1/openapi.json"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(bundled_openapi()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&dir)
        .env("HAMSTIK_HOST", server.uri())
        .args(["--no-input", "--json", "agent", "validate"])
        .arg(canonical_skill())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "matching contract: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(check(&body, "openapi.freshness")["status"], "pass");
}

/// A live contract that differs from the bundled snapshot is a stale snapshot
/// and fails with the general exit code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_snapshot_warns_with_general_exit_code() {
    let server = MockServer::start().await;
    let mut live = bundled_openapi();
    live["paths"]["/api/v1/extra"]["get"] = json!({"operationId": "extraOperation"});
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/v1/openapi.json"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(live))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&dir)
        .env("HAMSTIK_HOST", server.uri())
        .args(["--no-input", "--json", "agent", "validate"])
        .arg(canonical_skill())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "stale snapshot exit code");
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let freshness = check(&body, "openapi.freshness");
    assert_eq!(freshness["status"], "fail");
    assert!(
        freshness["detail"].as_str().unwrap().contains("differs"),
        "clear warning: {freshness}"
    );
}

/// An unreachable host skips the freshness comparison instead of failing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_live_contract_is_skipped() {
    let dir = TempDir::new().unwrap();
    let output = base(&dir)
        .env("HAMSTIK_HOST", "http://127.0.0.1:1")
        .args(["--no-input", "--json", "agent", "validate"])
        .arg(canonical_skill())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(check(&body, "openapi.freshness")["status"], "skipped");
}
