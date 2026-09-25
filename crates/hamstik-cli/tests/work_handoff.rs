// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work export` / `work import` contract tests (CLI-66).
//!
//! These pin the exported document shape, the round-trip mapping onto the
//! existing create/edit requests, and the idempotency behavior that keeps a
//! re-import from duplicating items.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token-value");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn work_item(key: &str, revision: i64) -> Value {
    json!({
        "id": format!("id-{key}"), "key": key, "projectId": "p1",
        "title": "Fix the thing", "description": "## Steps\n\nRepro",
        "type": "bug", "status": "todo", "priority": "high",
        "assignee": null, "reporter": null, "sprint": null, "parent": null,
        "labels": [{"id": "L1", "name": "regression", "color": "#00ff00"}],
        "storyPoints": null, "dueDate": null, "archivedAt": null,
        "createdAt": "2026-09-01T00:00:00Z", "updatedAt": "2026-09-02T00:00:00Z",
        "revision": revision
    })
}

fn page(items: Value) -> Value {
    json!({"items": items, "page": {"limit": 50, "hasMore": false, "nextCursor": null}})
}

/// The document used by the import tests. It carries every documented field.
fn document() -> String {
    "---\n\
     key: GH-1234\n\
     title: Imported title\n\
     type: bug\n\
     status: todo\n\
     priority: high\n\
     labels:\n\
     \x20 - regression\n\
     links:\n\
     \x20 - relation: blocks\n\
     \x20   target: HAM-7\n\
     comments:\n\
     \x20 - author: Rae\n\
     \x20   body: A handoff comment\n\
     ---\n\
     \n## Imported body\n\nDetails.\n"
        .to_string()
}

async fn mount_absent(server: &MockServer, key: &str) {
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/work-items/{key}"
        )))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "code": "NOT_FOUND",
            "message": "no such work item",
        })))
        .mount(server)
        .await;
}

/// Export emits one canonical document: YAML frontmatter plus the description
/// as the Markdown body, with links always and comments on request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_writes_canonical_markdown_document() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_item("HAM-42", 5)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/links",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "l1", "relation": "blocks", "otherWorkItem": {
                "id": "7", "key": "HAM-7",
                "project": {"id": "p1", "key": "HAM", "name": "Ham"},
                "title": "Other", "status": "todo", "type": "task"},
             "createdBy": null, "createdAt": "2026-09-01T00:00:00Z"}
        ]))))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/comments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "c1", "workItemId": "id-HAM-42", "parentCommentId": null,
             "author": {"id": "u1", "name": "Rae"}, "body": "A comment",
             "deleted": false, "createdAt": "2026-09-02T00:00:00Z",
             "updatedAt": "2026-09-02T00:00:00Z", "editedAt": null}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--format",
            "markdown",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "export",
            "HAM-42",
            "--comments",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("---\n"), "{text}");
    assert!(text.contains("key: HAM-42"), "{text}");
    assert!(text.contains("title: Fix the thing"), "{text}");
    assert!(text.contains("type: bug"), "{text}");
    assert!(text.contains("status: todo"), "{text}");
    assert!(text.contains("priority: high"), "{text}");
    assert!(text.contains("regression"), "{text}");
    assert!(text.contains("HAM-7"), "{text}");
    assert!(text.contains("A comment"), "{text}");
    assert!(text.contains("## Steps\n\nRepro"), "{text}");
}

/// `--json` exposes the same fields as a structured envelope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_json_envelope_carries_the_documented_fields() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_item("HAM-42", 5)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/links",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "export",
            "HAM-42",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["documentVersion"], 1);
    assert_eq!(body["frontmatter"]["key"], "HAM-42");
    assert_eq!(body["frontmatter"]["title"], "Fix the thing");
    assert_eq!(body["frontmatter"]["labels"][0], "regression");
    assert_eq!(body["body"], "## Steps\n\nRepro");
    assert!(body["document"].as_str().unwrap().starts_with("---\n"));
}

/// Importing a document whose embedded key is absent creates one item and
/// applies labels, links, and comments through their documented endpoints.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_creates_item_and_applies_follow_ups() {
    let server = MockServer::start().await;
    mount_absent(&server, "GH-1234").await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .and(header("Idempotency-Key", "work-import:GH-1234"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("ETag", "\"rev-1\"")
                .set_body_json(work_item("HAM-99", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-99/labels",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-2\"")
                .set_body_json(work_item("HAM-99", 2)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-99/comments",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "c1", "workItemId": "id-HAM-99", "parentCommentId": null,
            "author": {"id": "u1", "name": "Rae"}, "body": "A handoff comment",
            "deleted": false, "createdAt": "2026-09-02T00:00:00Z",
            "updatedAt": "2026-09-02T00:00:00Z", "editedAt": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-99/links",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "l1", "relation": "blocks", "otherWorkItem": {
                "id": "7", "key": "HAM-7",
                "project": {"id": "p1", "key": "HAM", "name": "Ham"},
                "title": "Other", "status": "todo", "type": "task"},
            "createdBy": null, "createdAt": "2026-09-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let file = dir.path().join("handoff.md");
    std::fs::write(&file, document()).unwrap();

    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "import",
            "--file",
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["action"], "created");
    assert_eq!(body["key"], "GH-1234");
    assert_eq!(body["workItem"]["key"], "HAM-99");

    // The create request carries the mapped fields and the derived key.
    let requests = server.received_requests().await.unwrap();
    let create = requests
        .iter()
        .find(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/work-items"))
        .unwrap();
    let create_body: Value = serde_json::from_slice(&create.body).unwrap();
    assert_eq!(create_body["title"], "Imported title");
    assert_eq!(create_body["type"], "bug");
    assert_eq!(create_body["status"], "todo");
    assert_eq!(create_body["priority"], "high");
    assert_eq!(create_body["description"], "## Imported body\n\nDetails.");

    let label = requests
        .iter()
        .find(|r| r.url.path().ends_with("/HAM-99/labels"))
        .unwrap();
    let label_body: Value = serde_json::from_slice(&label.body).unwrap();
    assert_eq!(label_body["label"], "regression");

    let link = requests
        .iter()
        .find(|r| r.url.path().ends_with("/HAM-99/links"))
        .unwrap();
    let link_body: Value = serde_json::from_slice(&link.body).unwrap();
    assert_eq!(link_body["relation"], "blocks");
    assert_eq!(link_body["targetKey"], "HAM-7");

    let comment = requests
        .iter()
        .find(|r| r.url.path().ends_with("/HAM-99/comments"))
        .unwrap();
    let comment_body: Value = serde_json::from_slice(&comment.body).unwrap();
    assert_eq!(comment_body["body"], "A handoff comment");
}

/// Re-importing the same document reuses the derived idempotency key, so the
/// server replays the create instead of duplicating the item.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reimport_reuses_the_same_idempotency_key() {
    let server = MockServer::start().await;
    mount_absent(&server, "GH-1234").await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("ETag", "\"rev-1\"")
                .insert_header("Idempotency-Replayed", "true")
                .set_body_json(work_item("HAM-99", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-99/labels",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-2\"")
                .set_body_json(work_item("HAM-99", 2)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-99/comments",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "c1", "workItemId": "id-HAM-99", "parentCommentId": null,
            "author": {"id": "u1", "name": "Rae"}, "body": "A handoff comment",
            "deleted": false, "createdAt": "2026-09-02T00:00:00Z",
            "updatedAt": "2026-09-02T00:00:00Z", "editedAt": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-99/links",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "l1", "relation": "blocks", "otherWorkItem": {
                "id": "7", "key": "HAM-7",
                "project": {"id": "p1", "key": "HAM", "name": "Ham"},
                "title": "Other", "status": "todo", "type": "task"},
            "createdBy": null, "createdAt": "2026-09-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let file = dir.path().join("handoff.md");
    std::fs::write(&file, document()).unwrap();

    for _ in 0..2 {
        let output = base(&server, &dir)
            .args([
                "--no-input",
                "--quiet",
                "--org",
                "acme",
                "--project",
                "HAM",
                "work",
                "import",
                "--file",
                file.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "stderr: {:?}", output.stderr);
    }

    let requests = server.received_requests().await.unwrap();
    let keys: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/work-items"))
        .map(|r| {
            r.headers
                .get("Idempotency-Key")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(keys.len(), 2, "two imports must each create once");
    assert_eq!(keys[0], keys[1], "the derived key must be stable");
    assert_eq!(keys[0], "work-import:GH-1234");
}

/// An embedded key that already resolves updates in place and reconciles
/// labels/links/comments without adding duplicates.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_updates_existing_item_without_duplicating_follow_ups() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-5\"")
                .set_body_json(work_item("HAM-42", 5)),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-6\"")
                .set_body_json(work_item("HAM-42", 6)),
        )
        .mount(&server)
        .await;
    // The item already has the document's label, link, and comment.
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/links",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "l1", "relation": "blocks", "otherWorkItem": {
                "id": "7", "key": "HAM-7",
                "project": {"id": "p1", "key": "HAM", "name": "Ham"},
                "title": "Other", "status": "todo", "type": "task"},
             "createdBy": null, "createdAt": "2026-09-01T00:00:00Z"}
        ]))))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-42/comments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "c1", "workItemId": "id-HAM-42", "parentCommentId": null,
             "author": {"id": "u1", "name": "Rae"}, "body": "A handoff comment",
             "deleted": false, "createdAt": "2026-09-02T00:00:00Z",
             "updatedAt": "2026-09-02T00:00:00Z", "editedAt": null}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let doc = document().replace("key: GH-1234", "key: HAM-42");
    let file = dir.path().join("handoff.md");
    std::fs::write(&file, doc).unwrap();

    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "import",
            "--file",
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["action"], "updated");

    let requests = server.received_requests().await.unwrap();
    let patches = requests
        .iter()
        .filter(|r| r.method.as_str() == "PATCH")
        .count();
    assert_eq!(patches, 1, "exactly one edit");
    assert!(
        requests.iter().all(|r| r.method.as_str() != "POST"),
        "no follow-up may be duplicated: {requests:?}"
    );
    let patch = requests
        .iter()
        .find(|r| r.method.as_str() == "PATCH")
        .unwrap();
    let patch_body: Value = serde_json::from_slice(&patch.body).unwrap();
    assert_eq!(patch_body["title"], "Imported title");
    assert_eq!(patch.headers.get("If-Match").unwrap(), "\"rev-5\"");
}

/// `--dry-run` previews the mapped create without sending any mutation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_dry_run_previews_without_sending() {
    let server = MockServer::start().await;
    mount_absent(&server, "GH-1234").await;

    let dir = TempDir::new().unwrap();
    let file = dir.path().join("handoff.md");
    std::fs::write(&file, document()).unwrap();

    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "import",
            "--file",
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["dryRun"], true);
    assert_eq!(body["operation"], "work.import");
    assert_eq!(body["request"]["method"], "POST");
    assert_eq!(body["resolved"]["action"], "create");
    assert_eq!(body["body"]["title"], "Imported title");
    assert_eq!(
        body["request"]["headers"]["Idempotency-Key"],
        "work-import:GH-1234"
    );

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests.iter().all(|r| r.method.as_str() == "GET"),
        "dry-run must send no mutation: {requests:?}"
    );
}

/// The explicit `--idempotency-key` overrides the derived key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_honours_explicit_idempotency_key() {
    let server = MockServer::start().await;
    mount_absent(&server, "GH-1234").await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .and(header("Idempotency-Key", "explicit-import-key"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("ETag", "\"rev-1\"")
                .set_body_json(work_item("HAM-99", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-99/labels",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-2\"")
                .set_body_json(work_item("HAM-99", 2)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-99/comments",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "c1", "workItemId": "id-HAM-99", "parentCommentId": null,
            "author": {"id": "u1", "name": "Rae"}, "body": "A handoff comment",
            "deleted": false, "createdAt": "2026-09-02T00:00:00Z",
            "updatedAt": "2026-09-02T00:00:00Z", "editedAt": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-99/links",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "l1", "relation": "blocks", "otherWorkItem": {
                "id": "7", "key": "HAM-7",
                "project": {"id": "p1", "key": "HAM", "name": "Ham"},
                "title": "Other", "status": "todo", "type": "task"},
            "createdBy": null, "createdAt": "2026-09-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let file = dir.path().join("handoff.md");
    std::fs::write(&file, document()).unwrap();

    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--quiet",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "import",
            "--file",
            file.to_str().unwrap(),
            "--idempotency-key",
            "explicit-import-key",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
}
