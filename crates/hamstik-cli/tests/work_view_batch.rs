// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work view` batch-read contract tests (CLI-54): one invocation fetches
//! many items; the JSON envelope, per-item failure behavior, ordering, keys
//! files, section selection, and exit codes are all part of the contract.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
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

fn item_fixture(key: &str, title: &str) -> Value {
    json!({
        "id": format!("id-{key}"), "key": key, "projectId": "p1", "title": title,
        "description": format!("Description of {key}."), "type": "task", "status": "todo",
        "priority": "medium", "assignee": null, "reporter": {"id": "u2", "name": "Rae"},
        "sprint": null, "parent": null, "labels": [], "storyPoints": null, "dueDate": null,
        "archivedAt": null, "createdAt": "2026-09-01T00:00:00Z",
        "updatedAt": "2026-09-02T00:00:00Z", "revision": 1
    })
}

async fn mount_item(server: &MockServer, key: &str, title: &str) {
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/work-items/{key}"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(item_fixture(key, title)))
        .mount(server)
        .await;
}

async fn mount_sections(server: &MockServer, key: &str) {
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/work-items/{key}/comments"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": format!("c-{key}"), "workItemId": key,
                "author": {"id": "u1", "name": "A"}, "body": format!("body of {key}"),
                "deleted": false, "createdAt": "2026-09-02T01:00:00Z",
                "updatedAt": "2026-09-02T01:00:00Z", "editedAt": null,
                "parentCommentId": Value::Null}],
            "page": {"limit": 5, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/work-items/{key}/activity"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": format!("a-{key}"), "action": "created",
                "actor": {"id": "u1", "name": "A"}, "detail": Value::Null,
                "createdAt": "2026-09-01T00:00:00Z"}],
            "page": {"limit": 5, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/work-items/{key}/links"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id": format!("l-{key}"), "relation": "blocks", "otherWorkItem":
                {"id": "7", "key": "HAM-7", "project": {"id": "p1", "key": "HAM",
                    "name": "Ham"}, "title": "Other", "status": "todo", "type": "task"},
                "createdBy": {"id": "u2", "name": "Rae"},
                "createdAt": "2026-09-01T00:00:00Z"}],
            "page": {"limit": 5, "hasMore": false, "nextCursor": Value::Null}
        })))
        .mount(server)
        .await;
}

fn dir() -> TempDir {
    TempDir::new().unwrap()
}

fn run(server: &MockServer, extra: &[&str]) -> std::process::Output {
    base(server, &dir())
        .args(["--json", "--no-input", "--org", "acme", "--project", "HAM"])
        .args(extra)
        .output()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_envelope_reports_all_items_in_input_order() {
    let server = MockServer::start().await;
    mount_item(&server, "HAM-7", "Second").await;
    mount_item(&server, "HAM-42", "First").await;

    // Request HAM-42 last: output order must follow input, not completion.
    let output = run(&server, &["work", "view", "HAM-42", "HAM-7"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["total"], 2);
    assert_eq!(body["failures"], 0);
    assert_eq!(body["items"][0]["key"], "HAM-42");
    assert_eq!(body["items"][0]["status"], "ok");
    assert_eq!(body["items"][0]["item"]["title"], "First");
    assert_eq!(body["items"][1]["key"], "HAM-7");
    assert_eq!(body["items"][1]["item"]["title"], "Second");
    // The full server item rides along, including the description.
    assert_eq!(
        body["items"][0]["item"]["description"],
        "Description of HAM-42."
    );
    // Sections are absent unless requested.
    assert!(body["items"][0].get("comments").is_none());
    assert!(body["items"][0].get("activity").is_none());
    assert!(body["items"][0].get("links").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_item_is_reported_per_key_without_aborting_the_batch() {
    let server = MockServer::start().await;
    mount_item(&server, "HAM-42", "Present").await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-404",
        ))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"error": {"code": "NOT_FOUND", "message": "nope"}})),
        )
        .mount(&server)
        .await;

    let output = run(&server, &["work", "view", "HAM-42", "HAM-404"]);
    // stdout remains exactly one valid JSON envelope.
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["total"], 2);
    assert_eq!(body["failures"], 1);
    assert_eq!(body["items"][0]["status"], "ok");
    assert_eq!(body["items"][1]["key"], "HAM-404");
    assert_eq!(body["items"][1]["status"], "error");
    assert_eq!(body["items"][1]["error"]["status"], 404);
    // Exit code follows the partial-failure rule: the per-item NOT_FOUND code.
    assert_eq!(output.status.code(), Some(5));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_item_reports_authorization_exit_code() {
    let server = MockServer::start().await;
    mount_item(&server, "HAM-42", "Present").await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-999",
        ))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"error": {"code": "FORBIDDEN", "message": "no access"}})),
        )
        .mount(&server)
        .await;

    let output = run(&server, &["work", "view", "HAM-42", "HAM-999"]);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["failures"], 1);
    assert_eq!(body["items"][1]["error"]["status"], 403);
    assert_eq!(output.status.code(), Some(4));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn keys_file_reads_stdin_ignoring_comments_and_blanks() {
    let server = MockServer::start().await;
    mount_item(&server, "HAM-42", "First").await;
    mount_item(&server, "HAM-7", "Second").await;

    let output = base(&server, &dir())
        .args([
            "--json",
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "view",
            "--file",
            "-",
        ])
        .write_stdin("HAM-42\n# review list\n\nHAM-7\n")
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["total"], 2);
    assert_eq!(body["items"][0]["key"], "HAM-42");
    assert_eq!(body["items"][1]["key"], "HAM-7");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requested_sections_are_included_verbatim_per_item() {
    let server = MockServer::start().await;
    mount_item(&server, "HAM-42", "First").await;
    mount_item(&server, "HAM-7", "Second").await;
    mount_sections(&server, "HAM-42").await;
    mount_sections(&server, "HAM-7").await;

    let output = run(
        &server,
        &[
            "work",
            "view",
            "HAM-42",
            "HAM-7",
            "--comments",
            "5",
            "--activity",
            "5",
            "--links",
            "5",
        ],
    );
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        body["items"][0]["comments"]["items"][0]["body"],
        "body of HAM-42"
    );
    assert_eq!(
        body["items"][0]["activity"]["items"][0]["action"],
        "created"
    );
    assert_eq!(body["items"][0]["links"]["items"][0]["relation"], "blocks");
    assert_eq!(
        body["items"][1]["comments"]["items"][0]["body"],
        "body of HAM-7"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compact_replaces_description_and_comment_bodies_with_markers() {
    let server = MockServer::start().await;
    mount_item(&server, "HAM-42", "First").await;
    mount_item(&server, "HAM-7", "Second").await;
    mount_sections(&server, "HAM-42").await;
    mount_sections(&server, "HAM-7").await;

    let output = run(
        &server,
        &[
            "work",
            "view",
            "HAM-42",
            "HAM-7",
            "--compact",
            "--comments",
            "5",
        ],
    );
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        body["items"][0]["item"]["description"]["truncated"],
        serde_json::Value::Bool(true)
    );
    assert_eq!(
        body["items"][0]["comments"]["items"][0]["body"]["truncated"],
        serde_json::Value::Bool(true)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_key_keeps_single_item_output() {
    let server = MockServer::start().await;
    mount_item(&server, "HAM-42", "First").await;

    let output = run(&server, &["work", "view", "HAM-42"]);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    // Raw item at the top level — not the batch envelope.
    assert_eq!(body["key"], "HAM-42");
    assert_eq!(body["title"], "First");
    assert!(body.get("items").is_none());
    assert!(body.get("failures").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_key_with_sections_is_a_usage_error() {
    let server = MockServer::start().await;
    let output = run(&server, &["work", "view", "HAM-42", "--comments", "5"]);
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("work context"), "stderr: {stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn too_many_keys_is_a_usage_error() {
    let server = MockServer::start().await;
    let dir = dir();
    let keys_path = dir.path().join("keys.txt");
    let keys: Vec<String> = (0..501).map(|i| format!("HAM-{i}")).collect();
    std::fs::write(&keys_path, keys.join("\n")).unwrap();

    let output = base(&server, &dir)
        .args([
            "--json",
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "view",
            "--file",
            keys_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("too many keys"), "stderr: {stderr}");
}
