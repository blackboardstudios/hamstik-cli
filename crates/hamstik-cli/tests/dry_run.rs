// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Dry-run preview tests: `--dry-run` resolves inputs exactly as a
//! real invocation would but never sends a mutation request, and emits a
//! versioned preview envelope (JSON mode) or a labeled human summary.

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn work_item_json(status: &str, revision: i64) -> Value {
    json!({
        "id": "1", "key": "HAM-1", "projectId": "2", "title": "T", "description": null,
        "type": "task", "status": status, "priority": "low", "assignee": null, "reporter": null,
        "sprint": null, "parent": null, "labels": [],
        "storyPoints": null, "dueDate": null, "archivedAt": null,
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": revision
    })
}

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
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

/// A command with no server: a dry run must succeed without any endpoint.
fn local(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env("HAMSTIK_HOST", "http://127.0.0.1:1");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

/// The versioned preview envelope: operation, method/path template, resolved
/// path, sanitized headers, and typed body. Authorization never appears.
#[test]
fn create_preview_matches_documented_envelope() {
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
            "work",
            "create",
            "--title",
            "Contract work",
            "--idempotency-key",
            "contract-dry-run-1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    assert!(
        output.stderr.is_empty(),
        "dry-run success must not write to stderr"
    );

    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["dryRun"], true, "preview must be labeled dryRun");
    assert_eq!(body["previewVersion"], 1, "previewVersion drifted");
    assert_eq!(body["operation"], "work.create");
    assert_eq!(body["request"]["method"], "POST");
    assert_eq!(
        body["request"]["pathTemplate"],
        "/api/v1/organizations/{organization}/projects/{project}/work-items"
    );
    assert_eq!(
        body["request"]["path"],
        "/api/v1/organizations/acme/projects/HAM/work-items"
    );
    assert_eq!(
        body["request"]["headers"]["Idempotency-Key"], "contract-dry-run-1",
        "idempotency intent must be visible"
    );
    assert!(
        body["request"]["headers"].get("Authorization").is_none(),
        "Authorization must never appear in a preview"
    );
    assert_eq!(body["body"]["title"], "Contract work", "body drifted");
    assert_eq!(body["resolved"]["organization"], "acme");
    assert_eq!(body["resolved"]["project"], "HAM");
}

/// `If-Match: *` appears only when the user explicitly forced
/// last-write-wins; otherwise the preview shows the ETag fetched by the safe
/// read. No mutation is sent in either case.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edit_preview_etag_and_force_semantics() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-42\"")
                .set_body_json(work_item_json("todo", 7)),
        )
        .mount(&server)
        .await;
    // Any mutation attempt would fail the test below via received_requests().
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(ResponseTemplate::new(500))
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
            "work",
            "edit",
            "HAM-1",
            "--title",
            "New title",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));

    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        body["request"]["headers"]["If-Match"], "\"wi-42\"",
        "dry-run must surface the ETag obtained by the safe read"
    );
    assert_eq!(body["resolved"]["workItem"], "HAM-1");

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
            "work",
            "edit",
            "HAM-1",
            "--title",
            "New title",
            "--force",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        body["request"]["headers"]["If-Match"], "*",
        "`If-Match: *` must appear only when the user explicitly forced it"
    );

    // Zero mutation requests were sent across both invocations: the
    // non-force run performed exactly one ETag read; the forced run made
    // no request at all.
    let requests = server.received_requests().await.unwrap();
    for request in &requests {
        assert_eq!(
            request.method,
            "GET",
            "dry-run sent a non-read request: {} {}",
            request.method,
            request.url.path()
        );
    }
    assert_eq!(
        requests.len(),
        1,
        "only the non-forced invocation may read (one ETag fetch)"
    );
}

/// Force mode makes zero network requests: no read, no mutation.
#[test]
fn forced_dry_run_makes_no_requests() {
    let dir = TempDir::new().unwrap();
    // No server is running on this port; any request would fail the command.
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
            "delete",
            "HAM-1",
            "--force",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["operation"], "work.delete");
    assert_eq!(body["request"]["method"], "DELETE");
    assert_eq!(body["request"]["headers"]["If-Match"], "*");
    assert_eq!(body["body"]["cascade"], false);
}

/// Attachment upload previews metadata without file bytes and without a
/// server; stdin/file sources are validated for shape and size first.
#[test]
fn upload_preview_shows_metadata_not_bytes() {
    let dir = TempDir::new().unwrap();
    let upload = dir.path().join("payload.bin");
    std::fs::write(&upload, b"binary-payload").unwrap();

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
            "attachment",
            "upload",
            "HAM-1",
        ])
        .arg(&upload)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);

    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        !text.contains("binary-payload"),
        "preview must not leak file bytes"
    );
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["operation"], "work.attachment.upload");
    assert_eq!(body["resolved"]["fileName"], "payload.bin");
    assert_eq!(body["resolved"]["size"], 14, "size metadata drifted");
    assert_eq!(body["body"]["multipart"], "form-data");
    assert!(
        body["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|note| note.as_str().unwrap_or("").contains("will not be uploaded")),
        "upload preview must state that bytes are not uploaded"
    );
}

/// Bulk previews read and shape-validate the operations file, then emit the
/// full envelope the real invocation would send.
#[test]
fn bulk_preview_reads_and_validates_operations_file() {
    let dir = TempDir::new().unwrap();
    let operations = dir.path().join("ops.json");
    std::fs::write(
        &operations,
        r#"[{"projectKey":"HAM","title":"One"},{"projectKey":"HAM","title":"Two"}]"#,
    )
    .unwrap();

    let output = local(&dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "work",
            "bulk",
            "create",
            "--operations-file",
        ])
        .arg(&operations)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["operation"], "work.bulk.create");
    assert_eq!(body["resolved"]["operationCount"], 2);
    assert_eq!(
        body["body"]["operations"].as_array().unwrap().len(),
        2,
        "bulk preview must carry the full typed operations body"
    );

    // Shape validation happens before preview: an invalid file fails.
    let invalid = dir.path().join("invalid.json");
    std::fs::write(&invalid, r#"[{"nope": true}]"#).unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "work",
            "bulk",
            "create",
            "--operations-file",
        ])
        .arg(&invalid)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "bulk dry-run must validate input shape before preview"
    );

    // An empty operations file is rejected as in real execution.
    let empty = dir.path().join("empty.json");
    std::fs::write(&empty, "[]").unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "work",
            "bulk",
            "create",
            "--operations-file",
        ])
        .arg(&empty)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

/// Human mode prints a labeled client-side-preview summary with the detail
/// block; quiet mode prints only the one-line summary.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_and_quiet_previews_are_labeled_client_side() {
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
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/transitions",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"currentStatus":"todo","transitions":[{"targetStatus":"in_progress"}]}),
        ))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "transition",
            "HAM-1",
            "in_progress",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains(
            "dry-run: POST /api/v1/organizations/acme/projects/HAM/work-items/HAM-1/transitions"
        ),
        "human preview must lead with the dry-run summary line: {text}"
    );
    assert!(
        text.contains("client-side preview only"),
        "human preview must label itself as client-side"
    );
    assert!(
        text.contains("If-Match: \"wi-1\""),
        "human preview must show If-Match intent"
    );
    assert!(
        !text.contains("secret-token"),
        "the bearer token must never appear in preview output"
    );

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--quiet",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "transition",
            "HAM-1",
            "in_progress",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1, "quiet preview must be one line: {text:?}");
    assert!(lines[0].starts_with("dry-run: POST "));
}

/// Reads reject `--dry-run` with a usage error naming the constraint.
#[test]
fn read_commands_reject_dry_run() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "work",
            "list",
            "--org",
            "acme",
            "--project",
            "HAM",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(body["error"]["code"], "INVALID_INPUT");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("--dry-run applies only to mutation commands"),
        "rejection message drifted"
    );

    let output = local(&dir)
        .args(["--no-input", "--json", "--dry-run", "me"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

/// Preview output is deterministic apart from generated idempotency keys:
/// identical inputs produce identical envelopes.
#[test]
fn preview_envelope_is_deterministic() {
    let run = || {
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
                "work",
                "create",
                "--title",
                "Same",
                "--type",
                "bug",
                "--priority",
                "high",
                "--idempotency-key",
                "deterministic-key-1",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        let mut body: Value = serde_json::from_slice(&output.stdout).unwrap();
        // The generated idempotency key is intentionally random; pin its
        // presence and shape, then strip it for the determinism comparison.
        let key = body["request"]["headers"]["Idempotency-Key"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(key, "deterministic-key-1");
        body["request"]["headers"]
            .as_object_mut()
            .unwrap()
            .remove("Idempotency-Key");
        body
    };
    assert_eq!(
        run(),
        run(),
        "identical inputs must produce identical previews"
    );
}
