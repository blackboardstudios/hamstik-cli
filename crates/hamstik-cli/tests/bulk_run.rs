// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `work bulk run`: batching, streamed input, and journal resume.

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
    cmd.env_remove("NO_COLOR");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd.arg("--no-input");
    cmd
}

/// Mounts a bulk endpoint that answers one `201` result per sent operation.
async fn mount_bulk(server: &MockServer, http_method: &str, route: &str) {
    Mock::given(method(http_method))
        .and(path(route))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap_or_else(|_| json!({}));
            let count = body
                .get("operations")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
            let results: Vec<Value> = (0..count)
                .map(|index| json!({ "index": index, "status": 201 }))
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({ "results": results }))
        })
        .mount(server)
        .await;
}

fn create_operations(count: usize) -> String {
    let operations: Vec<Value> = (0..count)
        .map(|index| json!({ "projectKey": "HAM", "title": format!("Item {index}") }))
        .collect();
    serde_json::to_string(&operations).unwrap()
}

fn idempotency_key(request: &wiremock::Request) -> String {
    request
        .headers
        .get("idempotency-key")
        .expect("bulk requests carry an idempotency key")
        .to_str()
        .unwrap()
        .to_string()
}

/// A run of more than 50 operations splits into batches of at most 50, each
/// with its own idempotency key, and states that batches are not atomic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_splits_operations_into_bounded_batches() {
    let server = MockServer::start().await;
    mount_bulk(
        &server,
        "POST",
        "/api/v1/organizations/acme/bulk-work-items",
    )
    .await;

    let dir = TempDir::new().unwrap();
    let ops = dir.path().join("ops.json");
    std::fs::write(&ops, create_operations(51)).unwrap();
    let journal = dir.path().join("run.jsonl");

    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "work",
            "bulk",
            "run",
            "--op",
            "create",
            "--operations-file",
        ])
        .arg(&ops)
        .arg("--journal")
        .arg(&journal)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        2,
        "51 operations must split into two batches"
    );
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    let second: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(first["operations"].as_array().unwrap().len(), 50);
    assert_eq!(second["operations"].as_array().unwrap().len(), 1);
    assert_ne!(
        idempotency_key(&requests[0]),
        idempotency_key(&requests[1]),
        "each batch owns a distinct idempotency key"
    );

    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["atomic"], false, "batches must be labeled non-atomic");
    assert_eq!(body["summary"]["batches"], 2);
    assert_eq!(body["summary"]["completed"], 2);
    assert_eq!(body["summary"]["operations"], 51);
    assert_eq!(body["batches"][0]["state"], "completed");

    // The journal is local and free of credentials.
    let text = std::fs::read_to_string(&journal).unwrap();
    assert!(
        !text.contains("secret-token"),
        "journal must never contain credentials"
    );
    for required in [
        "\"type\":\"header\"",
        "\"type\":\"batch\"",
        "\"type\":\"complete\"",
        "\"type\":\"result\"",
    ] {
        assert!(
            text.contains(required),
            "journal schema lost {required}: {text}"
        );
    }
}

/// JSON-lines on stdin is streamed one operation at a time into the existing
/// batch envelope.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_accepts_streamed_json_lines_on_stdin() {
    let server = MockServer::start().await;
    mount_bulk(
        &server,
        "POST",
        "/api/v1/organizations/acme/bulk-work-items",
    )
    .await;

    let dir = TempDir::new().unwrap();
    let journal = dir.path().join("run.jsonl");
    let mut cmd = base(&server, &dir);
    cmd.args([
        "--json",
        "--org",
        "acme",
        "work",
        "bulk",
        "run",
        "--op",
        "create",
        "--operations-file",
        "-",
        "--journal",
    ])
    .arg(&journal);
    cmd.write_stdin(
        "{\"projectKey\":\"HAM\",\"title\":\"One\"}\n\n{\"projectKey\":\"HAM\",\"title\":\"Two\"}\n",
    );
    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(sent["operations"].as_array().unwrap().len(), 2);
}

/// A crafted journal with one completed and one uncertain batch: resume skips
/// the completed batch and replays the uncertain batch with its original body
/// and idempotency key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_skips_completed_and_replays_uncertain_batch() {
    let server = MockServer::start().await;
    mount_bulk(
        &server,
        "POST",
        "/api/v1/organizations/acme/bulk-work-items",
    )
    .await;

    let dir = TempDir::new().unwrap();
    let journal = dir.path().join("resume.jsonl");
    let records = [
        json!({"type":"header","journalVersion":1,"operation":"create","organization":"acme","source":"ops.json","createdAt":"2026-01-01T00:00:00Z"}),
        json!({"type":"batch","sequence":0,"idempotencyKey":"batch-key-0000","operations":[{"projectKey":"HAM","title":"Done"}]}),
        json!({"type":"batch","sequence":1,"idempotencyKey":"batch-key-0001","operations":[{"projectKey":"HAM","title":"Retry"}]}),
        json!({"type":"complete","at":"2026-01-01T00:00:00Z"}),
        json!({"type":"result","sequence":0,"state":"completed","recordedAt":"2026-01-01T00:00:00Z","results":[{"index":0,"status":201}]}),
        json!({"type":"result","sequence":1,"state":"uncertain","attemptedAt":"2026-01-01T00:00:00Z","recordedAt":"2026-01-01T00:00:00Z"}),
    ];
    let text: String = records.iter().map(|record| format!("{record}\n")).collect();
    std::fs::write(&journal, text).unwrap();

    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "work",
            "bulk",
            "run",
            "--op",
            "create",
            "--journal",
        ])
        .arg(&journal)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "only the uncertain batch is re-sent");
    assert_eq!(idempotency_key(&requests[0]), "batch-key-0001");
    let sent: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        sent["operations"],
        json!([{"projectKey": "HAM", "title": "Retry"}]),
        "the original request body is reused verbatim"
    );

    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["resumed"], true);
    assert_eq!(
        body["summary"]["completed"], 2,
        "replayed batch is recorded"
    );
}

/// Failed batches are not retried unless `--retry-failed` is passed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_does_not_retry_failed_batches_without_the_flag() {
    let server = MockServer::start().await;
    mount_bulk(
        &server,
        "POST",
        "/api/v1/organizations/acme/bulk-work-items",
    )
    .await;

    let dir = TempDir::new().unwrap();
    let journal = dir.path().join("failed.jsonl");
    let records = [
        json!({"type":"header","journalVersion":1,"operation":"create","organization":"acme","source":"ops.json","createdAt":"2026-01-01T00:00:00Z"}),
        json!({"type":"batch","sequence":0,"idempotencyKey":"failed-key-0000","operations":[{"projectKey":"HAM","title":"Bad"}]}),
        json!({"type":"complete","at":"2026-01-01T00:00:00Z"}),
        json!({"type":"result","sequence":0,"state":"failed","attemptedAt":"2026-01-01T00:00:00Z","recordedAt":"2026-01-01T00:00:00Z","error":{"code":"VALIDATION_ERROR","message":"bad","status":400}}),
    ];
    let text: String = records.iter().map(|record| format!("{record}\n")).collect();
    std::fs::write(&journal, text).unwrap();

    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "work",
            "bulk",
            "run",
            "--op",
            "create",
            "--journal",
        ])
        .arg(&journal)
        .output()
        .unwrap();
    assert_ne!(
        output.status.code(),
        Some(0),
        "a remaining failed batch must fail the run"
    );
    assert!(server.received_requests().await.unwrap().is_empty());

    // With the explicit review flag the batch is re-sent with its stored key.
    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "work",
            "bulk",
            "run",
            "--op",
            "create",
            "--retry-failed",
            "--journal",
        ])
        .arg(&journal)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(idempotency_key(&requests[0]), "failed-key-0000");
}

/// An update journal's stored concurrency mode is reused on resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_reuses_stored_concurrency_mode() {
    let server = MockServer::start().await;
    mount_bulk(
        &server,
        "PATCH",
        "/api/v1/organizations/acme/bulk-work-items",
    )
    .await;

    let dir = TempDir::new().unwrap();
    let journal = dir.path().join("update.jsonl");
    let records = [
        json!({"type":"header","journalVersion":1,"operation":"update","organization":"acme","concurrency":"last-write-wins","source":"ops.json","createdAt":"2026-01-01T00:00:00Z"}),
        json!({"type":"batch","sequence":0,"idempotencyKey":"update-key-0000","operations":[{"projectKey":"HAM","workItemKey":"HAM-1","changes":{"title":"New"}}]}),
        json!({"type":"complete","at":"2026-01-01T00:00:00Z"}),
    ];
    let text: String = records.iter().map(|record| format!("{record}\n")).collect();
    std::fs::write(&journal, text).unwrap();

    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "work",
            "bulk",
            "run",
            "--op",
            "update",
            "--journal",
        ])
        .arg(&journal)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let sent: Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(sent["concurrency"], "last-write-wins");
}

/// Drift between a provided source and the planned batches is rejected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_rejects_a_source_that_differs_from_the_plan() {
    let server = MockServer::start().await;

    let dir = TempDir::new().unwrap();
    let journal = dir.path().join("drift.jsonl");
    let records = [
        json!({"type":"header","journalVersion":1,"operation":"create","organization":"acme","source":"ops.json","createdAt":"2026-01-01T00:00:00Z"}),
        json!({"type":"batch","sequence":0,"idempotencyKey":"drift-key-0000","operations":[{"projectKey":"HAM","title":"Original"}]}),
        json!({"type":"complete","at":"2026-01-01T00:00:00Z"}),
    ];
    let text: String = records.iter().map(|record| format!("{record}\n")).collect();
    std::fs::write(&journal, text).unwrap();

    let ops = dir.path().join("changed.json");
    std::fs::write(&ops, r#"[{"projectKey":"HAM","title":"Changed"}]"#).unwrap();

    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "work",
            "bulk",
            "run",
            "--op",
            "create",
            "--operations-file",
        ])
        .arg(&ops)
        .arg("--journal")
        .arg(&journal)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("differs from the journal plan"),
        "drift must be explained: {body}"
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// A new run requires an operations source; `--restart` replaces an existing
/// journal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_replaces_the_journal_and_fresh_runs_require_a_source() {
    let server = MockServer::start().await;
    mount_bulk(
        &server,
        "POST",
        "/api/v1/organizations/acme/bulk-work-items",
    )
    .await;

    let dir = TempDir::new().unwrap();
    let journal = dir.path().join("restart.jsonl");

    // No source and no journal: a clear usage error.
    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "work",
            "bulk",
            "run",
            "--op",
            "create",
            "--journal",
        ])
        .arg(&journal)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));

    let ops = dir.path().join("ops.json");
    std::fs::write(&ops, create_operations(3)).unwrap();
    let run = |extra: &[&str]| {
        let mut cmd = base(&server, &dir);
        cmd.args([
            "--json",
            "--org",
            "acme",
            "work",
            "bulk",
            "run",
            "--op",
            "create",
            "--operations-file",
        ])
        .arg(&ops)
        .arg("--journal")
        .arg(&journal);
        cmd.args(extra.iter().copied());
        cmd.output().unwrap()
    };

    let first = run(&[]);
    assert_eq!(first.status.code(), Some(0), "{:?}", first.stderr);
    let body: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(body["summary"]["batches"], 1);

    // Restart with a smaller source replaces the plan.
    std::fs::write(&ops, create_operations(1)).unwrap();
    let restarted = run(&["--restart"]);
    assert_eq!(restarted.status.code(), Some(0), "{:?}", restarted.stderr);
    let body: Value = serde_json::from_slice(&restarted.stdout).unwrap();
    assert_eq!(body["summary"]["operations"], 1);
    assert_eq!(body["resumed"], false);
}
