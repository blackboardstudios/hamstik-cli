// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! CLI-8: SqueakQL query files, stdin, saved queries, and HTTP-boundary
//! proofs.

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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
    cmd.arg("--no-input");
    cmd
}

fn valid_response() -> Value {
    json!({"valid": true, "languageVersion": 1, "errors": []})
}

/// Captures one POST body into `path` and answers `valid`.
async fn mounting_capture(server: &MockServer, path: &'static str) {
    Mock::given(method("POST"))
        .and(wiremock::matchers::path(path))
        .respond_with(|request: &wiremock::Request| {
            let body = String::from_utf8_lossy(&request.body).to_string();
            std::fs::write("/tmp/kilo-sq-capture.json", body).ok();
            ResponseTemplate::new(200).set_body_json(valid_response())
        })
        .mount(server)
        .await;
}

/// Inline, file, and stdin produce byte-equivalent query values: one
/// trailing newline is stripped, everything else passes through verbatim.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_sources_are_byte_equivalent_after_newline_policy() {
    std::fs::create_dir_all("/tmp").unwrap();
    for name in ["file-with-newline", "file-without-newline", "stdin"] {
        let server = MockServer::start().await;
        mounting_capture(&server, "/api/v1/organizations/acme/squeakql/validate").await;
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("q.sqql");
        match name {
            "file-with-newline" => std::fs::write(&file, "status = todo\n").unwrap(),
            "file-without-newline" => std::fs::write(&file, "status = todo").unwrap(),
            _ => {}
        }
        let mut cmd = base(&server, &dir);
        cmd.args(["--org", "acme", "squeakql", "validate", "--file"]);
        if name == "stdin" {
            cmd.arg("-").write_stdin("status = todo");
        } else {
            cmd.arg(&file);
        }
        let output = cmd.output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{name}: {:?}", output.stderr);
        let sent: Value =
            serde_json::from_slice(&std::fs::read("/tmp/kilo-sq-capture.json").unwrap()).unwrap();
        assert_eq!(
            sent["query"], "status = todo",
            "{name}: newline policy drifted"
        );
    }
}

/// Supplying more than one query source fails at parse time (exit 2) before
/// any request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn conflicting_sources_fail_before_any_request() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("q.sqql");
    std::fs::write(&file, "status = todo").unwrap();

    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "squeakql",
            "validate",
            "status = todo",
            "--file",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "a conflicting-source usage error must not reach the wire"
    );

    // --saved conflicts with both inline and file.
    let output = base(&server, &dir)
        .args([
            "--org", "acme", "squeakql", "validate", "--saved", "x", "--file",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// File-not-found, empty expression, and invalid UTF-8 produce usage-class
/// errors naming the cause; no request is made for local failures.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_input_failures_are_actionable() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();

    // Missing file.
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--json",
            "squeakql",
            "validate",
            "--file",
            "/nonexistent/query.sqql",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("cannot read query file"),
        "{body}"
    );

    // Empty expression (whitespace only).
    let empty = dir.path().join("empty.sqql");
    std::fs::write(&empty, "   \n  \n").unwrap();
    let output = base(&server, &dir)
        .args(["--org", "acme", "--json", "squeakql", "validate", "--file"])
        .arg(&empty)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("must not be empty"),
        "{body}"
    );

    // Invalid UTF-8.
    let binary = dir.path().join("binary.sqql");
    std::fs::write(&binary, [0xFF, 0xFE, b'a']).unwrap();
    let output = base(&server, &dir)
        .args(["--org", "acme", "--json", "squeakql", "validate", "--file"])
        .arg(&binary)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// Saved queries: save/show/list/delete round-trip, --force replacement,
/// invalid names rejected, and the storage file contains only expressions.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_queries_round_trip_with_validation() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();

    // Save from stdin.
    let mut cmd = base(&server, &dir);
    cmd.args([
        "--org",
        "acme",
        "squeakql",
        "save",
        "my-urgent",
        "--file",
        "-",
    ])
    .write_stdin("priority >= urgent AND status != done");
    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);

    // Duplicate save requires --force.
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--json",
            "squeakql",
            "save",
            "my-urgent",
            "status = todo",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("--force"),
        "{body}"
    );

    // Show returns the exact expression.
    let output = base(&server, &dir)
        .args(["--org", "acme", "--json", "squeakql", "show", "my-urgent"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["query"], "priority >= urgent AND status != done");

    // The storage file holds only the expression: no tokens or headers.
    let queries_text = std::fs::read_to_string(dir.path().join("queries.toml")).unwrap();
    assert!(
        !queries_text.contains("secret-token"),
        "the saved-queries file must never contain credential material"
    );
    assert!(queries_text.contains("priority >= urgent"));
}

/// Saved queries are plain local expressions: using `--saved` sends exactly
/// the stored text to the wire with the documented fields and no
/// Idempotency-Key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saved_query_used_on_the_wire_has_documented_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/squeakql/validate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(valid_response()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/work-items/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "page": {"limit": 50, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "squeakql",
            "save",
            "todos",
            "status = todo",
        ])
        .output()
        .unwrap();

    // Validate via --saved.
    let output = base(&server, &dir)
        .args([
            "--org", "acme", "--json", "squeakql", "validate", "--saved", "todos",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);

    // Search via --saved: the request is a JSON POST with only
    // query/limit/cursor and no Idempotency-Key.
    let output = base(&server, &dir)
        .args([
            "--org", "acme", "--json", "work", "search", "--saved", "todos",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let request = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.url.path().ends_with("/search"))
        .unwrap();
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["query"], "status = todo");
    assert!(
        sent.as_object()
            .unwrap()
            .keys()
            .all(|key| key != "idempotencyKey"),
        "SqueakQL search is read-only: no Idempotency-Key"
    );
    assert!(
        !request.headers.contains_key("idempotency-key"),
        "no idempotency header on read-only search"
    );
    assert!(request.headers.contains_key("authorization"));
}

/// Validation JSON preserves the full Public API response, including error
/// spans; human output names location and message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validation_errors_preserve_spans_and_details() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/squeakql/validate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "valid": false,
            "languageVersion": 1,
            "errors": [
                {"code": "SQUEAKQL_SYNTAX_ERROR", "message": "unexpected token",
                 "line": 2, "column": 7, "suggestion": "use `and`"}
            ]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org", "acme", "--json", "squeakql", "validate", "status =",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let error = &body["errors"][0];
    assert_eq!(error["code"], "SQUEAKQL_SYNTAX_ERROR");
    assert_eq!(error["line"], 2);
    assert_eq!(error["column"], 7);
    assert_eq!(error["message"], "unexpected token");
    assert_eq!(error["suggestion"], "use `and`");

    // Human output names the location.
    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["--org", "acme", "squeakql", "validate", "bogus"])
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("2:7"), "{text}");
    assert!(text.contains("unexpected token"), "{text}");
    assert!(text.contains("use `and`"), "{text}");
}
