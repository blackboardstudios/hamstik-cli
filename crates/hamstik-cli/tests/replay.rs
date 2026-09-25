// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Failed-request journal and `replay` contract tests.
//!
//! A server failure must leave a local, redacted journal entry carrying the
//! server request id; `replay` must surface it without any network access; and
//! no entry may ever contain a token, body, or query string.
//!
//! `HAMSTIK_REQUEST_JOURNAL` pins the journal inside each test's temporary
//! directory so the tests never touch a real user's journal.

use std::fs;

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "super-secret-token";

fn base(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_REQUEST_JOURNAL", journal_path(dir));
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn journal_path(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("state").join("request-journal.log")
}

fn read_journal(dir: &TempDir) -> String {
    match fs::read_to_string(journal_path(dir)) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => panic!("cannot read journal: {err}"),
    }
}

fn first_entry(dir: &TempDir) -> Value {
    let text = read_journal(dir);
    let line = text
        .lines()
        .next()
        .unwrap_or_else(|| panic!("journal is empty: {text:?}"));
    serde_json::from_str(line).unwrap()
}

async fn mount_me_error(server: &MockServer, status: u16, request_id: &str) {
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(status)
                .insert_header("X-Request-Id", request_id)
                .set_body_json(json!({
                    "error": {"code": "INTERNAL_ERROR", "message": "boom"}
                })),
        )
        .mount(server)
        .await;
}

/// A 5xx failure produces a retrievable journal entry carrying its request id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_error_is_journaled_with_request_id() {
    let server = MockServer::start().await;
    mount_me_error(&server, 500, "req-5xx").await;
    let dir = TempDir::new().unwrap();

    let output = base(&dir)
        .env("HAMSTIK_HOST", server.uri())
        .env("HAMSTIK_TOKEN", TOKEN)
        .args(["--json", "me"])
        .output()
        .unwrap();
    assert!(!output.status.success());

    let entry = first_entry(&dir);
    assert_eq!(entry["method"], "GET");
    assert_eq!(entry["path"], "/api/v1/me");
    assert_eq!(entry["status"], 500);
    assert_eq!(entry["requestId"], "req-5xx");
    assert!(entry["durationMs"].is_number());
    assert_eq!(entry["transport"], false);
    let headers = entry["headerIntent"].as_array().unwrap();
    assert!(
        headers.iter().any(|header| header == "authorization"),
        "header intent must record that auth was attached: {headers:?}"
    );
    assert!(
        headers.iter().any(|header| header == "accept"),
        "header intent must record the default Accept header: {headers:?}"
    );

    // No credential, header value, or body ever reaches the journal.
    let text = read_journal(&dir);
    assert!(!text.contains(TOKEN));
    assert!(!text.contains("Bearer"));
    assert!(!text.contains("super-secret"));
}

/// The acceptance-criteria entry is retrievable through `replay --json`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_retrieves_the_journal_entry() {
    let server = MockServer::start().await;
    mount_me_error(&server, 503, "req-replay").await;
    let dir = TempDir::new().unwrap();

    let failed = base(&dir)
        .env("HAMSTIK_HOST", server.uri())
        .env("HAMSTIK_TOKEN", TOKEN)
        .args(["--json", "me"])
        .output()
        .unwrap();
    assert!(!failed.status.success());

    let output = base(&dir)
        .args(["--json", "replay", "--last", "5"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let entries = body.as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["status"], 503);
    assert_eq!(entries[0]["requestId"], "req-replay");
    assert_eq!(entries[0]["path"], "/api/v1/me");
}

/// Client errors (4xx) are failures too and are journaled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_error_is_journaled() {
    let server = MockServer::start().await;
    mount_me_error(&server, 404, "req-404").await;
    let dir = TempDir::new().unwrap();

    let output = base(&dir)
        .env("HAMSTIK_HOST", server.uri())
        .env("HAMSTIK_TOKEN", TOKEN)
        .args(["--json", "me"])
        .output()
        .unwrap();
    assert!(!output.status.success());

    let entry = first_entry(&dir);
    assert_eq!(entry["status"], 404);
    assert_eq!(entry["requestId"], "req-404");
    assert_eq!(entry["transport"], false);
}

/// A transport failure is journaled with a null status and `transport: true`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_failure_is_journaled_without_status() {
    let dir = TempDir::new().unwrap();

    // Port 1 on loopback is refused immediately; no server is involved.
    let output = base(&dir)
        .env("HAMSTIK_HOST", "http://127.0.0.1:1")
        .env("HAMSTIK_TOKEN", TOKEN)
        .args(["--json", "me"])
        .output()
        .unwrap();
    assert!(!output.status.success());

    let entry = first_entry(&dir);
    assert_eq!(entry["method"], "GET");
    assert_eq!(entry["path"], "/api/v1/me");
    assert!(entry["status"].is_null());
    assert_eq!(entry["transport"], true);
}

/// `replay` is local-only: it works with no host, token, or context.
#[test]
fn replay_reads_the_journal_without_network_or_credentials() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(journal_path(&dir).parent().unwrap()).unwrap();
    fs::write(
        journal_path(&dir),
        format!(
            "{}\n",
            json!({
                "when": "2026-01-02T09:12:44Z",
                "method": "PATCH",
                "path": "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
                "headerIntent": ["accept", "authorization", "if-match"],
                "requestId": "req-local",
                "status": 500,
                "durationMs": 42,
                "transport": false,
            })
        ),
    )
    .unwrap();

    let output = base(&dir).args(["replay", "--last", "1"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("PATCH"), "{stdout}");
    assert!(stdout.contains("req-local"), "{stdout}");
    assert!(stdout.contains("500"), "{stdout}");
}

/// `replay --last` caps the number of returned entries to the most recent.
#[test]
fn replay_last_returns_only_the_most_recent_entries() {
    let dir = TempDir::new().unwrap();
    fs::create_dir_all(journal_path(&dir).parent().unwrap()).unwrap();
    let mut contents = String::new();
    for index in 0..4 {
        contents.push_str(
            &json!({
                "when": "2026-01-02T09:12:44Z",
                "method": "GET",
                "path": "/api/v1/me",
                "headerIntent": ["accept"],
                "requestId": format!("req-{index}"),
                "status": 500,
                "durationMs": 1,
                "transport": false,
            })
            .to_string(),
        );
        contents.push('\n');
    }
    fs::write(journal_path(&dir), contents).unwrap();

    let output = base(&dir)
        .args(["--json", "replay", "--last", "2"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let entries = body.as_array().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["requestId"], "req-2");
    assert_eq!(entries[1]["requestId"], "req-3");
}

/// An empty journal is an explicit, successful result.
#[test]
fn replay_reports_an_empty_journal() {
    let dir = TempDir::new().unwrap();
    let output = base(&dir).args(["replay"]).output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("no failed-request journal entries"));
}

/// `--last 0` is a usage error rather than a silent empty result.
#[test]
fn replay_rejects_last_zero() {
    let dir = TempDir::new().unwrap();
    let output = base(&dir).args(["replay", "--last", "0"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}
