// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik api rate-limit` contract tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A minimal valid `GET /me` body so the typed probe read deserializes.
fn me_json() -> Value {
    json!({
        "id": "11111111-1111-4111-8111-111111111111",
        "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg",
        "name": "Steven",
        "email": "steven@example.com",
        "authentication": {
            "type": "pat",
            "credentialId": "22222222-2222-4222-8222-222222222222",
            "credentialName": "cli-pat",
            "scopes": ["work:read"],
            "expiresAt": "2027-01-01T00:00:00Z"
        },
        "defaultOrganization": null,
        "organizations": []
    })
}

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token-value");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.env_remove("NO_COLOR");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

/// `--json --no-input` emits the documented stable snapshot shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_output_uses_the_documented_rate_limit_shape() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(me_json())
                .insert_header("RateLimit-Limit", "100")
                .insert_header("RateLimit-Remaining", "37")
                .insert_header("RateLimit-Reset", "12")
                .insert_header("X-Request-Id", "req-probe"),
        )
        .mount(&server)
        .await;

    let output = base(&server, &dir)
        .args(["--json", "--no-input", "api", "rate-limit"])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        body["rateLimit"],
        json!({"limit": 100, "remaining": 37, "resetIn": 12})
    );
    assert_eq!(body["requestId"], "req-probe");
}

/// The probe and a concurrent `api request` observe the same snapshot,
/// because both read it from the one transport-level parser.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_matches_the_passthrough_meta_snapshot() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(me_json())
                .insert_header("RateLimit-Limit", "100")
                .insert_header("RateLimit-Remaining", "37")
                .insert_header("RateLimit-Reset", "12"),
        )
        .mount(&server)
        .await;

    let probe = base(&server, &dir)
        .args(["--json", "--no-input", "api", "rate-limit"])
        .output()
        .unwrap();
    let probe_body: Value = serde_json::from_slice(&probe.stdout).unwrap();

    let passthrough = base(&server, &dir)
        .args(["--json", "--no-input", "api", "request", "/api/v1/me"])
        .output()
        .unwrap();
    let passthrough_body: Value = serde_json::from_slice(&passthrough.stdout).unwrap();

    assert_eq!(
        probe_body["rateLimit"],
        passthrough_body["meta"]["rateLimit"]
    );
}

/// A response without `RateLimit-*` headers succeeds with an explicit `null`
/// snapshot plus a warning; it is not an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_headers_report_a_null_snapshot() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json()))
        .mount(&server)
        .await;

    let output = base(&server, &dir)
        .args(["--json", "--no-input", "api", "rate-limit"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(body["rateLimit"].is_null());

    // Human mode explains the absent snapshot on stderr without failing.
    base(&server, &dir)
        .args(["--no-input", "api", "rate-limit"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no snapshot"));
}

/// Human output phrases the values as a snapshot, not a guarantee.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_output_frames_the_values_as_a_snapshot() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(me_json())
                .insert_header("RateLimit-Limit", "100")
                .insert_header("RateLimit-Remaining", "37")
                .insert_header("RateLimit-Reset", "12"),
        )
        .mount(&server)
        .await;

    base(&server, &dir)
        .args(["--no-input", "api", "rate-limit"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("37 of 100 remaining")
                .and(predicate::str::contains("snapshot")),
        );
}

/// A rate-limited probe surfaces the stable exit code 7 and the snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limited_probe_exits_seven() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_json(json!({
                    "error": {"code": "RATE_LIMITED", "message": "slow down"}
                }))
                .insert_header("Retry-After", "1")
                .insert_header("RateLimit-Limit", "100")
                .insert_header("RateLimit-Remaining", "0")
                .insert_header("RateLimit-Reset", "30"),
        )
        .mount(&server)
        .await;

    base(&server, &dir)
        .args(["--json", "--no-input", "api", "rate-limit"])
        .assert()
        .code(7);
}
