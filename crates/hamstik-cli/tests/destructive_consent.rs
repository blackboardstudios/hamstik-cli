// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Integration tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Destructive-command consent gating: `work delete`, `project archive`, and
//! completing a Sprint require explicit per-process consent
//! (`--confirm-destructive`, or the `--yes` scripting override) even outside
//! `--no-input`. Interactive sessions may confirm at a prompt; `--dry-run`
//! sends no mutation and needs no consent.

use assert_cmd::Command;
use predicates::prelude::*;
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
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn project_json(key: &str, revision: i64) -> Value {
    json!({
        "id": "p1", "organizationId": "o1", "key": key, "name": "P",
        "description": null, "color": "#000000", "revision": revision,
        "archivedAt": "2026-04-01T00:00:00Z",
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z"
    })
}

fn sprint_json() -> Value {
    json!({
        "id": "11111111-1111-1111-1111-111111111111", "name": "S", "state": "done",
        "startDate": null, "endDate": null, "goal": null, "targetPoints": null,
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": 2
    })
}

/// Every gated command fails with a usage error naming the flag, and never
/// contacts the server, whether or not `--no-input` is present.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gated_commands_without_consent_fail_usage_without_network() {
    let server = MockServer::start().await;
    let cases: Vec<Vec<&str>> = vec![
        vec![
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "delete",
            "HAM-1",
        ],
        vec!["--org", "acme", "project", "archive", "WEB"],
        vec![
            "--org",
            "acme",
            "--project",
            "HAM",
            "sprint",
            "transition",
            "11111111-1111-1111-1111-111111111111",
            "done",
        ],
    ];
    for case in cases {
        for extra in [Vec::new(), vec!["--no-input"]] {
            let dir = TempDir::new().unwrap();
            let mut args = extra;
            args.extend(case.iter().copied());
            base(&server, &dir)
                .args(&args)
                .assert()
                .code(2)
                .stderr(predicate::str::contains("--confirm-destructive"));
        }
    }
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "consent must fail before any request is sent"
    );
}

/// `--yes` is the scripting override and satisfies the consent gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn yes_override_satisfies_consent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/WEB/archive"))
        .respond_with(ResponseTemplate::new(200).set_body_json(project_json("WEB", 2)))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "project",
            "archive",
            "WEB",
            "--force",
            "--yes",
            "--json",
        ])
        .assert()
        .success();

    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// The explicit consent flag satisfies the gate for every gated command.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn confirm_destructive_flag_satisfies_consent() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/sprints/11111111-1111-1111-1111-111111111111/transitions",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(sprint_json()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "delete",
            "HAM-1",
            "--force",
            "--confirm-destructive",
            "--json",
        ])
        .assert()
        .success();

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "sprint",
            "transition",
            "11111111-1111-1111-1111-111111111111",
            "done",
            "--move-to-backlog",
            "--force",
            "--confirm-destructive",
            "--json",
        ])
        .assert()
        .success();
}

/// Sprint transitions that do not complete the Sprint are not gated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_destructive_sprint_transition_needs_no_consent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/sprints/11111111-1111-1111-1111-111111111111/transitions",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "11111111-1111-1111-1111-111111111111", "name": "S", "state": "active",
            "startDate": null, "endDate": null, "goal": null, "targetPoints": null,
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": 2
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "sprint",
            "transition",
            "11111111-1111-1111-1111-111111111111",
            "active",
            "--force",
            "--json",
        ])
        .assert()
        .success();
}

/// `work archive` is not in the destructive set and is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_archive_is_not_gated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/archive",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "1", "key": "HAM-1", "projectId": "2", "title": "T", "description": null,
            "type": "task", "status": "todo", "priority": "low", "assignee": null,
            "reporter": null, "sprint": null, "parent": null, "labels": [],
            "storyPoints": null, "dueDate": null, "archivedAt": null,
            "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": 2
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "archive",
            "HAM-1",
            "--force",
            "--json",
        ])
        .assert()
        .success();
}

/// `--dry-run` previews a gated command without consent and without a server.
#[test]
fn dry_run_gated_command_needs_no_consent() {
    let dir = TempDir::new().unwrap();
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
    cmd.args([
        "--no-retry",
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
    ]);
    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["operation"], "work.delete");
}
