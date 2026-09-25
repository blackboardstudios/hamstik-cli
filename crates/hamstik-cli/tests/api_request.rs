// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Public API v1 passthrough contract tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token-value");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn page(items: Value) -> Value {
    json!({ "items": items, "page": { "limit": 50, "hasMore": false, "nextCursor": null } })
}

// ---------------------------------------------------------------------------
// Path validation: only /api/v1/... passes; everything else fails locally
// (exit 2) and must never reach the network (no mock expectations mounted,
// and the mock server records zero requests).
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn private_and_non_v1_paths_fail_locally() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    for bad in [
        "/api/v0/organizations",
        "/api/",
        "/api/private/organizations",
        "/internal/admin",
        "/api/v1/../private/organizations",
        "https://evil.example/api/v1/orgs",
        "/api/v1/organizations?limit=5",
        "/api/v1/organizations#top",
        "/api/v1/organizations\\",
        "/api/v1/org%2Fencoded",
        "/api/v1/admin@user",
        "api/v1/organizations",
        "/apiv1/organizations",
        "",
    ] {
        base(&server, &dir)
            .args(["api", "request", bad])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("INVALID_INPUT"));
    }
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "a rejected path must never reach the network"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_path_reaches_the_server_origin() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/future-things"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
        .mount(&server)
        .await;
    base(&server, &dir)
        .args(["api", "request", "/api/v1/future-things"])
        .assert()
        .success();
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Forward compatibility: a route unknown to the checked-in OpenAPI snapshot
// remains callable — the snapshot must never act as an allowlist.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn newer_server_route_remains_callable() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    // `/sprints/{key}/forecast` is deliberately absent from
    // openapi/hamstik-v1.json — a newer server route the installed CLI has
    // no typed command for. The passthrough must still deliver it.
    Mock::given(method("GET"))
        .and(path("/api/v1/sprints/SPR-9/forecast"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "forecast": [{"week": 1, "velocity": 42}]
                }))
                .insert_header("X-Request-Id", "req-forward"),
        )
        .mount(&server)
        .await;

    let output = base(&server, &dir)
        .args([
            "--json",
            "--no-input",
            "api",
            "request",
            "/api/v1/sprints/SPR-9/forecast",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let body: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(body["method"], "GET");
    assert_eq!(body["path"], "/api/v1/sprints/SPR-9/forecast");
    assert_eq!(body["data"]["forecast"][0]["velocity"], 42);
    assert_eq!(body["meta"]["requestId"], "req-forward");

    // The bare form reaches the same handler.
    base(&server, &dir)
        .args(["api", "/api/v1/sprints/SPR-9/forecast"])
        .assert()
        .success();
}

#[test]
fn forecast_route_is_absent_from_the_checked_in_snapshot() {
    let snapshot = include_str!("../../../openapi/hamstik-v1.json");
    assert!(
        !snapshot.contains("/sprints/{key}/forecast"),
        "the fixture route exists in the snapshot; the forward-compat test must use an unknown route"
    );
}

// ---------------------------------------------------------------------------
// Methods, bodies, idempotency.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_is_the_default_method() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;
    base(&server, &dir)
        .args(["api", "request", "/api/v1/organizations"])
        .assert()
        .success();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_carries_the_body_and_an_automatic_idempotency_key() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "o9", "slug": "new", "name": "New", "suspended": false, "plan": "pro"
        })))
        .mount(&server)
        .await;

    let output = base(&server, &dir)
        .args([
            "--json",
            "--no-input",
            "api",
            "request",
            "--method",
            "POST",
            "--field",
            "name=New",
            "--field",
            "slug=new",
            "/api/v1/organizations",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["data"]["slug"], "new");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let key = requests[0]
        .headers
        .get("Idempotency-Key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert_eq!(key.len(), 36, "generated key must be a uuid: {key}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn body_file_from_stdin_is_used_verbatim() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("PATCH"))
        .and(path("/api/v1/organizations/acme"))
        .and(body_json(json!({"name": "Renamed"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "o1", "slug": "acme", "name": "Renamed", "suspended": false, "plan": "pro"
        })))
        .mount(&server)
        .await;

    base(&server, &dir)
        .args([
            "--no-input",
            "api",
            "request",
            "--method",
            "PATCH",
            "--body-file",
            "-",
            "/api/v1/organizations/acme",
        ])
        .write_stdin("{\"name\": \"Renamed\"}\n")
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflicting_body_sources_fail_at_parse_time() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "api",
            "request",
            "--method",
            "POST",
            "--body-file",
            "/tmp/whatever.json",
            "--field",
            "name=x",
            "/api/v1/organizations",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_sends_an_idempotency_key_and_handles_204() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("DELETE"))
        .and(path("/api/v1/organizations/acme/projects/HAM"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    base(&server, &dir)
        .args([
            "api",
            "request",
            "--method",
            "DELETE",
            "/api/v1/organizations/acme/projects/HAM",
        ])
        .assert()
        .success();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].headers.get("Idempotency-Key").is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idempotency_key_override_is_sent_verbatim() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "1", "key": "HAM-9"})))
        .mount(&server)
        .await;

    base(&server, &dir)
        .args([
            "api",
            "request",
            "--method",
            "POST",
            "--idempotency-key",
            "my-logical-op-42",
            "--field",
            "title=Hello",
            "/api/v1/organizations/acme/projects/HAM/work-items",
        ])
        .assert()
        .success();
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0]
            .headers
            .get("Idempotency-Key")
            .and_then(|v| v.to_str().ok()),
        Some("my-logical-op-42")
    );
}

/// Internal retries reuse the same generated key: two attempts, one key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn internal_retries_reuse_the_generated_idempotency_key() {
    let server = MockServer::start().await;
    let seen_keys: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let keys = Arc::clone(&seen_keys);
    Mock::given(method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            let key = request
                .headers
                .get("Idempotency-Key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            keys.lock().unwrap().push(key);
            // 503 is a retryable status under the shared policy.
            ResponseTemplate::new(503).set_body_json(json!({
                "error": {"code": "MAINTENANCE", "message": "try again"}
            }))
        })
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    // With the default retry policy (two attempts), a failing 500 retries
    // once; the second attempt must reuse the first attempt's key. Retries
    // are enabled here (no --no-retry).
    let mut cmd = Command::cargo_bin("hamstik").unwrap();
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token-value");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.args([
        "api",
        "request",
        "--field",
        "title=Hello",
        "--method",
        "POST",
        "/api/v1/organizations",
    ]);
    cmd.assert().failure();

    let keys = seen_keys.lock().unwrap();
    // The default policy makes 3 attempts (first + 2 retries); every
    // attempt must carry the SAME generated idempotency key.
    assert!(
        keys.len() >= 2,
        "expected at least one retry, got {}",
        keys.len()
    );
    assert_eq!(keys[0].len(), 36, "generated key must be a uuid");
    assert!(
        keys.iter().all(|k| k == &keys[0]),
        "every attempt must reuse the same idempotency key: {keys:?}"
    );
}

// ---------------------------------------------------------------------------
// Response metadata, dry-run.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn response_metadata_is_preserved_in_json_output() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "id": "o1", "slug": "acme", "name": "Acme", "suspended": false, "plan": "pro"
                }))
                .insert_header("X-Request-Id", "req-123")
                .insert_header("ETag", "\"rev-7\""),
        )
        .mount(&server)
        .await;

    let output = base(&server, &dir)
        .args([
            "--json",
            "--no-input",
            "api",
            "request",
            "/api/v1/organizations/acme",
        ])
        .output()
        .unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["meta"]["requestId"], "req-123");
    assert_eq!(body["meta"]["etag"], "\"rev-7\"");
}

/// `Idempotency-Replayed` from the server surfaces in the meta block.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idempotency_replayed_metadata_surfaces() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "id": "o1", "slug": "acme", "name": "Acme", "suspended": false, "plan": "pro"
                }))
                .insert_header("Idempotency-Replayed", "true"),
        )
        .mount(&server)
        .await;

    let output = base(&server, &dir)
        .args([
            "--json",
            "--no-input",
            "api",
            "request",
            "--method",
            "POST",
            "--field",
            "name=Acme",
            "/api/v1/organizations",
        ])
        .output()
        .unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["meta"]["idempotencyReplayed"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dry_run_previews_the_mutation_and_sends_nothing() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    // No mock mounted: ANY network access would leave the request unmatched.
    let output = base(&server, &dir)
        .args([
            "--json",
            "--no-input",
            "--dry-run",
            "api",
            "request",
            "--method",
            "POST",
            "--field",
            "title=Hello",
            "/api/v1/organizations",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["dryRun"], true);
    assert_eq!(body["operation"], "api.request");
    assert_eq!(body["request"]["method"], "POST");
    assert_eq!(body["request"]["path"], "/api/v1/organizations");
    assert_eq!(body["body"]["title"], "Hello");
    let key = body["request"]["headers"]["Idempotency-Key"]
        .as_str()
        .unwrap();
    assert_eq!(key.len(), 36, "generated key must be a strong random key");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dry_run_is_rejected_for_get() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--dry-run", "api", "request", "/api/v1/organizations"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("previews mutations only"));
}

// ---------------------------------------------------------------------------
// Redaction: no credential value may appear anywhere for any failure mode.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_credential_value_leaks_into_any_output() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": {"code": "FORBIDDEN", "message": "insufficient scope"}
        })))
        .mount(&server)
        .await;

    for args in [
        vec![
            "--json",
            "--no-input",
            "api",
            "request",
            "/api/v1/organizations",
        ],
        vec!["api", "request", "/api/v1/organizations"],
    ] {
        let output = base(&server, &dir).args(args).output().unwrap();
        assert!(!output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !stdout.contains("secret-token-value"),
            "stdout leaked: {stdout}"
        );
        assert!(
            !stderr.contains("secret-token-value"),
            "stderr leaked: {stderr}"
        );
    }
}

/// The header-override allowlist rejects credential-bearing headers before
/// any request is built; a hostile `Authorization` can never reach the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credential_header_overrides_are_rejected_locally() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    for header in [
        "Authorization: Bearer stolen",
        "authorization: x",
        "X-Api-Key: k",
        "Cookie: session=1",
    ] {
        base(&server, &dir)
            .args([
                "api",
                "request",
                "--method",
                "POST",
                "--header",
                header,
                "/api/v1/organizations",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("allowlist"));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn path_validation_rejects_traversal_and_malformed_input() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    for bad in [
        "/api/v1/organizations/../private",
        "/api/v1/./organizations",
        "/api/v1//organizations",
        "/api/v1/organizations ",
        "/api/v1/../",
    ] {
        base(&server, &dir)
            .args(["api", "request", bad])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("INVALID_INPUT"));
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}
