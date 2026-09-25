// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Bulk preflight and per-operation diagnostics tests.

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn page_for_bulk(results: Value) -> Value {
    json!({ "results": results })
}

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
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

/// A command with no reachable server: preflight failures must occur before
/// any network activity.
fn local(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env("HAMSTIK_HOST", "http://127.0.0.1:1");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

/// Malformed JSON, wrong envelope shape, unknown fields, missing required
/// fields, invalid enum values, and count violations all fail before any
/// HTTP request, with the failing index and field path identified.
#[test]
fn preflight_rejects_malformed_input_before_any_request() {
    struct Case {
        name: &'static str,
        content: &'static str,
        expect_in_message: &'static [&'static str],
    }
    let cases = [
        Case {
            name: "not json",
            content: "not json at all",
            expect_in_message: &["not valid JSON"],
        },
        Case {
            name: "object not array",
            content: r#"{"projectKey":"HAM","title":"One"}"#,
            expect_in_message: &["JSON array of operations"],
        },
        Case {
            name: "unknown field",
            content: r#"[{"projectKey":"HAM","title":"One","bogus":1}]"#,
            expect_in_message: &["operations[0].bogus", "unknown fields are rejected"],
        },
        Case {
            name: "missing required",
            content: r#"[{"title":"One"}]"#,
            expect_in_message: &["operations[0].projectKey", "is required"],
        },
        Case {
            name: "empty required",
            content: r#"[{"projectKey":"","title":"One"}]"#,
            expect_in_message: &["operations[0].projectKey", "must not be empty"],
        },
        Case {
            name: "too many operations",
            content: r#"[{"projectKey":"HAM","title":"One"},{"projectKey":"HAM","title":"Two"},{"projectKey":"HAM","title":"Three"},{"projectKey":"HAM","title":"Four"},{"projectKey":"HAM","title":"Five"},{"projectKey":"HAM","title":"Six"},{"projectKey":"HAM","title":"Seven"},{"projectKey":"HAM","title":"Eight"},{"projectKey":"HAM","title":"Nine"},{"projectKey":"HAM","title":"Ten"},{"projectKey":"HAM","title":"Eleven"},{"projectKey":"HAM","title":"Twelve"},{"projectKey":"HAM","title":"Thirteen"},{"projectKey":"HAM","title":"Fourteen"},{"projectKey":"HAM","title":"Fifteen"},{"projectKey":"HAM","title":"Sixteen"},{"projectKey":"HAM","title":"Seventeen"},{"projectKey":"HAM","title":"Eighteen"},{"projectKey":"HAM","title":"Nineteen"},{"projectKey":"HAM","title":"Twenty"},{"projectKey":"HAM","title":"TwentyOne"},{"projectKey":"HAM","title":"TwentyTwo"},{"projectKey":"HAM","title":"TwentyThree"},{"projectKey":"HAM","title":"TwentyFour"},{"projectKey":"HAM","title":"TwentyFive"},{"projectKey":"HAM","title":"TwentySix"},{"projectKey":"HAM","title":"TwentySeven"},{"projectKey":"HAM","title":"TwentyEight"},{"projectKey":"HAM","title":"TwentyNine"},{"projectKey":"HAM","title":"Thirty"},{"projectKey":"HAM","title":"ThirtyOne"},{"projectKey":"HAM","title":"ThirtyTwo"},{"projectKey":"HAM","title":"ThirtyThree"},{"projectKey":"HAM","title":"ThirtyFour"},{"projectKey":"HAM","title":"ThirtyFive"},{"projectKey":"HAM","title":"ThirtySix"},{"projectKey":"HAM","title":"ThirtySeven"},{"projectKey":"HAM","title":"ThirtyEight"},{"projectKey":"HAM","title":"ThirtyNine"},{"projectKey":"HAM","title":"Forty"},{"projectKey":"HAM","title":"FortyOne"},{"projectKey":"HAM","title":"FortyTwo"},{"projectKey":"HAM","title":"FortyThree"},{"projectKey":"HAM","title":"FortyFour"},{"projectKey":"HAM","title":"FortyFive"},{"projectKey":"HAM","title":"FortySix"},{"projectKey":"HAM","title":"FortySeven"},{"projectKey":"HAM","title":"FortyEight"},{"projectKey":"HAM","title":"FortyNine"},{"projectKey":"HAM","title":"Fifty"},{"projectKey":"HAM","title":"FiftyOne"}]"#,
            expect_in_message: &["51 operations", "at most 50"],
        },
        Case {
            name: "invalid transition status",
            content: r#"[{"projectKey":"HAM","workItemKey":"HAM-1","targetStatus":"archived"}]"#,
            expect_in_message: &[
                "operations[0].targetStatus",
                "not a Work Item status",
                "backlog, todo, in_progress, in_review, done",
            ],
        },
        Case {
            name: "malformed Attribute change",
            content: r#"[{"projectKey":"HAM","title":"One","attributes":[{"key":"verified"}]}]"#,
            expect_in_message: &[
                "operations[0].attributes[0]",
                "exactly one of clear, booleanValue, or optionKeys",
            ],
        },
        Case {
            name: "unknown Attribute assignment field",
            content: r#"[{"projectKey":"HAM","title":"One","attributes":[{"key":"verified","booleanValue":false,"schema":"secret"}]}]"#,
            expect_in_message: &[
                "operations[0].attributes[0].schema",
                "not part of the Attribute assignment schema",
            ],
        },
    ];
    let update_cases = [
        Case {
            name: "non-positive revision",
            content: r#"[{"projectKey":"HAM","workItemKey":"HAM-1","revision":0,"changes":{"title":"X"}}]"#,
            expect_in_message: &["operations[0].revision", "positive integer"],
        },
        Case {
            name: "empty changes",
            content: r#"[{"projectKey":"HAM","workItemKey":"HAM-1","revision":1,"changes":{}}]"#,
            expect_in_message: &["operations[0].changes", "must not be empty"],
        },
        Case {
            name: "false cannot mean clear",
            content: r#"[{"projectKey":"HAM","workItemKey":"HAM-1","revision":1,"changes":{"attributes":[{"key":"customer","clear":false}]}}]"#,
            expect_in_message: &["operations[0].changes.attributes[0].clear", "must be true"],
        },
    ];
    for (kind, batch) in [("create", &cases[..]), ("update", &update_cases[..])] {
        for case in batch {
            let dir = TempDir::new().unwrap();
            let file = dir.path().join("ops.json");
            std::fs::write(&file, case.content).unwrap();
            let output = local(&dir)
                .args([
                    "--no-input",
                    "--json",
                    "--org",
                    "acme",
                    "work",
                    "bulk",
                    kind,
                    "--operations-file",
                ])
                .arg(&file)
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(2),
                "`{}` should fail preflight with usage exit code: {:?}",
                case.name,
                output.stderr
            );
            let body: Value = serde_json::from_slice(&output.stderr).unwrap();
            let message = body["error"]["message"].as_str().unwrap_or_default();
            for expected in case.expect_in_message {
                assert!(
                    message.contains(expected),
                    "`{}`: expected {expected:?} in: {message}",
                    case.name
                );
            }
            // No unrelated payload content is echoed.
            assert!(
                !message.contains("secret"),
                "`{}` leaked unexpected content",
                case.name
            );
        }
    }
}

/// Update and transition kinds validate their own required fields.
#[test]
fn preflight_enforces_per_kind_required_fields() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("ops.json");
    std::fs::write(&file, r#"[{"projectKey":"HAM","title":"One"}]"#).unwrap();

    for subcommand in ["update", "transition"] {
        let output = local(&dir)
            .args([
                "--no-input",
                "--json",
                "--org",
                "acme",
                "work",
                "bulk",
                subcommand,
                "--operations-file",
            ])
            .arg(&file)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let body: Value = serde_json::from_slice(&output.stderr).unwrap();
        let message = body["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("workItemKey"),
            "`{subcommand}` must require workItemKey: {message}"
        );
    }

    // The same file passes preflight for create (dry-run makes no requests).
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
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
}

/// A valid file passes preflight and reaches the wire with the documented
/// envelope, concurrency mode, and idempotency header; a replayed response
/// surfaces the replay note.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_bulk_create_sends_documented_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/bulk-work-items"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-Idempotent-Replay", "true")
                .set_body_json(page_for_bulk(json!([
                    {"index": 0, "status": 201, "workItem": {"id": "1", "key": "HAM-1", "projectId": "2", "title": "One", "description": null, "type": "task", "status": "todo", "priority": "low", "assignee": null, "reporter": null, "sprint": null, "parent": null, "labels": [], "storyPoints": null, "dueDate": null, "archivedAt": null, "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": 1}},
                    {"index": 1, "status": 412, "error": {"code": "REVISION_CONFLICT", "message": "revision changed", "requestId": "bulk-req-1"}}
                ]))),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let file = dir.path().join("ops.json");
    std::fs::write(
        &file,
        r#"[{"projectKey":"HAM","title":"One"},{"projectKey":"HAM","title":"Two"}]"#,
    )
    .unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "work",
            "bulk",
            "create",
            "--idempotency-key",
            "bulk-preflight-key-1",
            "--operations-file",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);

    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request
            .headers
            .get("idempotency-key")
            .unwrap()
            .to_str()
            .unwrap(),
        "bulk-preflight-key-1"
    );
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        sent["operations"].as_array().unwrap().len(),
        2,
        "both operations must be sent: {sent}"
    );
    assert!(
        sent.get("concurrency").is_none(),
        "create has no concurrency field"
    );

    // JSON preserves every documented per-operation field exactly.
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(results[0]["index"], 0);
    assert_eq!(results[0]["status"], 201);
    assert_eq!(results[0]["workItem"]["key"], "HAM-1");
    assert_eq!(results[1]["error"]["code"], "REVISION_CONFLICT");
    assert_eq!(results[1]["error"]["requestId"], "bulk-req-1");
}

/// Human output summarizes total/succeeded/failed and prints actionable
/// failure details; the concurrency mode is always stated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_bulk_output_summarizes_and_diagnoses_failures() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/organizations/acme/bulk-work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page_for_bulk(json!([
            {"index": 0, "status": 200, "workItem": {"id": "1", "key": "HAM-1", "projectId": "2", "title": "One", "description": null, "type": "task", "status": "todo", "priority": "low", "assignee": null, "reporter": null, "sprint": null, "parent": null, "labels": [], "storyPoints": null, "dueDate": null, "archivedAt": null, "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": 2}},
            {"index": 1, "status": 412, "error": {"code": "REVISION_CONFLICT", "message": "revision changed", "requestId": "bulk-req-9"}},
            {"index": 2, "status": 400, "error": {"code": "VALIDATION_ERROR", "message": "title too long", "requestId": "bulk-req-9"}}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let file = dir.path().join("ops.json");
    std::fs::write(
        &file,
        r#"[{"projectKey":"HAM","workItemKey":"HAM-1","revision":2,"changes":{"title":"One"}},{"projectKey":"HAM","workItemKey":"HAM-2","revision":1,"changes":{"title":"Two"}},{"projectKey":"HAM","workItemKey":"HAM-3","revision":1,"changes":{"title":"Three"}}]"#,
    )
    .unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "work",
            "bulk",
            "update",
            "--operations-file",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("bulk results: 1/3 succeeded, 2 failed"),
        "summary line drifted: {text}"
    );
    assert!(
        text.contains("concurrency: require-revision"),
        "the selected concurrency mode must be stated: {text}"
    );
    assert!(
        text.contains("operations[1] (HTTP 412) REVISION_CONFLICT"),
        "failure line must identify index and code: {text}"
    );
    assert!(
        text.contains("re-read the Work Item") || text.contains("last-write-wins"),
        "revision conflicts need an actionable fix: {text}"
    );
    assert!(
        text.contains("operations[2] (HTTP 400) VALIDATION_ERROR"),
        "failures need index and code: {text}"
    );
    assert!(
        text.contains("request id: bulk-req-9"),
        "server request ids must be surfaced: {text}"
    );
}

/// File and stdin inputs behave consistently under `--no-input`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_operations_file_behaves_like_file_input() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/bulk-work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page_for_bulk(json!([
            {"index": 0, "status": 201, "workItem": {"id": "1", "key": "HAM-1", "projectId": "2", "title": "One", "description": null, "type": "task", "status": "todo", "priority": "low", "assignee": null, "reporter": null, "sprint": null, "parent": null, "labels": [], "storyPoints": null, "dueDate": null, "archivedAt": null, "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": 1}}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let mut cmd = base(&server, &dir);
    cmd.args([
        "--no-input",
        "--json",
        "--org",
        "acme",
        "work",
        "bulk",
        "create",
        "--operations-file",
        "-",
    ]);
    cmd.write_stdin(r#"[{"projectKey":"HAM","title":"One"}]"#);
    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["results"][0]["workItem"]["key"], "HAM-1");

    // Invalid stdin JSON fails preflight with the same diagnostics wording.
    let dir = TempDir::new().unwrap();
    let mut cmd = base(&server, &dir);
    cmd.args([
        "--no-input",
        "--json",
        "--org",
        "acme",
        "work",
        "bulk",
        "create",
        "--operations-file",
        "-",
    ]);
    cmd.write_stdin("[{]");
    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("stdin is not valid JSON"),
        "stdin diagnostics should name the source: {:?}",
        body["error"]["message"]
    );
}
