// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

#![allow(clippy::expect_used, clippy::unwrap_used)]

//! CLI contract tests for `release` and `milestone`: previews, server flows,
//! consent gating, pagination, and output modes.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.env_remove("NO_COLOR");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn local(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env("HAMSTIK_HOST", "http://127.0.0.1:1");
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

const RELEASE_ID: &str = "22222222-2222-2222-2222-222222222222";
const MILESTONE_ID: &str = "44444444-4444-4444-4444-444444444444";
const REPORT_ID: &str = "77777777-7777-7777-7777-777777777777";

fn release_json(state: &str, revision: i64) -> Value {
    json!({
        "id": RELEASE_ID, "projectId": "p1", "name": "Hamstik 0.4.0",
        "displayVersion": "0.4.0", "description": null, "ownerId": null,
        "ownerPublicId": null, "ownerName": null, "createdById": null,
        "creatorPublicId": null, "creatorName": null, "targetDate": null,
        "releaseDate": null, "state": state, "stateBeforeArchive": null,
        "firstReleasedAt": null, "archivedAt": null, "revision": revision,
        "createdAt": "2026-09-01T00:00:00Z", "updatedAt": "2026-09-01T00:00:00Z"
    })
}

fn milestone_json(state: &str, revision: i64) -> Value {
    json!({
        "id": MILESTONE_ID, "organizationId": "o1", "name": "Q4 hardening",
        "description": null, "ownerId": null, "ownerPublicId": null,
        "ownerName": null, "targetDate": null, "state": state,
        "stateBeforeArchive": null, "completedAt": null, "archivedAt": null,
        "revision": revision, "createdAt": "2026-09-01T00:00:00Z",
        "updatedAt": "2026-09-01T00:00:00Z"
    })
}

#[test]
fn release_create_dry_run_preview_reports_operation_and_key() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "create",
            "--name",
            "Hamstik 0.4.0",
            "--display-version",
            "0.4.0",
            "--description",
            "Ships releases.",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(preview["operation"], "release.create");
    assert_eq!(preview["request"]["method"], "POST");
    assert_eq!(
        preview["request"]["pathTemplate"],
        "/api/v1/organizations/{organization}/projects/{project}/releases"
    );
    assert!(preview["request"]["headers"]["Idempotency-Key"].is_string());
    assert_eq!(preview["body"]["displayVersion"], "0.4.0");
}

#[test]
fn release_edit_without_changes_fails_usage() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "edit",
            RELEASE_ID,
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no changes specified"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn release_transition_sends_if_match_and_prevalidates_targets() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/releases/{RELEASE_ID}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"release-1\"")
                .set_body_json(release_json("planned", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/releases/{RELEASE_ID}/transitions"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currentState": "planned",
            "transitions": [{"targetState": "in_progress"}, {"targetState": "released"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/releases/{RELEASE_ID}/transitions"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"release-2\"")
                .set_body_json(release_json("released", 2)),
        )
        .mount(&server)
        .await;

    // An impossible target fails locally and never reaches the transition
    // endpoint (only the two GETs are served). `archived` would hit the
    // consent gate first, so an ungated spelling is used here.
    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "transition",
            RELEASE_ID,
            "planned",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not allowed from planned"));

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "transition",
            RELEASE_ID,
            "released",
            "--reason",
            "GA",
        ])
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    let post = requests
        .iter()
        .find(|request| {
            request.method.as_str() == "POST" && request.url.path().ends_with("/transitions")
        })
        .expect("transition POST");
    assert_eq!(
        post.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"release-1\""
    );
    let sent: Value = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(sent["targetState"], "released");
    assert_eq!(sent["reason"], "GA");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn release_archive_requires_consent_and_restores_without_it() {
    let server = MockServer::start().await;
    // No mocks: a consent failure must never touch the network.
    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "archive",
            RELEASE_ID,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--confirm-destructive"));

    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/releases/{RELEASE_ID}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"release-1\"")
                .set_body_json(release_json("released", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/releases/{RELEASE_ID}/archive"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"release-2\"")
                .set_body_json(release_json("archived", 2)),
        )
        .mount(&server)
        .await;

    base(&server, &dir)
        .args([
            "--json",
            "--yes",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "archive",
            RELEASE_ID,
            "--reason",
            "superseded",
        ])
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    let archive = requests
        .iter()
        .find(|request| request.url.path().ends_with("/archive"))
        .expect("archive POST");
    let sent: Value = serde_json::from_slice(&archive.body).unwrap();
    assert_eq!(sent["reason"], "superseded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn release_list_supports_follow_all_and_output_modes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/releases"))
        .and(query_param("cursor", "second"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [release_json("released", 2)],
            "page": {"limit": 1, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/releases"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [release_json("planned", 1)],
            "page": {"limit": 1, "hasMore": true, "nextCursor": "second"}
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "list",
            "--all",
            "--limit",
            "10",
        ])
        .assert()
        .success();

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "list",
            "--state",
            "planned",
            "--include-archived",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Hamstik 0.4.0"), "{stdout}");
    assert!(stdout.contains("planned"), "{stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn release_item_flows_preview_and_server_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-8\"")
                .set_body_json(work_item_json(8)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/releases",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "workItemId": "1",
            "workItemRevision": 9,
            "releaseVersions": [{
                "id": RELEASE_ID,
                "name": "Hamstik 0.4.0",
                "displayVersion": "0.4.0",
                "state": "planned",
                "archivedAt": null
            }]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "item",
            "add",
            "HAM-1",
            RELEASE_ID,
            "--reason",
            "backfilled",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(preview["operation"], "release.item.add");
    assert_eq!(preview["body"]["mode"], "add");
    assert_eq!(preview["request"]["headers"]["If-Match"], "\"wi-8\"");

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "item",
            "replace",
            "HAM-1",
            RELEASE_ID,
            "--confirm-released-scope-correction",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["workItemRevision"], 9);

    let requests = server.received_requests().await.unwrap();
    let post = requests
        .iter()
        .find(|request| request.method.as_str() == "POST")
        .expect("membership POST");
    let sent: Value = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(sent["mode"], "replace");
    assert_eq!(sent["confirmReleasedScopeCorrection"], true);
}

fn work_item_json(revision: i64) -> Value {
    json!({
        "id":"1","key":"HAM-1","projectId":"2","title":"T","description":null,
        "type":"task","status":"todo","priority":"low","assignee":null,"reporter":null,
        "sprint":null,"parent":null,"labels":[],"storyPoints":null,"dueDate":null,
        "archivedAt":null,"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z","revision":revision
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn milestone_create_view_transition_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/milestones"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("ETag", "\"milestone-1\"")
                .set_body_json(milestone_json("planned", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/milestones/{MILESTONE_ID}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"milestone-2\"")
                .set_body_json(json!({
                    "id": MILESTONE_ID, "organizationId": "o1", "name": "Q4 hardening",
                    "description": null, "ownerId": null, "ownerPublicId": null,
                    "ownerName": null, "targetDate": null, "state": "in_progress",
                    "stateBeforeArchive": null, "completedAt": null, "archivedAt": null,
                    "revision": 2, "createdAt": "2026-09-01T00:00:00Z",
                    "updatedAt": "2026-09-01T00:00:00Z",
                    "releases": [{
                        "id": RELEASE_ID, "name": "Hamstik 0.4.0", "state": "released",
                        "projectId": "p1", "projectName": "HAM", "projectKey": "HAM",
                        "targetDate": null, "releaseDate": "2026-09-24T00:00:00Z",
                        "totalWorkItems": 12, "completedWorkItems": 12
                    }],
                    "page": {"limit": 20, "hasMore": false, "nextAfter": null},
                    "scope": "authorized_projects"
                })),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/milestones/{MILESTONE_ID}/transitions"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currentState": "in_progress",
            "transitions": [{"targetState": "completed"}]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "milestone",
            "create",
            "--name",
            "Q4 hardening",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["state"], "planned");
    let post = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.method.as_str() == "POST")
        .unwrap();
    assert!(post.headers.get("idempotency-key").is_some());
    let sent: Value = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(sent["name"], "Q4 hardening");

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "milestone",
            "view",
            MILESTONE_ID,
            "--release-limit",
            "20",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Hamstik 0.4.0"), "{stdout}");
    assert!(stdout.contains("12/12 done"), "{stdout}");

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "milestone",
            "transition",
            MILESTONE_ID,
            "planned",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not allowed from in_progress"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn milestone_archive_transition_requires_consent() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "milestone",
            "transition",
            MILESTONE_ID,
            "archived",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--confirm-destructive"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn milestone_release_remove_requires_flag_and_add_round_trips() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/organizations/acme/milestones/{MILESTONE_ID}/releases"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"milestone-2\"")
                .set_body_json(milestone_json("in_progress", 2)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/milestones/{MILESTONE_ID}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"milestone-1\"")
                .set_body_json(json!({
                    "id": MILESTONE_ID, "organizationId": "o1", "name": "Q4 hardening",
                    "description": null, "ownerId": null, "ownerPublicId": null,
                    "ownerName": null, "targetDate": null, "state": "in_progress",
                    "stateBeforeArchive": null, "completedAt": null, "archivedAt": null,
                    "revision": 1, "createdAt": "2026-09-01T00:00:00Z",
                    "updatedAt": "2026-09-01T00:00:00Z",
                    "releases": [],
                    "page": {"limit": 20, "hasMore": false, "nextAfter": null},
                    "scope": "authorized_projects"
                })),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "milestone",
            "release",
            "remove",
            MILESTONE_ID,
            RELEASE_ID,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--confirm-remove"));

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "milestone",
            "release",
            "add",
            MILESTONE_ID,
            RELEASE_ID,
        ])
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    let post = requests
        .iter()
        .find(|request| request.method.as_str() == "POST")
        .expect("membership POST");
    let sent: Value = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(sent["mode"], "add");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn announcement_flows_render_draft_and_publish() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/releases/{RELEASE_ID}/announcements/draft"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "releaseVersionId": RELEASE_ID,
            "revision": 1,
            "introduction": "This release ships releases.",
            "highlights": ["Release versions"],
            "categories": [{"id": "features", "title": "Features", "workItemIds": ["1"]}],
            "items": [{
                "id": "1", "key": "HAM-1", "title": "Add releases", "itemType": "feature",
                "type": "feature", "status": "done", "include": true,
                "categoryId": "features", "position": 0
            }],
            "sourceReleaseRevision": 5,
            "generatedAt": "2026-09-02T00:00:00Z",
            "updatedAt": "2026-09-02T00:00:00Z",
            "diff": {"added": [], "removed": [], "changed": ["HAM-1"]}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/releases/{RELEASE_ID}/announcements/publish"
        )))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "66666666-6666-6666-6666-666666666666", "revision": 1
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
            "release",
            "announcement",
            "draft",
            "show",
            RELEASE_ID,
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("This release ships releases."), "{stdout}");
    assert!(stdout.contains("[x] HAM-1"), "{stdout}");
    assert!(stdout.contains("scope drift"), "{stdout}");

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "announcement",
            "publish",
            RELEASE_ID,
            "--confirm-empty",
            "--reason",
            "GA",
        ])
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    let publish = requests
        .iter()
        .find(|request| request.method.as_str() == "POST")
        .expect("publish POST");
    let sent: Value = serde_json::from_slice(&publish.body).unwrap();
    assert_eq!(sent["revision"], 1);
    assert_eq!(sent["confirmEmpty"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_flows_generate_list_and_download() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/release-audit-reports",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": REPORT_ID, "kind": "dossier",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "generatedAt": "2026-09-02T00:00:00Z"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/release-audit-reports",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{
                "id": REPORT_ID, "kind": "dossier",
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "generatedAt": "2026-09-02T00:00:00Z"
            }],
            "page": {"limit": 50, "nextCursor": null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/release-audit-reports/{REPORT_ID}"
        )))
        .and(query_param("format", "json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"report": "frozen"})))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "audit",
            "generate",
            "--kind",
            "dossier",
            "--release",
            RELEASE_ID,
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["kind"], "dossier");

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "audit",
            "list",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("dossier"));

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "audit",
            "get",
            REPORT_ID,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"frozen\""));

    // CSV requires --output.
    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "audit",
            "get",
            REPORT_ID,
            "--format",
            "csv",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--output"));
}

#[test]
fn release_bulk_membership_rejects_empty_envelope() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("ops.json"),
        json!({"operations": []}).to_string(),
    )
    .unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "release",
            "bulk-membership",
            "--file",
            "ops.json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("between 1 and 50 operations"), "{stderr}");
}
