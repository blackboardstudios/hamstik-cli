// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Integration tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `hamstik project report` / `hamstik sprint report` end-to-end tests:
//! verbatim `--json` passthrough fidelity, human rendering, option
//! forwarding, and server-authoritative error surfacing.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PROJECT_REPORT: &str = "/api/v1/organizations/acme/projects/HAM/reports/velocity";
const SPRINT_REPORT: &str = "/api/v1/organizations/acme/projects/HAM/sprints/sprint-1/report";

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
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

fn velocity_report() -> Value {
    json!({
        "kind": "velocity",
        "items": [],
        "data": [
            {"label": "Sprint 1", "committed": 10, "completed": 8, "carried": 2},
            {"label": "Sprint 2", "committed": 12, "completed": 12, "carried": 0}
        ],
        "page": {"limit": 50, "hasMore": false, "nextCursor": null},
        "limitations": ["Unestimated items count as zero points."]
    })
}

fn sprint_report() -> Value {
    json!({
        "sprint": {
            "id": "sprint-1",
            "name": "Sprint 12",
            "state": "active",
            "startDate": "2026-09-01T00:00:00.000Z",
            "endDate": "2026-09-14T00:00:00.000Z",
            "goal": "Ship reports",
            "targetPoints": 34,
            "updatedAt": "2026-09-08T10:00:00.000Z",
            "reportEndAt": null
        },
        "mode": "active",
        "stable": true,
        "commitment": {"available": true, "itemCount": 12, "points": 34},
        "plannedScope": {"itemCount": 12, "points": 34},
        "completion": {
            "originalCommitment": {"itemCount": 12, "points": 34},
            "finalScope": {"itemCount": 13, "points": 40},
            "completed": {"itemCount": 6, "points": 30},
            "estimatedItemCount": 11,
            "itemIds": {"originalCommitment": [], "finalScope": [], "completed": []},
            "percentages": {
                "finalScope": {"itemPercent": 46, "pointPercent": 75},
                "originalCommitment": {"itemPercent": 50, "pointPercent": 88}
            }
        },
        "scopeChanges": {"added": [], "removed": []},
        "carryover": [{
            "id": "row-2", "workItemId": "wi-22", "key": "HAM-22", "title": "Carried",
            "status": "todo", "storyPoints": null, "occurredAt": "2026-09-14T09:00:00.000Z",
            "destination": {"kind": "backlog", "sprintId": null, "name": "Backlog"}
        }],
        "statuses": [{"status": "done", "itemCount": 3, "points": 30}],
        "burndown": {
            "available": true,
            "points": [{"date": "2026-09-01", "remainingPoints": 34}],
            "display": [
                {"date": "2026-09-01", "label": "Sep 1", "remaining": 34, "ideal": 34},
                {"date": "2026-09-02", "label": "Sep 2", "remaining": 10, "ideal": 30}
            ]
        },
        "remaining": {"itemCount": 7, "points": 10},
        "limitations": [],
        "items": [{
            "id": "row-2", "workItemId": "wi-22", "key": "HAM-22", "title": "Carried",
            "status": "todo", "storyPoints": null, "occurredAt": "2026-09-14T09:00:00.000Z",
            "kind": "carryover",
            "destination": {"kind": "backlog", "sprintId": null, "name": "Backlog"}
        }],
        "page": {"limit": 50, "hasMore": true, "nextCursor": "next-feed-cursor"}
    })
}

/// The sorted query pairs of the single request the server received.
async fn received_query(server: &MockServer) -> Vec<String> {
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "exactly one request");
    let mut pairs: Vec<String> = requests[0]
        .url
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(str::to_string)
        .collect();
    pairs.sort();
    pairs
}

/// `--json` echoes the server report body verbatim, with no CLI-added fields.
#[tokio::test]
async fn project_report_json_is_the_server_payload_verbatim() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PROJECT_REPORT))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-Request-Id", "req-velocity")
                .set_body_json(velocity_report()),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["--json", "--org", "acme", "--project", "HAM"])
        .args(["project", "report", "velocity"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body, velocity_report());
}

/// Acceptance criterion: the typed `--json` payload equals what the raw
/// passthrough returns for the same endpoint (modulo the documented
/// `api request` envelope, which nests the body under `data`).
#[tokio::test]
async fn project_report_json_matches_the_passthrough_payload() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PROJECT_REPORT))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-Request-Id", "req-velocity-2")
                .set_body_json(velocity_report()),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let typed = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "project",
            "report",
            "velocity",
        ])
        .output()
        .unwrap();
    assert!(typed.status.success());
    let typed_body: Value = serde_json::from_slice(&typed.stdout).unwrap();

    let passthrough = base(&server, &dir)
        .args(["--json", "--org", "acme", "api", "request", PROJECT_REPORT])
        .output()
        .unwrap();
    assert!(passthrough.status.success());
    let passthrough_body: Value = serde_json::from_slice(&passthrough.stdout).unwrap();

    assert_eq!(typed_body, passthrough_body["data"]);
}

/// The human view renders the report rows as a table and repeats the server's
/// limitations verbatim; it adds no computed metric.
#[tokio::test]
async fn project_report_human_output_renders_rows_and_limitations() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PROJECT_REPORT))
        .respond_with(ResponseTemplate::new(200).set_body_json(velocity_report()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM"])
        .args(["project", "report", "velocity"])
        .assert()
        .success()
        .stdout(predicate::str::contains("report"))
        .stdout(predicate::str::contains("velocity"))
        .stdout(predicate::str::contains("LABEL"))
        .stdout(predicate::str::contains("COMMITTED"))
        .stdout(predicate::str::contains("Sprint 1"))
        .stdout(predicate::str::contains(
            "Unestimated items count as zero points.",
        ));
}

/// Report shaping and page options become query parameters and nothing else;
/// options that were not given are not sent.
#[tokio::test]
async fn project_report_forwards_shaping_and_page_options() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(PROJECT_REPORT))
        .respond_with(ResponseTemplate::new(200).set_body_json(velocity_report()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "project",
            "report",
            "velocity",
        ])
        .args(["--range", "6", "--unit", "points", "--group-by", "assignee"])
        .args(["--limit", "25", "--since-cursor", "cursor-abc"])
        .assert()
        .success();

    assert_eq!(
        received_query(&server).await,
        vec![
            "cursor=cursor-abc".to_string(),
            "groupBy=assignee".to_string(),
            "limit=25".to_string(),
            "range=6".to_string(),
            "unit=points".to_string(),
        ]
    );
}

/// Acceptance criterion: an unknown report type is the server's call. The CLI
/// prints the server message verbatim with the request id and its own
/// allowlist never gets in the way.
#[tokio::test]
async fn project_report_unknown_type_surfaces_the_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/reports/burndown",
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {
                "code": "VALIDATION_ERROR",
                "message": "unknown report type: burndown",
                "details": {"supported": ["velocity", "cumulative-flow", "epic-progress"]}
            },
            "requestId": "req-unknown-report"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM"])
        .args(["project", "report", "burndown"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown report type: burndown"))
        .stderr(predicate::str::contains("request id: req-unknown-report"));

    let output = base(&server, &dir)
        .args(["--json", "--org", "acme", "--project", "HAM"])
        .args(["project", "report", "burndown"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(body["error"]["requestId"], "req-unknown-report");
    assert_eq!(body["error"]["message"], "unknown report type: burndown");
    assert_eq!(body["error"]["details"]["supported"][0], "velocity");
}

/// The Sprint report renders commitment, statuses, carryover, and a burndown
/// bar per sample, and `--json` stays verbatim.
#[tokio::test]
async fn sprint_report_renders_report_and_passes_json_through() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SPRINT_REPORT))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-Request-Id", "req-sprint-report")
                .set_body_json(sprint_report()),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM"])
        .args(["sprint", "report", "sprint-1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Sprint 12 (sprint-1)"))
        .stdout(predicate::str::contains("committed"))
        .stdout(predicate::str::contains("12 items / 34 points"))
        .stdout(predicate::str::contains("75% points of final scope"))
        .stdout(predicate::str::contains("1 (1 → Backlog)"))
        .stdout(predicate::str::contains(
            "burndown (remaining points, | = ideal):",
        ))
        .stdout(predicate::str::contains("Sep 2"))
        .stdout(predicate::str::contains("more pages: yes"));

    let output = base(&server, &dir)
        .args(["--json", "--org", "acme", "--project", "HAM"])
        .args(["sprint", "report", "sprint-1"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body, sprint_report());
}

/// Sprint report pagination forwards `--limit` and `--since-cursor` to the
/// change feed and nothing else.
#[tokio::test]
async fn sprint_report_forwards_page_options() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SPRINT_REPORT))
        .respond_with(ResponseTemplate::new(200).set_body_json(sprint_report()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM"])
        .args([
            "sprint", "report", "sprint-1", "--limit", "5", "--cursor", "c2",
        ])
        .assert()
        .success();

    assert_eq!(
        received_query(&server).await,
        vec!["cursor=c2".to_string(), "limit=5".to_string()]
    );
}

/// Reports are reads: `--dry-run` is rejected the same way it is for other
/// read commands.
#[tokio::test]
async fn sprint_report_rejects_dry_run() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM", "--dry-run"])
        .args(["sprint", "report", "sprint-1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--dry-run"));
}
