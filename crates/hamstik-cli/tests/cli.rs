// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Integration tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! End-to-end CLI tests: drive the compiled `hamstik` binary against a local
//! wiremock server. Credentials use the ephemeral `HAMSTIK_TOKEN` path so the
//! tests never touch an OS keyring.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// A page envelope helper.
fn page(items: Value) -> Value {
    json!({ "items": items, "page": { "limit": 50, "hasMore": false, "nextCursor": null } })
}

fn work_item_json(status: &str, revision: i64) -> Value {
    json!({
        "id": "1", "key": "HAM-1", "projectId": "2", "title": "T", "description": null,
        "type": "task", "status": status, "priority": "low", "assignee": null, "reporter": null,
        "sprint": null, "parent": null, "labels": [],
        "storyPoints": null, "dueDate": null, "archivedAt": null,
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": revision
    })
}

fn me_json() -> Value {
    json!({
        "id": "u1", "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg",
        "name": "Steven", "email": "steven@example.com",
        "authentication": {"type": "pat", "credentialId": "c", "credentialName": "n", "scopes": [], "expiresAt": "2027-01-01T00:00:00Z"},
        "defaultOrganization": null,
        "organizations": []
    })
}

/// Mounts the checked-in compatible Public API contract for doctor tests.
async fn mount_doctor_openapi(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/openapi.json"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            include_str!("../../../openapi/hamstik-v1.json"),
            "application/json",
        ))
        .mount(server)
        .await;
}

/// Base command wired to `server` + a throwaway config, using an ephemeral token.
fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_list_json_emits_raw_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "o1", "slug": "acme", "name": "Acme", "suspended": false, "plan": "pro"}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["org", "list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["items"][0]["slug"], "acme");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_list_renders_table() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "o1", "slug": "acme", "name": "Acme", "suspended": false, "plan": "pro"}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["org", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("SLUG"))
        .stdout(predicate::str::contains("Acme"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_view_not_found_maps_to_exit_five() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/missing"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {"code": "NOT_FOUND", "message": "no such organization"}
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["org", "view", "missing"])
        .assert()
        .code(5)
        .stderr(predicate::str::contains("no such organization"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_status_uses_token_and_reports_identity() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["auth", "status", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["authenticated"], true);
    assert_eq!(body["user"]["email"], "steven@example.com");

    let req = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        req.headers.get("authorization").unwrap().to_str().unwrap(),
        "Bearer secret-token"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_create_sends_idempotency_key_and_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("ETag", "\"rev-1\"")
                .set_body_json(work_item_json("todo", 1)),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--title",
            "Ship it",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["key"], "HAM-1");

    let req = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    assert!(req.headers.contains_key("idempotency-key"));
    let sent: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(sent["title"], "Ship it");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_start_verifies_transition_and_sends_if_match() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-7\"")
                .set_body_json(work_item_json("todo", 7)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/transitions",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currentStatus": "todo",
            "transitions": [{"targetStatus": "in_progress"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/transitions",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-8\"")
                .set_body_json(work_item_json("in_progress", 8)),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "start",
            "HAM-1",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["status"], "in_progress");

    let post = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    assert_eq!(
        post.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"rev-7\""
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_transition_rejects_disallowed_target() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-7\"")
                .set_body_json(work_item_json("todo", 7)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/transitions",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currentStatus": "todo",
            "transitions": [{"targetStatus": "in_review"}]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "transition",
            "HAM-1",
            "done",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not allowed"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_list_requires_project() {
    let dir = TempDir::new().unwrap();
    // A mock server is needed for HAMSTIK_HOST but should never be contacted.
    let server = MockServer::start().await;
    base(&server, &dir)
        .args(["--org", "acme", "work", "list"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no project"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_reports_ready_when_authenticated() {
    let server = MockServer::start().await;
    mount_doctor_openapi(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["schemaVersion"], 1);
    assert_eq!(body["exitCode"], 0);
    let checks = body["checks"].as_array().unwrap();
    for id in [
        "local.config",
        "local.context_resolution",
        "local.host",
        "credential.source",
        "network.connectivity",
        "api.openapi",
        "api.compatibility",
        "api.authentication",
    ] {
        assert!(
            checks.iter().any(|check| check["id"] == id),
            "missing doctor check {id}: {body}"
        );
    }
    assert_eq!(
        checks
            .iter()
            .find(|check| check["id"] == "api.compatibility")
            .unwrap()["status"],
        "pass"
    );

    let requests = server.received_requests().await.unwrap();
    let openapi = requests
        .iter()
        .find(|request| request.url.path() == "/api/v1/openapi.json")
        .unwrap();
    assert!(
        openapi.headers.get("authorization").is_none(),
        "the public compatibility probe must not send the PAT"
    );
    let me = requests
        .iter()
        .find(|request| request.url.path() == "/api/v1/me")
        .unwrap();
    assert_eq!(
        me.headers.get("authorization").unwrap().to_str().unwrap(),
        "Bearer secret-token"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_reports_terminal_capabilities_in_json() {
    let server = MockServer::start().await;
    mount_doctor_openapi(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    // The test harness pipes stdout, so color is reported disabled for that
    // reason — the check reflects the actual invocation, not a hypothetical.
    let body: Value = serde_json::from_slice(
        &base(&server, &dir)
            .args(["doctor", "--json"])
            .env("TERM", "xterm-256color")
            .env("LANG", "en_US.UTF-8")
            .env_remove("NO_COLOR")
            .env_remove("CLICOLOR_FORCE")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let rendered = serde_json::to_string(&body).unwrap();
    assert!(!rendered.contains('\u{1b}'), "--json never carries ANSI");
    let checks = body["checks"].as_array().unwrap();
    let color = checks
        .iter()
        .find(|c| c["name"] == "terminal color")
        .expect("color check present");
    assert_eq!(color["ok"], false);
    assert_eq!(color["critical"], false);
    assert!(
        color["detail"]
            .as_str()
            .unwrap()
            .contains("stdout is not a terminal")
    );
    let emoji = checks
        .iter()
        .find(|c| c["name"] == "terminal emoji")
        .expect("emoji check present");
    assert_eq!(emoji["critical"], false);
    assert!(emoji["detail"].as_str().unwrap().contains("not verifiable"));

    // CLICOLOR_FORCE forces color on even for a piped stdout.
    let body: Value = serde_json::from_slice(
        &base(&server, &dir)
            .args(["doctor", "--json"])
            .env("CLICOLOR_FORCE", "1")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let color = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "terminal color")
        .expect("color check present");
    assert_eq!(color["ok"], true);
    assert!(color["detail"].as_str().unwrap().contains("CLICOLOR_FORCE"));
    // Forced color still never leaks ANSI into --json output.
    let rendered = serde_json::to_string(&body).unwrap();
    assert!(!rendered.contains('\u{1b}'));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_reports_disabled_color_when_no_color_flag() {
    let server = MockServer::start().await;
    mount_doctor_openapi(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let body: Value = serde_json::from_slice(
        &base(&server, &dir)
            .args(["doctor", "--json", "--no-color"])
            .env("TERM", "xterm-256color")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let color = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "terminal color")
        .expect("color check present");
    assert_eq!(color["ok"], false);
    assert_eq!(color["critical"], false);
    let detail = color["detail"].as_str().unwrap();
    assert!(detail.contains("--no-color"), "{detail}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_terminal_checks_are_informational_only() {
    // A dumb terminal must not fail doctor: it is a working, plain-text setup.
    let server = MockServer::start().await;
    mount_doctor_openapi(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let body: Value = serde_json::from_slice(
        &base(&server, &dir)
            .args(["doctor", "--json"])
            .env("TERM", "dumb")
            .env_remove("LANG")
            .env_remove("LC_ALL")
            .env_remove("LC_CTYPE")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(body["ok"], true, "terminal checks never fail doctor");
    let checks = body["checks"].as_array().unwrap();
    let color = checks
        .iter()
        .find(|c| c["name"] == "terminal color")
        .expect("color check present");
    assert_eq!(color["ok"], false);
    assert_eq!(color["critical"], false);
    // Piped stdout wins before TERM is even consulted here; assert the
    // informational (non-critical) outcome rather than a specific reason.
    let emoji = checks
        .iter()
        .find(|c| c["name"] == "terminal emoji")
        .expect("emoji check present");
    assert_eq!(emoji["critical"], false);
    assert_eq!(emoji["ok"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_human_output_omits_visual_samples_and_ansi_when_piped() {
    let server = MockServer::start().await;
    mount_doctor_openapi(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    // Piped stdout (the default under assert_cmd) must stay ANSI-free and not
    // include interactive-only visual samples.
    let output = base(&server, &dir)
        .env("TERM", "xterm-256color")
        .env("LANG", "en_US.UTF-8")
        .args(["doctor"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(stdout.contains("terminal color"), "{stdout}");
    assert!(stdout.contains("terminal emoji"), "{stdout}");
    assert!(!stdout.contains("emoji sample"), "{stdout}");
    assert!(!stdout.contains("color sample"), "{stdout}");
    assert!(
        !stdout.contains('\u{1b}'),
        "piped output must not emit ANSI: {stdout}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_color_probe_respects_no_color_env() {
    let server = MockServer::start().await;
    mount_doctor_openapi(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let body: Value = serde_json::from_slice(
        &base(&server, &dir)
            .args(["doctor", "--json"])
            .env("TERM", "xterm-256color")
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let color = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "terminal color")
        .expect("color check present");
    assert_eq!(color["ok"], false);
    assert!(
        color["detail"]
            .as_str()
            .unwrap()
            .contains("NO_COLOR is set")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_fails_without_credentials() {
    let server = MockServer::start().await;
    mount_doctor_openapi(&server).await;
    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .env_remove("HAMSTIK_TOKEN")
        .args(["doctor"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains(
            "no selected profile and HAMSTIK_TOKEN is not set",
        ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_validates_selected_organization_and_project() {
    let server = MockServer::start().await;
    mount_doctor_openapi(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "o1",
            "slug": "acme",
            "name": "Acme",
            "description": null,
            "plan": "pro",
            "role": "admin",
            "isDefault": true,
            "suspended": false,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-02T00:00:00Z"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "p1",
            "organizationId": "o1",
            "key": "HAM",
            "name": "Hamstik",
            "description": null,
            "color": "#3b82f6",
            "revision": 4,
            "archivedAt": null,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-02T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM", "doctor", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    for id in ["context.organization", "context.project"] {
        let check = body["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == id)
            .unwrap();
        assert_eq!(check["status"], "pass", "{check}");
        assert!(check["durationMs"].is_number(), "{check}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_rejects_a_contract_missing_a_required_operation() {
    let server = MockServer::start().await;
    let mut contract: Value =
        serde_json::from_str(include_str!("../../../openapi/hamstik-v1.json")).unwrap();
    contract["paths"]
        .as_object_mut()
        .unwrap()
        .remove("/api/v1/me");
    Mock::given(method("GET"))
        .and(path("/api/v1/openapi.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(contract))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(9));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], false);
    assert_eq!(body["exitCode"], 9);
    let compatibility = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "api.compatibility")
        .unwrap();
    assert_eq!(compatibility["status"], "fail");
    assert_eq!(compatibility["error"]["code"], "PROTOCOL_ERROR");
    assert!(compatibility["detail"].as_str().unwrap().contains("getMe"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_preserves_authentication_request_id() {
    let server = MockServer::start().await;
    mount_doctor_openapi(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(401)
                .insert_header("X-Request-Id", "req-doctor-auth")
                .set_body_json(json!({
                    "error": {
                        "code": "INVALID_TOKEN",
                        "message": "The PAT is invalid",
                        "requestId": "req-doctor-auth"
                    }
                })),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let authentication = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "api.authentication")
        .unwrap();
    assert_eq!(authentication["status"], "fail");
    assert_eq!(authentication["requestId"], "req-doctor-auth");
    assert_eq!(authentication["error"]["requestId"], "req-doctor-auth");
    assert_eq!(authentication["error"]["code"], "INVALID_TOKEN");
    assert_eq!(authentication["error"]["httpStatus"], 401);
}

#[test]
fn doctor_reports_network_failure_and_skips_dependent_checks() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let dir = TempDir::new().unwrap();
    let output = config_command(&dir)
        .env("HAMSTIK_TOKEN", "ephemeral-token")
        .args([
            "--host",
            &format!("http://{address}"),
            "--no-retry",
            "doctor",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(8));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["exitCode"], 8);
    let checks = body["checks"].as_array().unwrap();
    let network = checks
        .iter()
        .find(|check| check["id"] == "network.connectivity")
        .unwrap();
    assert_eq!(network["status"], "fail");
    assert_eq!(network["error"]["code"], "NETWORK_ERROR");
    for id in ["network.tls", "api.openapi", "api.authentication"] {
        assert_eq!(
            checks.iter().find(|check| check["id"] == id).unwrap()["status"],
            "skipped",
            "{id} should be dependency-skipped"
        );
    }
}

// ---- Banner (identity) surfaces -------------------------------------------

/// The banner art fragment used for presence/absence assertions (literal, not a
/// regex — `predicates::str::contains` matches substrings).
const BANNER_ART: &str = r"(\___/)";
const BANNER_FOOTER: &str = "© Blackboard Studios LLC";

/// A `hamstik` command isolated from any host/token/keyring. Only used for the
/// identity surfaces (help/version/bare/completion/parse-errors), which are
/// short-circuited during parsing and never touch the network or OS keyring.
fn banner_command(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    for var in [
        "HAMSTIK_HOST",
        "HAMSTIK_TOKEN",
        "HAMSTIK_PROFILE",
        "HAMSTIK_ORG",
        "HAMSTIK_PROJECT",
    ] {
        cmd.env_remove(var);
    }
    cmd.current_dir(dir.path());
    cmd
}

#[test]
fn root_help_prints_banner_to_stdout() {
    let dir = TempDir::new().unwrap();
    banner_command(&dir)
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(BANNER_ART))
        .stdout(predicate::str::contains(BANNER_FOOTER))
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")))
        .stdout(predicate::str::contains("Usage: hamstik"))
        .stderr(predicate::str::is_empty());
}

#[test]
fn short_help_prints_banner_to_stdout() {
    let dir = TempDir::new().unwrap();
    banner_command(&dir)
        .arg("-h")
        .assert()
        .success()
        .stdout(predicate::str::contains(BANNER_ART))
        .stdout(predicate::str::contains(BANNER_FOOTER));
}

/// A bare `hamstik` is a usage error: clap's missing-subcommand help goes to
/// stderr (so failure output is never mistaken for success content) with exit
/// code 2.
#[test]
fn bare_invocation_is_a_usage_error_on_stderr() {
    let dir = TempDir::new().unwrap();
    banner_command(&dir)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Usage: hamstik"))
        .stdout(predicate::str::is_empty());
}

/// `-V`/`--version` are the machine-parsed surfaces: one terse line. The
/// banner lives on the human `hamstik version` path only.
#[test]
fn version_flag_is_terse_and_version_command_prints_banner() {
    let dir = TempDir::new().unwrap();
    for args in [&["--version"][..], &["-V"][..]] {
        banner_command(&dir)
            .args(args)
            .assert()
            .success()
            .stdout(predicates::ord::eq(format!(
                "hamstik {}\n",
                env!("CARGO_PKG_VERSION")
            )))
            .stdout(predicate::str::contains(BANNER_ART).not());
    }
    banner_command(&dir)
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::contains(BANNER_ART))
        .stdout(predicate::str::contains(BANNER_FOOTER))
        .stdout(predicate::str::contains(format!(
            "v{}",
            env!("CARGO_PKG_VERSION")
        )));
}

#[test]
fn version_json_is_machine_readable_and_banner_free() {
    let dir = TempDir::new().unwrap();
    let output = banner_command(&dir)
        .args(["version", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains(BANNER_ART), "banner leaked into --json");
    let body: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn subcommand_help_and_completion_are_banner_free() {
    let dir = TempDir::new().unwrap();
    for args in [
        &["work", "--help"][..],
        &["auth", "login", "--help"][..],
        &["completion", "bash"][..],
    ] {
        banner_command(&dir)
            .args(args)
            .assert()
            .success()
            .stdout(predicate::str::contains(BANNER_ART).not());
    }
}

#[test]
fn unrecognized_subcommand_has_no_banner() {
    let dir = TempDir::new().unwrap();
    banner_command(&dir)
        .arg("bogus")
        .assert()
        .code(2)
        .stdout(predicate::str::contains(BANNER_ART).not())
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

// ---- Profile removal and broken config files ------------------------------

/// A `hamstik` command with an isolated config and no host/token/profile, for
/// commands that work purely on config state.
fn config_command(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    for var in [
        "HAMSTIK_HOST",
        "HAMSTIK_TOKEN",
        "HAMSTIK_PROFILE",
        "HAMSTIK_ORG",
        "HAMSTIK_PROJECT",
    ] {
        cmd.env_remove(var);
    }
    cmd.current_dir(dir.path());
    cmd
}

const TWO_PROFILES: &str = r#"version = 1
active_profile = "one"

[profiles.one]
host = "https://one.test"
user_id = "u1"
email = "one@example.com"

[profiles.two]
host = "https://two.test"
user_id = "u2"
email = "two@example.com"
"#;

#[test]
fn auth_forget_removes_the_profile_and_repairs_the_active_one() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("config.toml"), TWO_PROFILES).unwrap();

    let output = config_command(&dir)
        .args(["auth", "forget", "one", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["forget"], true);
    assert_eq!(body["profile"], "one");
    assert_eq!(body["host"], "https://one.test");
    // Local-only: a PAT is never revoked by the CLI.
    assert_eq!(body["revoked"], false);
    // `two` is the only profile left, so it becomes active.
    assert_eq!(body["activeProfile"], "two");

    let written = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(!written.contains("one.test"), "{written}");
    assert!(written.contains("two.test"), "{written}");
    assert!(written.contains(r#"active_profile = "two""#), "{written}");
}

#[test]
fn auth_forget_the_last_profile_leaves_no_active_profile() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("config.toml"),
        r#"version = 1
active_profile = "one"

[profiles.one]
host = "https://one.test"
user_id = "u1"
email = "one@example.com"
"#,
    )
    .unwrap();

    let output = config_command(&dir)
        .args(["auth", "forget", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    // No profile named: the active one is the target.
    assert_eq!(body["profile"], "one");
    assert_eq!(body["activeProfile"], Value::Null);
    let written = std::fs::read_to_string(dir.path().join("config.toml")).unwrap();
    assert!(!written.contains("one.test"), "{written}");
}

#[test]
fn auth_forget_unknown_profile_lists_what_exists() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("config.toml"), TWO_PROFILES).unwrap();

    config_command(&dir)
        .args(["auth", "forget", "nope"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no such profile"))
        .stderr(predicate::str::contains("configured profiles: one, two"));
}

#[test]
fn auth_forget_without_any_profile_is_a_usage_error() {
    let dir = TempDir::new().unwrap();
    config_command(&dir)
        .args(["auth", "forget"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no profile to forget"));
}

#[test]
fn corrupt_config_names_the_file_and_exits_configuration() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "version = 1\n[[oops\n").unwrap();

    let expected_path = path.display().to_string();
    config_command(&dir)
        .args(["auth", "list"])
        .assert()
        .code(10)
        .stderr(predicate::str::contains("invalid configuration"))
        .stderr(predicate::str::contains(expected_path))
        .stderr(predicate::str::contains("hint:"));
}

#[test]
fn doctor_reports_a_corrupt_config_instead_of_refusing_to_run() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "version = 1\n[[oops\n").unwrap();

    let expected_path = path.display().to_string();
    // `doctor` is the command people reach for when config is broken, so it must
    // still run and report, not bail before printing anything.
    config_command(&dir)
        .args(["--host", "http://127.0.0.1:9", "--no-retry", "doctor"])
        .assert()
        .code(10)
        .stdout(predicate::str::contains("[FAIL] configuration file"))
        .stdout(predicate::str::contains(expected_path))
        .stdout(predicate::str::contains("terminal color"))
        .stdout(predicate::str::contains("not ready."));
}

#[test]
fn doctor_reports_a_corrupt_context_file_with_its_path() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(".hamstik.toml");
    std::fs::write(&path, "version = 1\nhost = 42\n").unwrap();

    let expected_path = path.display().to_string();
    // The path appears inside a longer JSON string, so no surrounding quotes:
    // only the separators need JSON escaping (backslashes on Windows).
    let expected_json = expected_path.replace('\\', "\\\\");
    config_command(&dir)
        .args([
            "--host",
            "http://127.0.0.1:9",
            "--no-retry",
            "doctor",
            "--json",
        ])
        .assert()
        .code(10)
        .stdout(predicate::str::contains("local.context_file"))
        .stdout(predicate::str::contains("\"name\": \"context file\""))
        .stdout(predicate::str::contains(expected_json));
}

// ---- Hardening behaviors ---------------------------------------------------

/// A hostile or looping server must not be able to keep `--all` running: the
/// aggregation budget terminates the command with a protocol error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_list_all_terminates_on_an_endless_server() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [work_item_json("todo", 1)],
            "page": {"limit": 1, "hasMore": true, "nextCursor": "same"}
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM", "work", "list", "--all"])
        .assert()
        .code(9)
        .stderr(predicate::str::contains("exceeded"));
}

/// `HAMSTIK_TOKEN` containing a control character is rejected before any
/// network activity: such a value can corrupt header framing and can never be
/// a valid PAT.
#[test]
fn env_token_with_control_characters_is_rejected() {
    let dir = TempDir::new().unwrap();
    config_command(&dir)
        .env("HAMSTIK_HOST", "http://localhost:1")
        .env("HAMSTIK_TOKEN", "tok\u{1b}[31m")
        .args(["auth", "status"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("control characters"));
}

/// An oversized token piped to `--with-token` fails fast with a clear error.
#[test]
fn login_with_token_rejects_oversized_input() {
    let dir = TempDir::new().unwrap();
    let big = "x".repeat(8 * 1024);
    config_command(&dir)
        .env("HAMSTIK_HOST", "http://localhost:1")
        .args(["auth", "login", "--with-token"])
        .write_stdin(big)
        .assert()
        .failure()
        .stderr(predicate::str::contains("byte limit"));
}

/// The response-body cap and redirect refusal are enforced end-to-end through
/// the real binary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_is_not_followed_end_to_end() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/evil"))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["auth", "status"])
        .assert()
        .failure();
    // Exactly one request: the redirect was never followed.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

// ---- `use` command validation (SPEC §35) -----------------------------------

/// A project JSON body for `GET /organizations/{org}/projects/{key}`.
fn project_json(key: &str) -> Value {
    json!({
        "id": "p1", "key": key, "name": "P", "color": "#000000", "revision": 1, "archivedAt": null,
        "description": null, "organizationId": "o1",
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
    })
}

/// A 404 with the stable NOT_FOUND code.
fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({
        "error": {"code": "NOT_FOUND", "message": "no such resource"}
    }))
}

/// `org use` validates the slug through the API before persisting it; a typo
/// fails with the server's not-found error and nothing is stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_use_rejects_unknown_slug() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/sph"))
        .respond_with(not_found())
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["org", "use", "sph"])
        .assert()
        .code(5)
        .stderr(predicate::str::contains("no such resource"));

    // Nothing was persisted.
    let output = base(&server, &dir)
        .args(["context", "show", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["organization"]["value"], Value::Null);
    assert_eq!(body["profile"], Value::Null);
}

/// `org use` validates before persisting: the API is contacted first, and the
/// missing-profile usage error surfaces only after validation succeeded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_use_persists_validated_slug() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/sph"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "o1", "slug": "sph", "name": "SPH", "role": "member",
            "description": null, "plan": "pro", "isDefault": false, "suspended": false,
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["org", "use", "sph"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no active profile"));

    // Validation hit the API (the 200 above matched) before persistence was
    // attempted — persistence needs a profile, which the usage error confirms
    // comes after validation.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// `project use` validates the key inside the resolved organization; an
/// organization slug typed into `project use` fails with not-found.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_use_rejects_unknown_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/sph/projects/sph"))
        .respond_with(not_found())
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "sph", "project", "use", "sph"])
        .assert()
        .code(5)
        .stderr(predicate::str::contains("no such resource"));
}

/// `project use` stores the key after the server confirms it exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_use_persists_validated_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/sph/projects/P01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(project_json("P01")))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "sph", "project", "use", "P01"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no active profile"));

    // Validation hit the API (one request) before persistence was attempted.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

// ---- New API surface: sprints, labels, attachments, comment deletion -----

fn sprint_json(state: &str, revision: i64) -> Value {
    json!({
        "id": "11111111-1111-1111-1111-111111111111", "name": "Sprint 1", "state": state,
        "startDate": null, "endDate": null, "goal": null, "targetPoints": null,
        "createdAt": "2026-08-01T00:00:00Z", "updatedAt": "2026-09-01T00:00:00Z", "revision": revision
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sprint_list_renders_table_and_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/sprints"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(page(json!([sprint_json("active", 1)]))),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM", "sprint", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sprint 1"))
        .stdout(predicate::str::contains("active"));

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "sprint",
            "list",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["items"][0]["state"], "active");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sprint_create_sends_name_and_idempotency_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/sprints"))
        .respond_with(ResponseTemplate::new(201).set_body_json(sprint_json("future", 1)))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "sprint",
            "create",
            "--name",
            "Sprint 1",
            "--idempotency-key",
            "sprint-2026-01",
        ])
        .assert()
        .success();

    let req = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        req.headers
            .get("idempotency-key")
            .unwrap()
            .to_str()
            .unwrap(),
        "sprint-2026-01"
    );
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["name"], "Sprint 1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sprint_transition_sends_if_match_and_completion() {
    let sprint_id = "11111111-1111-1111-1111-111111111111";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/sprints/{sprint_id}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"sprint-1\"")
                .set_body_json(sprint_json("active", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/sprints/{sprint_id}/transitions"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currentState": "active",
            "transitions": [{"targetState": "done", "requiresCompletionAction": true}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/sprints/{sprint_id}/transitions"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"sprint-2\"")
                .set_body_json(sprint_json("done", 2)),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "sprint",
            "transition",
            sprint_id,
            "done",
            "--move-to-backlog",
            "--json",
        ])
        .assert()
        .success();

    let req = &server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    assert_eq!(
        req.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"sprint-1\""
    );
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["targetState"], "done");
    assert_eq!(body["completionAction"]["mode"], "backlog");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sprint_transition_rejects_disallowed_target() {
    let sprint_id = "11111111-1111-1111-1111-111111111111";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/sprints/{sprint_id}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"sprint-1\"")
                .set_body_json(sprint_json("done", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/sprints/{sprint_id}/transitions"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currentState": "done", "transitions": []
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "sprint",
            "transition",
            sprint_id,
            "active",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not allowed from done"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn label_list_and_create_flow() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/labels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "l1", "name": "api", "color": "#6366f1", "createdAt": "2026-01-01T00:00:00Z"}
        ]))))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/labels"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!(
            {"id": "l2", "name": "cli", "color": "#6366f1", "createdAt": "2026-01-01T00:00:00Z"}
        )))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "label",
            "list",
            "--json",
        ])
        .assert()
        .success();

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "label",
            "create",
            "--name",
            "cli",
            "--json",
        ])
        .assert()
        .success();

    let req = &server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["name"], "cli");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_label_add_sends_if_match_and_label_id() {
    const LABEL_ID: &str = "11111111-2222-4333-8444-555555555555";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-4\"")
                .set_body_json(work_item_json("todo", 4)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/labels",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-5\"")
                .set_body_json(work_item_json("todo", 5)),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "label",
            "add",
            "HAM-1",
            "--label",
            LABEL_ID,
            "--json",
        ])
        .assert()
        .success();

    let req = &server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    assert_eq!(
        req.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"wi-4\""
    );
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["labelId"], LABEL_ID);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_label_add_sends_label_name_directly() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-4\"")
                .set_body_json(work_item_json("todo", 4)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/labels",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-5\"")
                .set_body_json(work_item_json("todo", 5)),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "label",
            "add",
            "HAM-1",
            "--label",
            "Frontend",
            "--json",
        ])
        .assert()
        .success();

    let req = &server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["label"], "frontend");
    assert!(body.get("labelId").is_none());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_attachment_upload_download_delete() {
    let dir = TempDir::new().unwrap();
    let upload_path = dir.path().join("design.png");
    std::fs::write(&upload_path, b"PNGDATA").unwrap();

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/attachments",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "a1", "workItemId": "w1", "fileName": "design.png", "contentType": "application/octet-stream",
            "size": 7, "createdBy": {"id": "u", "name": "U"}, "createdAt": "2026-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/attachments/a1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "Content-Disposition",
                    "attachment; filename*=UTF-8''design.png",
                )
                .insert_header("Content-Type", "application/octet-stream")
                .set_body_bytes(b"PNGDATA".to_vec()),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/attachments/a1",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    // Upload
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "attachment",
            "upload",
            "HAM-1",
            upload_path.to_str().unwrap(),
            "--json",
        ])
        .assert()
        .success();
    let upload_req = &server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    let content_type = upload_req
        .headers
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        content_type.starts_with("multipart/form-data"),
        "{content_type}"
    );
    let upload_body = String::from_utf8_lossy(&upload_req.body).to_string();
    assert!(
        upload_body.contains("filename=\"design.png\""),
        "{upload_body}"
    );
    assert!(upload_body.contains("PNGDATA"), "{upload_body}");

    // Download
    let output_path = dir.path().join("downloaded.png");
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "attachment",
            "download",
            "HAM-1",
            "a1",
            "--output",
            output_path.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert_eq!(std::fs::read(&output_path).unwrap(), b"PNGDATA");

    // Delete
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "attachment",
            "delete",
            "HAM-1",
            "a1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"deleted\": true"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_comment_delete_maps_conflict_to_exit_six() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments/c1",
        ))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error": {"code": "CONFLICT", "message": "The Comment has replies."},
            "requestId": "r"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "comment",
            "delete",
            "HAM-1",
            "c1",
        ])
        .assert()
        .code(6)
        .stderr(predicate::str::contains("The Comment has replies."));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_comment_delete_success_is_quiet_json() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments/c1",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "comment",
            "delete",
            "HAM-1",
            "c1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"deleted\": true"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_create_requires_name_and_sends_idempotency() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects"))
        .respond_with(ResponseTemplate::new(201).set_body_json(project_json("WEB")))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    // Missing name without a TTY fails deterministically (SPEC §30).
    base(&server, &dir)
        .args(["--org", "acme", "project", "create"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--name"));

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org", "acme", "project", "create", "--name", "Website", "--key", "WEB", "--json",
        ])
        .assert()
        .success();

    let req = &server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["name"], "Website");
    assert_eq!(body["key"], "WEB");
    assert!(req.headers.contains_key("idempotency-key"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sprint_error_codes_map_to_exit_six() {
    let sprint_id = "11111111-1111-1111-1111-111111111111";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/sprints/{sprint_id}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"sprint-1\"")
                .set_body_json(sprint_json("active", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/sprints/{sprint_id}/transitions"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currentState": "active",
            "transitions": [{"targetState": "done", "requiresCompletionAction": true}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/sprints/{sprint_id}/transitions"
        )))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error": {"code": "SPRINT_HAS_UNFINISHED_WORK_ITEMS",
                      "message": "This Sprint has unfinished Work Items; provide completionAction."},
            "requestId": "r"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "sprint",
            "transition",
            sprint_id,
            "done",
            "--move-to-backlog",
        ])
        .assert()
        .code(6)
        .stderr(predicate::str::contains("unfinished Work Items"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_status_json_reports_public_id_and_memberships() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "u1", "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "Steven",
            "email": "steven@example.com",
            "authentication": {"type": "pat", "credentialId": "c", "credentialName": "n", "scopes": [], "expiresAt": "2027-01-01T00:00:00Z"},
            "defaultOrganization": null,
            "organizations": [{"id": "o1", "slug": "acme", "name": "Acme", "username": "steven"}]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["auth", "status", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["user"]["publicId"], "usr_cPbfeqnghA-RLpDVOMQhHg");
    assert_eq!(body["user"]["organizations"][0]["username"], "steven");
}

// ---- Color swatch rendering (human tables, ANSI-aware alignment) -----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_list_plain_when_piped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "p1", "key": "P01", "name": "Project 01", "color": "#6366f1", "revision": 1, "archivedAt": null,
             "description": null, "organizationId": "o1",
             "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    // Piped stdout => color disabled => the swatch still reserves its width
    // (frame + blocks) and the hex stays plain.
    let output = base(&server, &dir)
        .args(["--org", "acme", "project", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        !stdout.contains('\u{1b}'),
        "piped output must be ANSI-free: {stdout}"
    );
    assert!(stdout.contains("[██] #6366f1"), "{stdout}");
    // Column alignment: the swatch cell pads correctly (NAME column aligned).
    assert!(stdout.contains("Project 01"), "{stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_list_swatch_with_forced_truecolor() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "p1", "key": "P01", "name": "Project 01", "color": "#6366f1", "revision": 1, "archivedAt": null,
             "description": null, "organizationId": "o1",
             "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"},
            {"id": "p2", "key": "PK2", "name": "Project 2", "color": "#000000", "revision": 1, "archivedAt": null,
             "description": null, "organizationId": "o1",
             "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["--org", "acme", "project", "list"])
        .env("CLICOLOR_FORCE", "1")
        .env("COLORTERM", "truecolor")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    // Block glyphs are drawn in the foreground, so the swatch sets foreground
    // (38;2) AND background (48;2) to the exact RGB — glyphs in the project
    // color, background matching to seal font seams.
    assert!(
        stdout.contains("\u{1b}[38;2;99;102;241;48;2;99;102;241m"),
        "{stdout}"
    );
    // Black project: black-on-black swatch, but the bracket frame remains visible.
    assert!(stdout.contains("\u{1b}[38;2;0;0;0;48;2;0;0;0m"), "{stdout}");
    // The hex text is always plain (reset before the text).
    assert!(stdout.contains("#6366f1"), "{stdout}");
    assert!(stdout.contains("#000000"), "{stdout}");
    // Alignment must hold: the NAME column starts at the same byte offset in
    // every data row even though swatch cells contain escape sequences.
    let indigo_line = stdout.lines().find(|l| l.contains("P01")).unwrap();
    let black_line = stdout.lines().find(|l| l.contains("PK2")).unwrap();
    assert_eq!(
        indigo_line.find("Project"),
        black_line.find("Project"),
        "NAME column must align across rows with colored swatches"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_view_shows_swatch_in_detail() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/P01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(project_json("P01")))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["--org", "acme", "project", "view", "P01"])
        .env("CLICOLOR_FORCE", "1")
        .env("COLORTERM", "truecolor")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(stdout.contains("color"));
    // project_json uses #000000: glyphs painted black on black, visible frame,
    // plain hex.
    assert!(
        stdout.contains("\u{1b}[38;2;0;0;0;48;2;0;0;0m██\u{1b}[0m"),
        "{stdout}"
    );
    assert!(stdout.contains("#000000"), "{stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_list_json_has_no_swatch_ansi() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "p1", "key": "P01", "name": "Project 01", "color": "#6366f1", "revision": 1, "archivedAt": null,
             "description": null, "organizationId": "o1",
             "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["--org", "acme", "project", "list", "--json"])
        .env("CLICOLOR_FORCE", "1")
        .env("COLORTERM", "truecolor")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        !stdout.contains('\u{1b}'),
        "--json must be ANSI-free: {stdout}"
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["items"][0]["color"], "#6366f1");
}

// ---- Updated API surface tests ---------------------------------------------

fn project_with_archived(key: &str, revision: i64, archived_at: Option<&str>) -> Value {
    json!({
        "id": "p1", "organizationId": "o1", "key": key, "name": "P", "color": "#000000",
        "description": null, "revision": revision, "archivedAt": archived_at,
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
    })
}

fn me_public_json() -> Value {
    json!({
        "id": "u1", "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "Steven", "email": "steven@example.com",
        "authentication": {"type": "pat", "credentialId": "c", "credentialName": "n", "scopes": [], "expiresAt": "2027-01-01T00:00:00Z"},
        "defaultOrganization": null,
        "organizations": []
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_create_sends_assignee_public_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(ResponseTemplate::new(201).set_body_json(work_item_json("todo", 1)))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--title",
            "T",
            "--assignee",
            "usr_cPbfeqnghA-RLpDVOMQhHg",
            "--json",
        ])
        .assert()
        .success();

    let body: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(body["assigneePublicId"], "usr_cPbfeqnghA-RLpDVOMQhHg");
    assert!(body.get("assigneeId").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_create_resolves_assignee_me_via_whoami() {
    const PUBLIC_ID: &str = "usr_cPbfeqnghA-RLpDVOMQhHg";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_public_json()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(ResponseTemplate::new(201).set_body_json(work_item_json("todo", 1)))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--title",
            "T",
            "--assignee",
            "me",
            "--json",
        ])
        .assert()
        .success();

    let body: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[1].body).unwrap();
    assert_eq!(body["assigneePublicId"], PUBLIC_ID);
    assert!(body.get("assigneeId").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_edit_forwards_parent_identifier_to_the_server() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-9\"")
                .set_body_json(work_item_json("todo", 9)),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-10\"")
                .set_body_json(work_item_json("todo", 10)),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "edit",
            "HAM-42",
            "--parent",
            "HAM-1",
            "--json",
        ])
        .assert()
        .success();

    let body: Value = serde_json::from_slice(
        &server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.method.as_str() == "PATCH")
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(body["parentId"], "HAM-1");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_comment_list_excludes_deleted() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                {"id": "c1", "workItemId": "w1", "parentCommentId": null,
                 "author": {"id": "u1", "name": "Steven"}, "body": "alive",
                 "deleted": false, "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z", "editedAt": null},
                {"id": "c2", "workItemId": "w1", "parentCommentId": "c1",
                 "author": {"id": "u1", "name": "Steven"}, "body": null,
                 "deleted": true, "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z", "editedAt": null},
                {"id": "c3", "workItemId": "w1", "parentCommentId": null,
                 "author": {"id": "u1", "name": "Steven"}, "body": "also alive",
                 "deleted": false, "createdAt": "2026-01-02T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "editedAt": null}
            ],
            "page": {"limit": 50, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let threaded = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "comment",
            "list",
            "HAM-1",
        ])
        .output()
        .unwrap();
    assert!(threaded.status.success());
    let threaded_stdout = String::from_utf8(threaded.stdout).unwrap();
    assert!(threaded_stdout.contains("PARENT"));
    assert!(threaded_stdout.contains("c1"));
    assert!(threaded_stdout.contains("(deleted)"));

    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "comment",
            "list",
            "HAM-1",
            "--exclude-deleted",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("alive"));
    assert!(!stdout.contains("(deleted)"));
    assert!(!stdout.is_empty());
    assert_eq!(stdout.matches("Steven").count(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_list_supports_sort_and_archived_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "list",
            "--sort",
            "dueDate",
            "--overdue",
            "true",
            "--archived",
            "false",
            "--top-level=false",
        ])
        .assert()
        .success();

    let query = server.received_requests().await.unwrap()[0]
        .url
        .query()
        .unwrap_or_default()
        .to_string();
    assert!(query.contains("sort=dueDate"), "{query}");
    assert!(query.contains("overdue=true"), "{query}");
    assert!(query.contains("archived=false"), "{query}");
    assert!(query.contains("topLevel=false"), "{query}");
}

/// Regression test for CLI-18: `--archived true` selects archived items only.
/// It must never be described as "including" archived items alongside active
/// ones, and omitting the flag must not send the parameter at all (the server
/// then lists unarchived items).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_list_archived_flag_is_a_state_filter_not_a_union() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "list",
            "--json",
        ])
        .assert()
        .success();
    let omitted = server.received_requests().await.unwrap()[0]
        .url
        .query()
        .unwrap_or_default()
        .to_string();
    assert!(!omitted.contains("archived"), "{omitted}");

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "list",
            "--archived",
            "true",
            "--json",
        ])
        .assert()
        .success();
    let archived_only = server
        .received_requests()
        .await
        .unwrap()
        .last()
        .unwrap()
        .url
        .query()
        .unwrap_or_default()
        .to_string();
    assert!(archived_only.contains("archived=true"), "{archived_only}");
}

/// CLI-18: help text must state the archived-state-filter semantics without
/// implying that active and archived resources are returned together.
#[test]
fn archived_help_texts_describe_state_filter_semantics() {
    for args in [
        &["work", "list", "--help"][..],
        &["org", "work", "--help"][..],
        &["work", "mine", "--help"][..],
        &["user", "work", "--help"][..],
        &["project", "list", "--help"][..],
    ] {
        let output = Command::cargo_bin("hamstik")
            .expect("hamstik binary")
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(
            !help.contains("Include archived"),
            "'Include archived' wording leaked into {}: {help}",
            args.join(" ")
        );
        assert!(
            help.to_lowercase().contains("only archived"),
            "help for {} must state archived-state semantics: {help}",
            args.join(" ")
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_archive_sends_if_match_and_empty_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-4\"")
                .set_body_json(work_item_json("done", 4)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/archive",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_item_json("done", 5)))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "archive",
            "HAM-1",
            "--json",
        ])
        .assert()
        .success();

    let reqs = server.received_requests().await.unwrap();
    let archive = reqs.iter().find(|r| r.method.as_str() == "POST").unwrap();
    assert_eq!(
        archive.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"wi-4\""
    );
    assert!(archive.headers.contains_key("idempotency-key"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_delete_sends_cascade_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-4\"")
                .set_body_json(work_item_json("done", 4)),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "delete",
            "HAM-1",
            "--cascade",
            "--json",
        ])
        .assert()
        .success();

    let req = &server.received_requests().await.unwrap()[1];
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["cascade"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_link_add_list_and_delete() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/links"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "l1", "relation": "blocks",
            "otherWorkItem": {"id": "2", "key": "HAM-2", "project": {"id": "p", "key": "HAM", "name": "Ham"}, "title": "Other", "type": "task", "status": "todo"},
            "createdBy": null, "createdAt": "2026-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/links"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([{
            "id": "l1", "relation": "blocks",
            "otherWorkItem": {"id": "2", "key": "HAM-2", "project": {"id": "p", "key": "HAM", "name": "Ham"}, "title": "Other", "type": "task", "status": "todo"},
            "createdBy": null, "createdAt": "2026-01-01T00:00:00Z"
        }]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "link",
            "add",
            "HAM-1",
            "--target-key",
            "HAM-2",
            "--relation",
            "blocks",
            "--json",
        ])
        .assert()
        .success();

    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "link",
            "list",
            "HAM-1",
            "--json",
        ])
        .assert()
        .success();

    let reqs = server.received_requests().await.unwrap();
    let post = reqs.iter().find(|r| r.method.as_str() == "POST").unwrap();
    let body: Value = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(body["targetKey"], "HAM-2");
    assert_eq!(body["relation"], "blocks");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_link_duplicate_maps_to_exit_six() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/links",
        ))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error": {"code": "LINK_DUPLICATE", "message": "A link already exists."},
            "requestId": "r"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "link",
            "add",
            "HAM-1",
            "--target-key",
            "HAM-2",
            "--relation",
            "blocks",
        ])
        .assert()
        .code(6)
        .stderr(predicate::str::contains("A link already exists"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_activity_renders_feed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/activity"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "a1", "action": "status_changed", "actor": {"publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "A"},
             "detail": {"from": "todo", "to": "done"}, "createdAt": "2026-01-02T00:00:00Z"}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "activity",
            "HAM-1",
            "--json",
        ])
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_comment_edit_sends_body_and_idempotency() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments/c1",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "c1", "workItemId": "w1", "parentCommentId": null,
            "author": {"publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "A"},
            "body": "edited", "deleted": false,
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z",
            "editedAt": "2026-01-02T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "comment",
            "edit",
            "HAM-1",
            "c1",
            "--body",
            "edited",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["editedAt"], "2026-01-02T00:00:00Z");

    let req = &server.received_requests().await.unwrap()[0];
    assert!(req.headers.contains_key("idempotency-key"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_bulk_create_sends_operations() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/bulk-work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{"index": 0, "status": 201, "workItem": work_item_json("todo", 1)}]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let ops_path = dir.path().join("ops.json");
    std::fs::write(&ops_path, r#"[{"projectKey":"HAM","title":"First"}]"#).unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "work",
            "bulk",
            "create",
            "--operations-file",
            ops_path.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["results"][0]["workItem"]["key"], "HAM-1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_bulk_rejects_too_many_operations_locally() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let ops_path = dir.path().join("ops.json");
    let ops: Vec<Value> = (0..51)
        .map(|i| json!({"projectKey": "HAM", "title": format!("Item {i}")}))
        .collect();
    std::fs::write(&ops_path, serde_json::to_string(&ops).unwrap()).unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "work",
            "bulk",
            "create",
            "--operations-file",
            ops_path.to_str().unwrap(),
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("at most 50"))
        .stderr(predicate::str::contains("51 operations"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_edit_sends_if_match_and_idempotency() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/WEB"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"project-1\"")
                .set_body_json(project_with_archived("WEB", 1, None)),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/organizations/acme/projects/WEB"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"project-2\"")
                .set_body_json(project_with_archived("WEB", 2, None)),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org", "acme", "project", "edit", "WEB", "--name", "New name", "--json",
        ])
        .assert()
        .success();

    let reqs = server.received_requests().await.unwrap();
    let patch = reqs.iter().find(|r| r.method.as_str() == "PATCH").unwrap();
    assert_eq!(
        patch.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"project-1\""
    );
    assert!(patch.headers.contains_key("idempotency-key"));
    let body: Value = serde_json::from_slice(&patch.body).unwrap();
    assert_eq!(body["name"], "New name");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_archive_and_unarchive_flow() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/WEB"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"project-1\"")
                .set_body_json(project_with_archived("WEB", 1, None)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/WEB/archive"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(project_with_archived(
                "WEB",
                2,
                Some("2026-04-01T00:00:00Z"),
            )),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org", "acme", "project", "archive", "WEB", "--force", "--json",
        ])
        .assert()
        .success();

    // --force skips the preflight GET, so the archive POST is the only request.
    let req = &server.received_requests().await.unwrap()[0];
    let body: Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body, json!({}));
    assert_eq!(req.headers.get("if-match").unwrap().to_str().unwrap(), "*");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_activity_renders_feed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/activity"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "a1", "action": "created", "actor": null, "detail": null,
             "createdAt": "2026-01-02T00:00:00Z",
             "workItem": {"id": "1", "key": "HAM-1", "title": "T"}}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "project",
            "activity",
            "--json",
        ])
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_list_archived_flag_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "project", "list", "--archived", "true"])
        .assert()
        .success();

    let query = server.received_requests().await.unwrap()[0]
        .url
        .query()
        .unwrap_or_default()
        .to_string();
    assert!(query.contains("archived=true"), "{query}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_members_renders_directory() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "Steven", "username": "steven"}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["org", "members", "acme", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["items"][0]["publicId"], "usr_cPbfeqnghA-RLpDVOMQhHg");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_work_lists_context_items() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "1", "key": "HAM-1", "revision": 1, "title": "T", "status": "todo",
             "assignee": {"publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "A"},
             "project": {"id": "p", "key": "HAM", "name": "Ham", "color": "#000000"},
             "organization": {"id": "o", "slug": "acme", "name": "Acme"}}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "org", "work", "--project", "HAM", "--json"])
        .assert()
        .success();

    let query = server.received_requests().await.unwrap()[0]
        .url
        .query()
        .unwrap_or_default()
        .to_string();
    assert!(query.contains("project=HAM"), "{query}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn org_work_mine_sends_assignee_me() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "org", "work", "--mine", "--json"])
        .assert()
        .success();

    let query = server.received_requests().await.unwrap()[0]
        .url
        .query()
        .unwrap_or_default()
        .to_string();
    assert!(query.contains("assignee=me"), "{query}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_list_mine_sends_assignee_me() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "list",
            "--mine",
            "--json",
        ])
        .assert()
        .success();

    let query = server.received_requests().await.unwrap()[0]
        .url
        .query()
        .unwrap_or_default()
        .to_string();
    assert!(query.contains("assignee=me"), "{query}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_view_shows_profile_summary() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/usr_cPbfeqnghA-RLpDVOMQhHg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "Steven",
            "avatarUrl": null, "joinedAt": "2026-01-15T14:30:00.000Z",
            "isCurrentUser": true, "sharedOrganizations": [],
            "stats": {"projects": 1, "workItemsAssigned": 2, "workItemsCreated": 3, "workItemsCompleted": 4, "comments": 5}
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["user", "view", "usr_cPbfeqnghA-RLpDVOMQhHg", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["publicId"], "usr_cPbfeqnghA-RLpDVOMQhHg");
    assert_eq!(body["stats"]["workItemsCompleted"], 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_work_lists_context_items() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/usr_cPbfeqnghA-RLpDVOMQhHg/work"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([{
            "id": "wi-1",
            "key": "HAM-7",
            "revision": 1,
            "title": "Profile work row",
            "status": "todo",
            "assignee": {"publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "Assignee"},
            "reporter": {"publicId": "usr_OtherUserPublicIdAAAAQ", "name": "Reporter"},
            "project": {"id": "p1", "key": "HAM", "name": "Ham", "color": "#000000"},
            "organization": {"id": "o1", "slug": "acme", "name": "Acme"}
        }]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "user",
            "work",
            "usr_cPbfeqnghA-RLpDVOMQhHg",
            "--involvement",
            "assigned",
            "--json",
        ])
        .assert()
        .success();

    let body: Value = serde_json::from_slice(output.get_output().stdout.as_slice()).unwrap();
    assert_eq!(body["items"][0]["reporter"]["name"], "Reporter");

    let query = server.received_requests().await.unwrap()[0]
        .url
        .query()
        .unwrap_or_default()
        .to_string();
    assert!(query.contains("involvement=assigned"), "{query}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_avatar_downloads_bytes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/usr_cPbfeqnghA-RLpDVOMQhHg/avatar"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "image/png")
                .insert_header("Cache-Control", "public, max-age=3600")
                .insert_header("X-Request-Id", "avatar-request")
                .set_body_bytes(vec![1, 2, 3]),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output_path = dir.path().join("avatar.png");
    let output = base(&server, &dir)
        .args([
            "user",
            "avatar",
            "usr_cPbfeqnghA-RLpDVOMQhHg",
            "--output",
            output_path.to_str().unwrap(),
            "--avatar-version",
            "v2",
            "--format",
            "png",
            "--revision",
            "opaque-rev",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(std::fs::read(&output_path).unwrap(), vec![1, 2, 3]);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["contentType"], "image/png");
    assert_eq!(body["cacheControl"], "public, max-age=3600");
    assert_eq!(body["requestId"], "avatar-request");
    let query = server.received_requests().await.unwrap()[0]
        .url
        .query()
        .unwrap()
        .to_string();
    assert!(query.contains("v=v2"), "{query}");
    assert!(query.contains("format=png"), "{query}");
    assert!(query.contains("rev=opaque-rev"), "{query}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_status_reports_public_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_public_json()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["auth", "status", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["user"]["publicId"], "usr_cPbfeqnghA-RLpDVOMQhHg");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn me_exposes_the_complete_identity_projection() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "u1",
            "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg",
            "name": "Steven",
            "email": "steven@example.com",
            "authentication": {
                "type": "pat",
                "credentialId": "credential-1",
                "credentialName": "automation",
                "scopes": ["profile:read", "work-item:read"],
                "expiresAt": "2027-01-01T00:00:00Z"
            },
            "defaultOrganization": {"id": "o1", "slug": "acme", "name": "Acme"},
            "organizations": [
                {"id": "o1", "slug": "acme", "name": "Acme", "username": "steven"},
                {"id": "o2", "slug": "labs", "name": "Labs", "username": null}
            ]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir).args(["me", "--json"]).output().unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["publicId"], "usr_cPbfeqnghA-RLpDVOMQhHg");
    assert_eq!(body["authentication"]["credentialName"], "automation");
    assert_eq!(body["defaultOrganization"]["slug"], "acme");
    assert_eq!(body["organizations"][0]["username"], "steven");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_openapi_is_available_without_a_token_and_sends_no_authorization() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/openapi.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "openapi": "3.1.1",
            "paths": {"/api/v1/me": {}}
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .env_remove("HAMSTIK_TOKEN")
        .args(["api", "openapi"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["openapi"], "3.1.1");
    let request = &server.received_requests().await.unwrap()[0];
    assert!(!request.headers.contains_key("authorization"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_search_uses_squeakql_json_post_without_idempotency() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/work-items/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([{
            "id": "wi-1", "key": "HAM-7", "revision": 3, "title": "Search result",
            "status": "todo",
            "project": {"id": "p1", "key": "HAM", "name": "Ham", "color": "#000000"},
            "organization": {"id": "o1", "slug": "acme", "name": "Acme"}
        }]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "work",
            "search",
            "status = todo and priority >= high",
            "--limit",
            "25",
            "--cursor",
            "opaque+/cursor==",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["items"][0]["key"], "HAM-7");
    let request = &server.received_requests().await.unwrap()[0];
    assert!(request.url.query().is_none());
    assert!(!request.headers.contains_key("idempotency-key"));
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["query"], "status = todo and priority >= high");
    assert_eq!(sent["limit"], 25);
    assert_eq!(sent["cursor"], "opaque+/cursor==");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn squeakql_validate_renders_diagnostics_and_preserves_json() {
    let server = MockServer::start().await;
    let validation = json!({
        "valid": false,
        "languageVersion": 1,
        "errors": [{
            "code": "SQUEAKQL_UNKNOWN_FIELD",
            "message": "Unknown field statuz",
            "line": 1,
            "column": 1,
            "endLine": 1,
            "endColumn": 6,
            "token": "statuz",
            "expected": ["status"],
            "suggestion": "status"
        }]
    });
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/squeakql/validate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(validation.clone()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "squeakql", "validate", "statuz = todo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("SQUEAKQL_UNKNOWN_FIELD"))
        .stdout(predicate::str::contains("suggestion: status"));
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "squeakql",
            "validate",
            "statuz = todo",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        validation
    );
    for request in server.received_requests().await.unwrap() {
        assert!(!request.headers.contains_key("idempotency-key"));
        assert_eq!(
            serde_json::from_slice::<Value>(&request.body).unwrap(),
            json!({"query": "statuz = todo"})
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_mine_exposes_all_current_filters_with_repeated_values() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/my/work"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "work",
            "mine",
            "--project",
            "HAM",
            "--project",
            "WEB",
            "--status",
            "todo",
            "--status",
            "in_progress",
            "--scope",
            "open",
            "--type",
            "task",
            "--priority",
            "high",
            "--label",
            "label-1",
            "--label-name",
            "api",
            "--overdue",
            "false",
            "--due-before",
            "2026-10-01T00:00:00Z",
            "--due-after",
            "2026-09-01T00:00:00Z",
            "--sort",
            "dueDate",
            "--archived",
            "true",
            "--fields",
            "title,status,dueDate",
            "--limit",
            "17",
            "--cursor",
            "opaque+/cursor==",
            "--json",
        ])
        .assert()
        .success();

    let request = &server.received_requests().await.unwrap()[0];
    let pairs: Vec<(String, String)> = request
        .url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(pairs.iter().filter(|(key, _)| key == "project").count(), 2);
    assert_eq!(pairs.iter().filter(|(key, _)| key == "status").count(), 2);
    assert!(pairs.contains(&("overdue".into(), "false".into())));
    assert!(pairs.contains(&("archived".into(), "true".into())));
    assert!(pairs.contains(&("cursor".into(), "opaque+/cursor==".into())));
    assert!(pairs.contains(&("fields".into(), "title,status,dueDate".into())));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn structured_api_errors_preserve_fields_details_and_request_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {
                "code": "VALIDATION_ERROR",
                "message": "Invalid organization",
                "fieldErrors": {"organizationSlug": ["has an invalid shape"]},
                "details": {"received": "acme", "rule": "slug"}
            },
            "requestId": "req-validation-1"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["org", "view", "acme"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "organizationSlug: has an invalid shape",
        ))
        .stderr(predicate::str::contains("request id: req-validation-1"));
    let output = base(&server, &dir)
        .args(["org", "view", "acme", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(body["error"]["requestId"], "req-validation-1");
    assert_eq!(
        body["error"]["fieldErrors"]["organizationSlug"][0],
        "has an invalid shape"
    );
    assert_eq!(body["error"]["details"]["rule"], "slug");
}

// ---- Exit-code coverage, prompt gating, and retry replay -------------------

/// A `hamstik` command wired to `server` with retries left at the default
/// policy (unlike [`base`], which disables them).
fn retrying_command(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd
}

/// 403/INSUFFICIENT_SCOPE maps to exit 4 (SPEC §52).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_maps_to_exit_four() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": {"code": "INSUFFICIENT_SCOPE", "message": "missing work-item:read scope"},
            "requestId": "r-forbidden"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM", "work", "view", "HAM-1"])
        .assert()
        .code(4)
        .stderr(predicate::str::contains("INSUFFICIENT_SCOPE"));
}

/// 429 with no usable `Retry-After` maps to exit 7 (SPEC §52).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limited_maps_to_exit_seven() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({
            "error": {"code": "RATE_LIMITED", "message": "too many requests"},
            "requestId": "r-limited"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM", "work", "view", "HAM-1"])
        .assert()
        .code(7)
        .stderr(predicate::str::contains("RATE_LIMITED"));
}

/// A transport failure maps to exit 8 (SPEC §52).
#[test]
fn network_failure_maps_to_exit_eight() {
    // Bind then drop a loopback port so the connect attempt is refused
    // immediately, with no external network dependency.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let dir = TempDir::new().unwrap();
    config_command(&dir)
        .env("HAMSTIK_HOST", format!("http://127.0.0.1:{port}"))
        .env("HAMSTIK_TOKEN", "secret-token")
        .args(["--no-retry", "auth", "status"])
        .assert()
        .code(8);
}

/// 412/REVISION_CONFLICT on an ETag-protected edit maps to exit 6, and the
/// edit sends `If-Match` from the freshly read revision.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revision_conflict_maps_to_exit_six() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-3\"")
                .set_body_json(work_item_json("todo", 3)),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(ResponseTemplate::new(412).set_body_json(json!({
            "error": {
                "code": "REVISION_CONFLICT",
                "message": "revision changed",
                "details": {"expectedRevision": 3, "currentRevision": 4}
            },
            "requestId": "r-conflict"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "edit",
            "HAM-1",
            "--title",
            "Changed",
        ])
        .assert()
        .code(6)
        .stderr(predicate::str::contains("REVISION_CONFLICT"));

    let requests = server.received_requests().await.unwrap();
    let patch = requests
        .iter()
        .find(|request| request.method == wiremock::http::Method::PATCH)
        .expect("PATCH request");
    assert_eq!(
        patch.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"wi-3\""
    );
}

/// `--no-input` turns a missing required value into a usage error before any
/// request is sent; it can never hang waiting for a prompt (PRD §30).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_input_fails_instead_of_prompting() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .env("HAMSTIK_ORG", "acme")
        .env("HAMSTIK_PROJECT", "HAM")
        .args(["work", "create", "--no-input"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--title"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// A transient failure is retried with the *same* idempotency key, and a
/// server-signaled replay surfaces as a warning (SPEC §76).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_retries_with_same_idempotency_key_and_warns_on_replay() {
    let server = MockServer::start().await;
    let keys: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let first = Arc::new(AtomicBool::new(true));
    let seen_keys = keys.clone();
    let seen_first = first.clone();
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(move |request: &Request| {
            let key = request
                .headers
                .get("idempotency-key")
                .map(|value| value.to_str().unwrap().to_string())
                .unwrap_or_default();
            seen_keys.lock().unwrap().push(key);
            if seen_first.swap(false, Ordering::SeqCst) {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(201)
                    .insert_header("Idempotency-Replayed", "true")
                    .set_body_json(work_item_json("todo", 1))
            }
        })
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    retrying_command(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--title",
            "Retried",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("replayed"));

    let keys = keys.lock().unwrap();
    assert_eq!(keys.len(), 2, "expected a retry");
    assert!(!keys[0].is_empty(), "idempotency key must be generated");
    assert_eq!(
        keys[0], keys[1],
        "retry must reuse the same idempotency key"
    );
}

/// `--all` follows cursors and aggregates every page (SPEC §43).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_list_all_follows_cursor_pages() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(move |request: &Request| {
            let cursor = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "cursor")
                .map(|(_, value)| value.into_owned());
            match cursor.as_deref() {
                None => ResponseTemplate::new(200).set_body_json(json!({
                    "items": [work_item_json("todo", 1)],
                    "page": {"limit": 50, "hasMore": true, "nextCursor": "page-two"}
                })),
                Some("page-two") => ResponseTemplate::new(200).set_body_json(json!({
                    "items": [work_item_json("done", 2)],
                    "page": {"limit": 50, "hasMore": false, "nextCursor": null}
                })),
                Some(other) => panic!("unexpected cursor {other}"),
            }
        })
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "list",
            "--all",
            "--limit",
            "2",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 2);
    assert_eq!(body["items"][0]["status"], "todo");
    assert_eq!(body["items"][1]["status"], "done");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        let limit = request
            .url
            .query_pairs()
            .find(|(key, _)| key == "limit")
            .map(|(_, value)| value.into_owned());
        assert_eq!(
            limit.as_deref(),
            Some("2"),
            "--limit must apply to every page"
        );
    }
}

/// `auth login` must not persist `HAMSTIK_TOKEN`: an environment token is
/// ephemeral (SPEC §27, PRD §6.3).
#[test]
fn auth_login_rejects_env_token() {
    let dir = TempDir::new().unwrap();
    config_command(&dir)
        .env("HAMSTIK_TOKEN", "secret-token")
        .args(["auth", "login"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("ephemeral"));
    assert!(
        !dir.path().join("config.toml").exists(),
        "login must not write profile state when it refuses"
    );
}

/// `auth switch --json` emits JSON rather than a human line.
#[test]
fn auth_switch_json_emits_json() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("config.toml"), TWO_PROFILES).unwrap();

    let output = config_command(&dir)
        .args(["auth", "switch", "two", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["activeProfile"], "two");
}

/// `context init --json` and `context clear --json` emit JSON.
#[test]
fn context_init_and_clear_honor_json() {
    let dir = TempDir::new().unwrap();
    let output = config_command(&dir)
        .args(["context", "init", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["created"], true);
    assert!(
        body["contextFile"]
            .as_str()
            .unwrap()
            .ends_with(".hamstik.toml")
    );

    let output = config_command(&dir)
        .args(["context", "clear", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["cleared"], true);
}

/// `--title` and `--clear-description` are independent and may be combined.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_edit_title_with_clear_description_is_accepted() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-1\"")
                .set_body_json(work_item_json("todo", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_item_json("todo", 2)))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "edit",
            "HAM-1",
            "--title",
            "Renamed",
            "--clear-description",
        ])
        .assert()
        .success();

    let patch = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.method == wiremock::http::Method::PATCH)
        .expect("PATCH request");
    let body: Value = serde_json::from_slice(&patch.body).unwrap();
    assert_eq!(body["title"], "Renamed");
    assert!(
        body["description"].is_null(),
        "clear sends an explicit null"
    );
}

/// A local I/O failure maps to exit 1 (general failure).
#[test]
fn local_io_failure_maps_to_exit_one() {
    let dir = TempDir::new().unwrap();
    config_command(&dir)
        .env("HAMSTIK_HOST", "http://127.0.0.1:1")
        .env("HAMSTIK_TOKEN", "secret-token")
        .env("HAMSTIK_ORG", "acme")
        .env("HAMSTIK_PROJECT", "HAM")
        .args([
            "--no-retry",
            "work",
            "edit",
            "HAM-1",
            "--description-file",
            "/nonexistent/hamstik-nope.md",
        ])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("cannot read text"));
}

/// Bulk operations read from stdin are size-capped before buffering.
#[test]
fn bulk_operations_stdin_is_capped() {
    let dir = TempDir::new().unwrap();
    let big = "x".repeat(1024 * 1024 + 1);
    config_command(&dir)
        .env("HAMSTIK_HOST", "http://127.0.0.1:1")
        .env("HAMSTIK_TOKEN", "secret-token")
        .env("HAMSTIK_ORG", "acme")
        .env("HAMSTIK_PROJECT", "HAM")
        .args([
            "--no-retry",
            "work",
            "bulk",
            "create",
            "--operations-file",
            "-",
        ])
        .write_stdin(big)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("byte limit"));
}
