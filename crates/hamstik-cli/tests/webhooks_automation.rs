// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Wire coverage for the first-class Public API v1.0.0 webhook and automation
//! rule command families.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use assert_cmd::Command;
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{body_json, header, header_exists, method, path};
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
    cmd.args(["--no-retry", "--org", "acme"]);
    cmd
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn webhook_list_uses_the_documented_organization_route() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/webhooks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "page": {"limit": 50, "hasMore": false, "nextCursor": null}
        })))
        .expect(1)
        .mount(&server)
        .await;

    base(&server, &dir)
        .args(["webhook", "list"])
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_edit_sends_body_etag_and_contract_required_idempotency() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("PATCH"))
        .and(path("/api/v1/organizations/acme/automation-rules/rule-1"))
        .and(header("If-Match", "\"rule-4\""))
        .and(header_exists("Idempotency-Key"))
        .and(body_json(json!({"name": "Triage bugs"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "rule-1",
            "name": "Triage bugs",
            "revision": 5
        })))
        .expect(1)
        .mount(&server)
        .await;

    base(&server, &dir)
        .args([
            "rule",
            "edit",
            "rule-1",
            "--field",
            "name=Triage bugs",
            "--if-match",
            "\"rule-4\"",
        ])
        .assert()
        .success();
}
