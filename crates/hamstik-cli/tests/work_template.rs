// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Integration tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `work create --from`/`--template` convenience sources.
//!
//! These tests pin the documented allow-list (title/type/priority/description/
//! labels), the precedence of explicit flags, the local mutual-exclusion
//! error, and the fact that identity/ownership/state fields are never copied.

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
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
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

/// A command with no reachable server: any network call would fail loudly.
fn local(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env("HAMSTIK_HOST", "http://127.0.0.1:1");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn work_item(key: &str, status: &str, revision: i64) -> Value {
    json!({
        "id": "1", "key": key, "projectId": "2", "title": "T", "description": null,
        "type": "task", "status": status, "priority": "low",
        "assignee": null, "reporter": null, "sprint": null, "parent": null, "labels": [],
        "storyPoints": null, "dueDate": null, "archivedAt": null,
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": revision
    })
}

fn parse_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|err| {
        panic!(
            "stdout was not JSON: {err}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// A `--template` preview resolves the frontmatter plus the Markdown body.
#[test]
fn template_dry_run_resolves_frontmatter_and_body() {
    let dir = TempDir::new().unwrap();
    let template = dir.path().join("bug.md");
    std::fs::write(
        &template,
        "---\ntitle: Template title\ntype: bug\npriority: high\nlabels:\n  - regression\n---\n\n## Steps\n\nRepro\n",
    )
    .unwrap();

    let output = local(&dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--template",
            template.to_str().unwrap(),
            "--idempotency-key",
            "template-dry-run-1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);

    let body = parse_json(&output);
    assert_eq!(body["operation"], "work.create");
    assert_eq!(body["body"]["title"], "Template title");
    assert_eq!(body["body"]["type"], "bug");
    assert_eq!(body["body"]["priority"], "high");
    assert_eq!(body["body"]["description"], "## Steps\n\nRepro");
    assert_eq!(body["resolved"]["labels"], json!(["regression"]));
    assert!(
        body["notes"][0]
            .as_str()
            .unwrap()
            .contains("attached after create"),
        "the preview must name the post-create label attachments: {body}"
    );
}

/// Explicit flags always win over template values.
#[test]
fn explicit_flags_override_template_values() {
    let dir = TempDir::new().unwrap();
    let template = dir.path().join("bug.md");
    std::fs::write(
        &template,
        "---\ntitle: Template title\ntype: bug\npriority: high\n---\nTemplate body\n",
    )
    .unwrap();

    let output = local(&dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--template",
            template.to_str().unwrap(),
            "--title",
            "Override title",
            "--priority",
            "low",
            "--idempotency-key",
            "template-override-1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body = parse_json(&output);
    assert_eq!(body["body"]["title"], "Override title");
    assert_eq!(body["body"]["priority"], "low");
    // Non-overridden template values still apply.
    assert_eq!(body["body"]["type"], "bug");
    assert_eq!(body["body"]["description"], "Template body");
}

/// `--from` with an explicit override produces exactly the same create payload
/// as the equivalent hand-built `work create` invocation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_dry_run_matches_manual_construction() {
    let server = MockServer::start().await;
    let source = json!({
        "id": "1", "key": "HAM-1", "projectId": "2", "title": "Source title",
        "description": "Source body", "type": "bug", "status": "in_progress",
        "priority": "high",
        "assignee": {"publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "S"},
        "reporter": null,
        "sprint": null, "parent": null,
        "labels": [{"id": "11111111-1111-4111-8111-111111111111", "name": "api", "color": "#6366f1"}],
        "storyPoints": 8, "dueDate": "2027-01-01T00:00:00Z", "archivedAt": null,
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": 3
    });
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-3\"")
                .set_body_json(source),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let from = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--from",
            "HAM-1",
            "--title",
            "New title",
            "--idempotency-key",
            "from-dry-run-1",
        ])
        .output()
        .unwrap();
    assert_eq!(from.status.code(), Some(0), "{:?}", from.stderr);

    let manual = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--title",
            "New title",
            "--type",
            "bug",
            "--priority",
            "high",
            "--description",
            "Source body",
            "--idempotency-key",
            "from-dry-run-1",
        ])
        .output()
        .unwrap();
    assert_eq!(manual.status.code(), Some(0), "{:?}", manual.stderr);

    let from_body = parse_json(&from);
    let manual_body = parse_json(&manual);
    assert_eq!(
        from_body["body"], manual_body["body"],
        "`--from` must build the same create payload as explicit flags"
    );
    // Identity/ownership/state/scheduling fields are never copied.
    for forbidden in [
        "status",
        "assigneeId",
        "assigneePublicId",
        "sprintId",
        "parentId",
        "storyPoints",
        "dueDate",
    ] {
        assert!(
            from_body["body"].get(forbidden).is_none(),
            "`--from` must not copy {forbidden}: {from_body}"
        );
    }
}

/// `--from` and `--template` fail locally, before any request.
#[test]
fn from_and_template_conflict_before_any_network_call() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--from",
            "HAM-1",
            "--template",
            "template.md",
            "--title",
            "T",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "usage error expected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "mutual-exclusion usage error expected: {stderr}"
    );
}

/// Unknown frontmatter keys (including identity/state fields) are rejected
/// rather than silently ignored.
#[test]
fn template_rejects_unknown_frontmatter_keys_locally() {
    let dir = TempDir::new().unwrap();
    let template = dir.path().join("bad.md");
    std::fs::write(&template, "---\ntitle: T\nassignee: me\n---\n").unwrap();

    let output = local(&dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--template",
            template.to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "usage error expected");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid template"),
        "template shape error expected: {stderr}"
    );
}

/// A real `--from` create copies only the allow-list body fields and then
/// attaches the source labels through the existing label endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_create_copies_allow_list_and_attaches_labels() {
    let server = MockServer::start().await;
    let source = json!({
        "id": "1", "key": "HAM-1", "projectId": "2", "title": "Source title",
        "description": "Source body", "type": "bug", "status": "in_progress",
        "priority": "high",
        "assignee": {"publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "S"},
        "reporter": null,
        "sprint": null, "parent": null,
        "labels": [{"id": "11111111-1111-4111-8111-111111111111", "name": "api", "color": "#6366f1"}],
        "storyPoints": null, "dueDate": null, "archivedAt": null,
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": 3
    });
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-3\"")
                .set_body_json(source),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("ETag", "\"rev-1\"")
                .set_body_json(work_item("HAM-9", "todo", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-9/labels",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-2\"")
                .set_body_json(work_item("HAM-9", "todo", 2)),
        )
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
            "create",
            "--from",
            "HAM-1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body = parse_json(&output);
    assert_eq!(body["key"], "HAM-9");

    let requests = server.received_requests().await.unwrap();
    let create = requests
        .iter()
        .find(|request| request.url.path().ends_with("/work-items"))
        .expect("create request");
    let sent: Value = serde_json::from_slice(&create.body).unwrap();
    assert_eq!(sent["title"], "Source title");
    assert_eq!(sent["type"], "bug");
    assert_eq!(sent["priority"], "high");
    assert_eq!(sent["description"], "Source body");
    for forbidden in ["status", "assigneeId", "assigneePublicId"] {
        assert!(
            sent.get(forbidden).is_none(),
            "identity/state field {forbidden} must never be copied: {sent}"
        );
    }

    let attach = requests
        .iter()
        .find(|request| request.url.path().ends_with("/work-items/HAM-9/labels"))
        .expect("label attach request");
    let attach_body: Value = serde_json::from_slice(&attach.body).unwrap();
    assert_eq!(
        attach_body["labelId"], "11111111-1111-4111-8111-111111111111",
        "the copied label id must be attached after create"
    );
}
