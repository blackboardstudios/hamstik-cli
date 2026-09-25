// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `work bulk from-csv` conversion and local preflight tests.

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

/// A command with no reachable server: any network activity would hang/fail,
/// so a clean conversion failure proves nothing was sent.
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

/// Converting a valid CSV and piping it into `work bulk create
/// --operations-file -` sends exactly the same request as a hand-written JSON
/// operations file with the same content.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_csv_create_pipes_into_bulk_identically_to_json() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/bulk-work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let csv = dir.path().join("items.csv");
    std::fs::write(&csv, "title\n\"First, item\"\nSecond item\n").unwrap();

    // 1. Convert locally to the operations array.
    let converted = local(&dir)
        .args([
            "--no-input",
            "work",
            "bulk",
            "from-csv",
            csv.to_str().unwrap(),
            "--op",
            "create",
            "--project",
            "HAM",
        ])
        .output()
        .unwrap();
    assert!(
        converted.status.success(),
        "conversion failed: {:?}",
        converted
    );
    let converted_stdout = converted.stdout.clone();
    let converted_json: Value = serde_json::from_slice(&converted_stdout).unwrap();
    assert_eq!(
        converted_json,
        json!([
            { "projectKey": "HAM", "title": "First, item" },
            { "projectKey": "HAM", "title": "Second item" },
        ])
    );

    // 2. Pipe the conversion into the existing bulk path.
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "work",
            "bulk",
            "create",
            "--operations-file",
            "-",
            "--json",
        ])
        .write_stdin(converted_stdout)
        .output()
        .unwrap();

    // 3. The same content as a hand-written JSON file.
    let json_file = dir.path().join("ops.json");
    std::fs::write(
        &json_file,
        r#"[{"projectKey":"HAM","title":"First, item"},{"projectKey":"HAM","title":"Second item"}]"#,
    )
    .unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "work",
            "bulk",
            "create",
            "--operations-file",
            json_file.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body, requests[1].body,
        "CSV conversion must produce the same request body as the JSON file"
    );
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        body,
        json!({
            "operations": [
                { "projectKey": "HAM", "title": "First, item" },
                { "projectKey": "HAM", "title": "Second item" },
            ]
        })
    );
}

/// Update CSV conversion maps documented columns into the same typed envelope
/// as the bulk update path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn from_csv_update_pipes_into_bulk_identically_to_json() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/organizations/acme/bulk-work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let csv = dir.path().join("items.csv");
    std::fs::write(
        &csv,
        "workItemKey,revision,title,type,priority,storyPoints\n\
         HAM-1,3,New title,BUG,HIGH,5\n",
    )
    .unwrap();

    let converted = local(&dir)
        .args([
            "--no-input",
            "work",
            "bulk",
            "from-csv",
            csv.to_str().unwrap(),
            "--op",
            "update",
            "--project",
            "HAM",
        ])
        .output()
        .unwrap();
    assert!(
        converted.status.success(),
        "conversion failed: {:?}",
        converted
    );
    let converted_stdout = converted.stdout.clone();

    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "work",
            "bulk",
            "update",
            "--operations-file",
            "-",
            "--json",
        ])
        .write_stdin(converted_stdout)
        .output()
        .unwrap();

    let json_file = dir.path().join("ops.json");
    std::fs::write(
        &json_file,
        r#"[{"projectKey":"HAM","workItemKey":"HAM-1","revision":3,"changes":{"title":"New title","type":"bug","priority":"high","storyPoints":5}}]"#,
    )
    .unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "work",
            "bulk",
            "update",
            "--operations-file",
            json_file.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body, requests[1].body,
        "CSV conversion must produce the same request body as the JSON file"
    );
}

/// Invalid rows are rejected locally, before any network call, with the CSV row
/// number and column name.
#[test]
fn from_csv_rejects_invalid_rows_with_row_and_column_before_network() {
    struct Case {
        name: &'static str,
        op: &'static str,
        content: &'static str,
        expect: &'static [&'static str],
    }
    let cases = [
        Case {
            name: "bad enum",
            op: "update",
            content: "workItemKey,priority\nHAM-1,urgnet\n",
            expect: &[
                "row 2",
                "\"priority\"",
                "urgnet",
                "low, medium, high, urgent",
            ],
        },
        Case {
            name: "unknown column",
            op: "create",
            content: "title,status\nOne,done\n",
            expect: &["unknown column", "status", "supported columns are: title"],
        },
        Case {
            name: "missing work item key",
            op: "update",
            content: "workItemKey,title\n,One\n",
            expect: &["row 2", "workItemKey", "must not be empty"],
        },
        Case {
            name: "no change columns",
            op: "update",
            content: "workItemKey,revision\nHAM-1,2\n",
            expect: &["row 2", "no change columns"],
        },
        Case {
            name: "wrong column count",
            op: "update",
            content: "workItemKey,revision\nHAM-1\n",
            expect: &["row 2", "expected 2 columns"],
        },
        Case {
            name: "non-positive revision",
            op: "update",
            content: "workItemKey,revision,title\nHAM-1,0,One\n",
            expect: &["row 2", "\"revision\"", "positive integer"],
        },
    ];
    for case in cases {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("items.csv");
        std::fs::write(&file, case.content).unwrap();
        let output = local(&dir)
            .args([
                "--no-input",
                "--json",
                "work",
                "bulk",
                "from-csv",
                file.to_str().unwrap(),
                "--op",
                case.op,
                "--project",
                "HAM",
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "`{}` must fail with usage exit code: {}",
            case.name,
            String::from_utf8_lossy(&output.stderr)
        );
        let body: Value = serde_json::from_slice(&output.stderr).unwrap();
        let message = body["error"]["message"].as_str().unwrap_or_default();
        for expected in case.expect {
            assert!(
                message.contains(expected),
                "`{}`: expected {expected:?} in: {message}",
                case.name
            );
        }
        assert!(
            !message.contains("secret"),
            "`{}` leaked unexpected content",
            case.name
        );
    }
}

/// A structural CSV error names the row but does not require a server.
#[test]
fn from_csv_rejects_malformed_csv_locally() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("items.csv");
    std::fs::write(&file, "title\n\"unterminated\n").unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "--json",
            "work",
            "bulk",
            "from-csv",
            file.to_str().unwrap(),
            "--op",
            "create",
            "--project",
            "HAM",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("unterminated"), "{message}");
}

/// The global `--project` supplies the key when the subcommand flag is absent.
#[test]
fn from_csv_accepts_the_global_project_flag() {
    let dir = TempDir::new().unwrap();
    let csv = dir.path().join("items.csv");
    std::fs::write(&csv, "title\nOne\n").unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "--project",
            "GLOB",
            "work",
            "bulk",
            "from-csv",
            csv.to_str().unwrap(),
            "--op",
            "create",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body, json!([{ "projectKey": "GLOB", "title": "One" }]));

    // With no source at all, the command fails locally.
    let output = local(&dir)
        .args([
            "--no-input",
            "work",
            "bulk",
            "from-csv",
            csv.to_str().unwrap(),
            "--op",
            "create",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no project selected"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// With no flag at all, normal project selection (`HAMSTIK_PROJECT`) supplies
/// the key — the same local resolution every other command uses.
#[test]
fn from_csv_falls_back_to_normal_project_selection() {
    let dir = TempDir::new().unwrap();
    let csv = dir.path().join("items.csv");
    std::fs::write(&csv, "title\nOne\n").unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "work",
            "bulk",
            "from-csv",
            csv.to_str().unwrap(),
            "--op",
            "create",
        ])
        .env("HAMSTIK_PROJECT", "ENVP")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body, json!([{ "projectKey": "ENVP", "title": "One" }]));
}

/// `--output` writes the operations JSON to a file, and the file is accepted by
/// the existing bulk path.
#[test]
fn from_csv_output_writes_a_bulk_operations_file() {
    let dir = TempDir::new().unwrap();
    let csv = dir.path().join("items.csv");
    std::fs::write(&csv, "title\nOne\n").unwrap();
    let out = dir.path().join("ops.json");
    local(&dir)
        .args([
            "--no-input",
            "work",
            "bulk",
            "from-csv",
            csv.to_str().unwrap(),
            "--op",
            "create",
            "--project",
            "HAM",
            "--output",
            out.to_str().unwrap(),
        ])
        .assert()
        .success();
    let body: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(body, json!([{ "projectKey": "HAM", "title": "One" }]));
}

/// Existing JSON-file workflows are unaffected: a normal operations file still
/// validates and submits without any CSV involvement.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_json_bulk_workflow_is_unaffected() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/bulk-work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "results": [] })))
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let ops = dir.path().join("ops.json");
    std::fs::write(&ops, r#"[{"projectKey":"HAM","title":"One"}]"#).unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "work",
            "bulk",
            "create",
            "--operations-file",
            ops.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}
