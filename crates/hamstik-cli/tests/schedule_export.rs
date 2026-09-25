// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work export --query` and `schedule` contract tests (CLI-67).
//!
//! These pin the two acceptance criteria: a `--query` collection export is
//! rendered through the shared output modes (so `--format csv` works), and an
//! externally invoked schedule produces exactly the bytes of the equivalent
//! manual invocation. Schedule definitions are plain files next to the config.

#![allow(clippy::unwrap_used, clippy::expect_used)]

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
    cmd.env("HAMSTIK_TOKEN", "secret-token-value");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn summary_item(key: &str, title: &str) -> Value {
    json!({
        "id": format!("id-{key}"),
        "key": key,
        "revision": 1,
        "title": title,
        "type": "task",
        "status": "todo",
        "priority": "high",
        "assignee": null,
        "project": {"id": "p1", "key": "HAM", "name": "Ham", "color": "#123456"},
        "organization": {"id": "o1", "slug": "acme", "name": "Acme"},
    })
}

async fn mount_search(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/work-items/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [
                summary_item("HAM-1", "First task"),
                summary_item("HAM-2", "Second, task"),
            ],
            "page": {"limit": 50, "hasMore": false, "nextCursor": null}
        })))
        .mount(server)
        .await;
}

/// A `--query` export renders the shared list columns as CSV, and `--output`
/// writes exactly the bytes a stdout run would print.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_export_csv_matches_stdout_bytes() {
    let server = MockServer::start().await;
    mount_search(&server).await;
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("snapshot.csv");

    let stdout = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--format",
            "csv",
            "work",
            "export",
            "--query",
            "status = todo",
        ])
        .output()
        .unwrap();
    assert_eq!(stdout.status.code(), Some(0), "{:?}", stdout.stderr);
    let stdout_text = String::from_utf8(stdout.stdout.clone()).unwrap();
    assert!(stdout_text.starts_with("KEY,PROJECT,TITLE,STATUS,TYPE,PRIORITY,ASSIGNEE\n"));
    assert!(
        stdout_text.contains("HAM-1,HAM,First task,todo,task,high,-"),
        "{stdout_text}"
    );
    // RFC 4180 quoting for a title containing a comma.
    assert!(
        stdout_text.contains("\"Second, task\""),
        "comma must be quoted: {stdout_text}"
    );

    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--format",
            "csv",
            "work",
            "export",
            "--query",
            "status = todo",
            "--output",
            file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let written = std::fs::read(&file).unwrap();
    assert_eq!(
        written, stdout.stdout,
        "file export must equal the stdout bytes"
    );
}

/// A scheduled run re-executes the saved command through the same binary, so
/// the snapshot file it produces is byte-identical to a manual run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduled_run_matches_manual_invocation() {
    let server = MockServer::start().await;
    mount_search(&server).await;
    let dir = TempDir::new().unwrap();
    let scheduled_file = dir.path().join("scheduled.csv");

    let manual = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--format",
            "csv",
            "work",
            "export",
            "--query",
            "status = todo",
        ])
        .output()
        .unwrap();
    assert_eq!(manual.status.code(), Some(0), "{:?}", manual.stderr);
    assert!(!manual.stdout.is_empty());

    let save = base(&server, &dir)
        .args([
            "schedule",
            "save",
            "nightly",
            "--",
            "work",
            "export",
            "--org",
            "acme",
            "--format",
            "csv",
            "--query",
            "status = todo",
            "--output",
            scheduled_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(save.status.code(), Some(0), "{:?}", save.stderr);

    let definition = std::fs::read_to_string(dir.path().join("schedules/nightly.toml")).unwrap();
    assert!(definition.contains("command = ["));
    assert!(
        !definition.contains("secret-token"),
        "schedule definitions must never contain credentials"
    );

    let run = base(&server, &dir)
        .args(["schedule", "run", "nightly"])
        .output()
        .unwrap();
    assert_eq!(run.status.code(), Some(0), "{:?}", run.stderr);
    let written = std::fs::read(&scheduled_file).unwrap();
    assert_eq!(
        written, manual.stdout,
        "a scheduled run must match the manual invocation byte for byte"
    );
}

/// `schedule list/save/delete` manage plain files; `run` of a missing
/// definition fails as a usage error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schedule_lifecycle() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();

    let save = base(&server, &dir)
        .args([
            "schedule", "save", "weekly", "--", "work", "export", "--format", "csv",
        ])
        .output()
        .unwrap();
    assert_eq!(save.status.code(), Some(0), "{:?}", save.stderr);
    assert!(dir.path().join("schedules/weekly.toml").is_file());

    // Replacing without --force is rejected.
    let again = base(&server, &dir)
        .args([
            "schedule", "save", "weekly", "--", "work", "export", "--format", "jsonl",
        ])
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("--force"),
        "{:?}",
        again.stderr
    );

    let list = base(&server, &dir)
        .args(["--json", "schedule", "list"])
        .output()
        .unwrap();
    assert_eq!(list.status.code(), Some(0), "{:?}", list.stderr);
    let body: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(body["scheduleVersion"], 1);
    assert_eq!(body["schedules"][0]["name"], "weekly");
    assert_eq!(body["schedules"][0]["command"][0], "work");

    let delete = base(&server, &dir)
        .args(["schedule", "delete", "weekly"])
        .output()
        .unwrap();
    assert_eq!(delete.status.code(), Some(0), "{:?}", delete.stderr);
    assert!(!dir.path().join("schedules/weekly.toml").exists());

    let missing = base(&server, &dir)
        .args(["schedule", "run", "weekly"])
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(2));
}
