// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work context` read-bundle contract tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token-value");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

/// Mounts the five reads for one Work Item with a stable fixture.
async fn mount_item(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items/HAM-42"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "1", "key": "HAM-42", "projectId": "p1", "title": "Understand me",
            "description": "Long description text.", "type": "task", "status": "in_progress",
            "priority": "high", "assignee": {"id": "u1", "name": "Ada"},
            "reporter": {"id": "u2", "name": "Rae"}, "sprint": null, "parent": null,
            "labels": [{"id": "L1", "name": "cli", "color": "#00ff00"}], "storyPoints": 3, "dueDate": null,
            "archivedAt": null, "createdAt": "2026-09-01T00:00:00Z",
            "updatedAt": "2026-09-02T00:00:00Z", "revision": 5
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/links",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "l1", "relation": "blocks", "otherWorkItem":
                {"id": "7", "key": "HAM-7", "project": {"id": "p1", "key": "HAM", "name": "Ham"},
                 "title": "Other", "status": "todo", "type": "task"},
                "createdBy": {"id": "u2", "name": "Rae"}, "createdAt": "2026-09-01T00:00:00Z"}],
            "page": {"limit": 50, "hasMore": false, "nextCursor": null}
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/watcher",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "workItemId": "1", "watched": true, "muted": false, "manualWatch": true,
            "assigneeOrigin": false, "assigned": false
        })))
        .mount(server)
        .await;
}

// --json is a stable, deterministic layout with only server data.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_bundle_is_stable_and_composed_from_reads() {
    let server = MockServer::start().await;
    mount_item(&server).await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/comments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "c1", "workItemId": "1", "author": {"id": "u1", "name": "A"},
                            "body": "First comment", "deleted": false,
                "createdAt": "2026-09-02T01:00:00Z", "updatedAt": "2026-09-02T01:00:00Z",
                "editedAt": null, "parentCommentId": Value::Null}],
            "page": {"limit": 10, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/activity",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "a1", "action": "created", "actor": {"id": "u1", "name": "A"},
                "detail": Value::Null, "createdAt": "2026-09-01T00:00:00Z"}],
            "page": {"limit": 10, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(&server)
        .await;

    let output = base(&server, &dir())
        .args([
            "--json",
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "context",
            "HAM-42",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();

    // Deterministic layout, server data verbatim.
    assert_eq!(body["bundleVersion"], 1);
    assert_eq!(body["item"]["key"], "HAM-42");
    assert_eq!(body["item"]["status"], "in_progress");
    assert_eq!(body["item"]["storyPoints"], 3);
    assert_eq!(body["item"]["description"], "Long description text.");
    assert_eq!(body["links"]["items"][0]["relation"], "blocks");
    assert_eq!(body["comments"]["items"][0]["body"], "First comment");
    assert_eq!(body["activity"]["items"][0]["action"], "created");
    assert_eq!(body["watcher"]["watched"], true);
    // No client-side interpretation fields exist anywhere in the bundle.
    let bundle_text = serde_json::to_string(&body).unwrap();
    for computed in ["\"ready\"", "\"blocked\"", "\"isReady\"", "\"isBlocked\""] {
        assert!(
            !bundle_text.contains(computed),
            "client-side judgment found: {computed}"
        );
    }
}

fn dir() -> TempDir {
    TempDir::new().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn markdown_renders_sections_and_truncation_markers() {
    let server = MockServer::start().await;
    mount_item(&server).await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/comments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": "c1", "workItemId": "1", "author": {"id": "u1", "name": "A"}, "body": "Comment one",
                "deleted": false, "createdAt": "2026-09-02T01:00:00Z",
                "updatedAt": "2026-09-02T01:00:00Z", "editedAt": Value::Null,
                "parentCommentId": Value::Null}],
            "page": {"limit": 10, "hasMore": true, "nextCursor": Value::Null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/activity",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "page": {"limit": 10, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(&server)
        .await;

    let output = base(&server, &dir())
        .args([
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "context",
            "HAM-42",
            "--format",
            "markdown",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {:?} stderr: {:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("# HAM-42 — Understand me"));
    assert!(text.contains("## Links"));
    assert!(text.contains("- blocks HAM-7"));
    assert!(text.contains("## Comments"));
    // Explicit truncation marker for the clipped section.
    assert!(text.contains("more comments available; increase --comments"));
}

// --comments N / --activity N / --compact compose predictably with markers.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn size_zero_omits_sections_with_explicit_markers() {
    let server = MockServer::start().await;
    mount_item(&server).await;

    let output = base(&server, &dir())
        .args([
            "--json",
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "context",
            "HAM-42",
            "--comments",
            "0",
            "--activity",
            "0",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["comments"]["note"], "omitted by --comments 0");
    assert_eq!(body["activity"]["note"], "omitted by --activity 0");
    // Other sections are unaffected.
    assert_eq!(body["item"]["key"], "HAM-42");
    assert_eq!(body["links"]["items"][0]["relation"], "blocks");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_replaces_long_text_with_markers() {
    let server = MockServer::start().await;
    mount_item(&server).await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/comments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "page": {"limit": 10, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/activity",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "page": {"limit": 10, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(&server)
        .await;

    let output = base(&server, &dir())
        .args([
            "--json",
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "context",
            "HAM-42",
            "--compact",
        ])
        .output()
        .unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    // Long text is replaced with an explicit marker, not silently dropped.
    assert_eq!(
        body["item"]["description"],
        json!({"truncated": true, "note": "omitted by --compact"})
    );
    // Metadata is unaffected.
    assert_eq!(body["item"]["title"], "Understand me");
}

// Read-only: only GET requests are sent, and context resolution is
// deterministic.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bundle_is_read_only() {
    let server = MockServer::start().await;
    mount_item(&server).await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/comments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [], "page": {"limit": 10, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/activity",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [], "page": {"limit": 10, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(&server)
        .await;

    base(&server, &dir())
        .args([
            "--json",
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "context",
            "HAM-42",
        ])
        .assert()
        .success();
    for request in server.received_requests().await.unwrap() {
        assert_eq!(request.method, "GET", "only GET requests may be sent");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_item_fails_with_stable_exit_code() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-404",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {"code": "NOT_FOUND", "message": "no such work item"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-404/watcher",
        ))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let mut cmd = base(&server, &dir);
    cmd.env("HAMSTIK_ORG", "acme");
    cmd.env("HAMSTIK_PROJECT", "HAM");
    cmd.args(["--json", "--no-input", "work", "context", "HAM-404"]);
    cmd.assert()
        .code(5)
        .stderr(predicate::str::contains("NOT_FOUND"));
}

// The bundle must work under the standard global flag placements.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn global_flags_compose_with_format_flag() {
    let server = MockServer::start().await;
    mount_item(&server).await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/comments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [], "page": {"limit": 10, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/activity",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [], "page": {"limit": 10, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let mut cmd = base(&server, &dir);
    cmd.env("HAMSTIK_ORG", "acme");
    cmd.env("HAMSTIK_PROJECT", "HAM");
    cmd.args([
        "--no-input",
        "work",
        "context",
        "HAM-42",
        "--format",
        "json",
    ]);
    let output = cmd.output().unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["bundleVersion"], 1);
}
