// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! CLI-76: `work search --explain` server-plan passthrough and the offline
//! `squeakql cache size|clear` management surface.
//!
//! `--explain` never invents a cost model: it forwards server-provided plan
//! data when the response carries it and degrades with a clear message
//! otherwise. Cache management is entirely local and must never touch the
//! network.

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A command wired to `server` and a throwaway config, using an ephemeral
/// token and an explicit Organization.
fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd.arg("--no-input");
    cmd
}

/// A fully offline command: no host, no token. Only local surfaces run.
fn offline(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.env_remove("HAMSTIK_TOKEN");
    cmd.env_remove("HAMSTIK_HOST");
    cmd.current_dir(dir.path());
    cmd.arg("--no-input");
    cmd
}

fn empty_page() -> Value {
    json!({"items": [], "page": {"limit": 50, "hasMore": false, "nextCursor": null}})
}

/// `cache size` and `cache clear` are local-only, idempotent, and never make a
/// network request — even when an Organization/token is absent.
#[test]
fn cache_size_and_clear_work_offline_and_are_idempotent() {
    let dir = TempDir::new().unwrap();

    // Seed two saved queries through the offline `save` path.
    for (name, query) in [("first", "status = todo"), ("second", "priority >= high")] {
        let output = offline(&dir)
            .args(["squeakql", "save", name, query])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    }

    let output = offline(&dir)
        .args(["--json", "squeakql", "cache", "size"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["queries"], 2);
    assert!(body["bytes"].as_u64().unwrap() > 0, "{body}");
    assert!(body["path"].as_str().unwrap().ends_with("queries.toml"));
    assert_eq!(body["exists"], true);

    // Clear removes the file and reports the removal.
    let output = offline(&dir)
        .args(["--json", "squeakql", "cache", "clear"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["cleared"], true);
    assert!(!dir.path().join("queries.toml").exists());

    // A second clear is a no-op that still succeeds.
    let output = offline(&dir)
        .args(["--json", "squeakql", "cache", "clear"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["cleared"], false);

    // Size after clear is zero and the file stays absent.
    let output = offline(&dir)
        .args(["--json", "squeakql", "cache", "size"])
        .output()
        .unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["queries"], 0);
    assert_eq!(body["bytes"], 0);
    assert_eq!(body["exists"], false);
}

/// A corrupt cache file still clears: `cache clear` removes it without
/// parsing, so it is the documented recovery path.
#[test]
fn cache_clear_recovers_from_a_corrupt_file() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("queries.toml"), "not valid toml {{{").unwrap();

    let output = offline(&dir)
        .args(["--json", "squeakql", "cache", "clear"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["cleared"], true);
    assert!(!dir.path().join("queries.toml").exists());
}

/// Human cache output names the count, size, and path without requiring a
/// network endpoint.
#[test]
fn cache_size_human_output_is_actionable() {
    let dir = TempDir::new().unwrap();
    offline(&dir)
        .args(["squeakql", "save", "todos", "status = todo"])
        .output()
        .unwrap();

    let output = offline(&dir)
        .args(["squeakql", "cache", "size"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Saved queries: 1"), "{text}");
    assert!(text.contains("queries.toml"), "{text}");

    let output = offline(&dir)
        .args(["squeakql", "cache", "clear"])
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("cleared the saved-query cache"), "{text}");
}

/// `--explain` forwards the server-provided plan verbatim and never adds an
/// unsupported field to the search request body.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explain_forwards_server_plan_without_inventing_a_request_field() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/work-items/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "page": {"limit": 50, "hasMore": false, "nextCursor": null},
            "queryPlan": {"estimatedRows": 12, "steps": ["index:status"]}
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--json",
            "work",
            "search",
            "status = todo",
            "--explain",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["explain"]["available"], true);
    assert_eq!(body["explain"]["plan"]["estimatedRows"], 12);
    assert_eq!(body["explain"]["plan"]["steps"][0], "index:status");

    // The request body stayed inside the frozen SqueakQLSearchRequest schema:
    // only query/limit/cursor, never an `explain` field.
    let request = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.url.path().ends_with("/search"))
        .expect("search request");
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["query"], "status = todo");
    assert!(sent.get("explain").is_none(), "{sent}");
    let keys: Vec<&String> = sent.as_object().unwrap().keys().collect();
    assert!(
        keys.iter()
            .all(|key| matches!(key.as_str(), "query" | "limit" | "cursor")),
        "unexpected search request fields: {keys:?}"
    );
}

/// Without server plan data, `--explain` keeps the search result and reports
/// the absence clearly in both structured and human output.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explain_degrades_clearly_when_server_has_no_plan_data() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/work-items/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_page()))
        .mount(&server)
        .await;

    // Structured: an explicit unavailable notice beside the unchanged result.
    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--json",
            "work",
            "search",
            "status = todo",
            "--explain",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["explain"]["available"], false);
    let message = body["explain"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("query-plan") && message.contains("no client-side estimate"),
        "{body}"
    );
    assert_eq!(body["items"], json!([]));

    // Human: the search table still renders and the notice goes to stderr.
    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "work",
            "search",
            "status = todo",
            "--explain",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("query-plan"), "{stderr}");
    assert!(stderr.contains("no client-side estimate"), "{stderr}");
}
