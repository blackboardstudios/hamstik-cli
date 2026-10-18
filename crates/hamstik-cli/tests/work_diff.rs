// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work diff` contract tests (CLI-56).
//!
//! Baseline and revision (`--since`) modes against a sandboxed HTTP server,
//! including the honest partial-coverage contract when the Public API
//! exposes no revision snapshots.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ITEM: &str = "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1";
const ACTIVITY: &str = "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/activity";

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
    cmd.env("HAMSTIK_ORG", "acme");
    cmd.env("HAMSTIK_PROJECT", "HAM");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn item(key: &str, title: &str, status: &str, revision: i64) -> Value {
    json!({
        "id": "id-HAM-1",
        "key": key,
        "projectId": "p1",
        "title": title,
        "description": null,
        "type": "task",
        "status": status,
        "priority": "medium",
        "assignee": null,
        "reporter": null,
        "sprint": null,
        "parent": null,
        "labels": [],
        "storyPoints": null,
        "dueDate": null,
        "archivedAt": null,
        "createdAt": "2026-09-01T00:00:00Z",
        "updatedAt": "2026-09-02T00:00:00Z",
        "revision": revision
    })
}

async fn mount_item(server: &MockServer, body: Value) {
    server
        .register(
            Mock::given(method("GET"))
                .and(path(ITEM))
                .respond_with(ResponseTemplate::new(200).set_body_json(body)),
        )
        .await;
}

async fn mount_activity(server: &MockServer, body: Value) {
    server
        .register(
            Mock::given(method("GET"))
                .and(path(ACTIVITY))
                .respond_with(ResponseTemplate::new(200).set_body_json(body)),
        )
        .await;
}

/// Baseline mode: after `--save-baseline`, an edit produces a per-field
/// before/after diff; the baseline file lives under the config directory.
#[tokio::test]
async fn diff_reports_field_changes_against_the_baseline() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    mount_item(&server, item("HAM-1", "Original title", "backlog", 4)).await;

    base(&server, &dir)
        .args(["work", "diff", "HAM-1", "--save-baseline"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("baseline saved for HAM-1"));

    // The baseline file is per-user, under the config directory, and carries
    // no credentials.
    let baseline_path = dir
        .path()
        .join("baselines")
        .join("acme")
        .join("HAM")
        .join("HAM-1.json");
    let baseline: Value = serde_json::from_slice(&std::fs::read(&baseline_path).unwrap()).unwrap();
    assert_eq!(baseline["baselineVersion"], json!(1));
    assert_eq!(baseline["revision"], json!(4));
    let serialized = serde_json::to_string(&baseline).unwrap();
    assert!(!serialized.contains("secret-token"));

    mount_item(&server, item("HAM-1", "Renamed title", "in_progress", 6)).await;
    // A description the baseline lacked.
    let mut changed = item("HAM-1", "Renamed title", "in_progress", 6);
    changed["description"] = json!("Now described");
    server.reset().await;
    mount_item(&server, changed).await;

    base(&server, &dir)
        .args(["--json", "work", "diff", "HAM-1"])
        .assert()
        .code(0)
        .stdout(predicate::function(|stdout: &str| {
            let body: Value = serde_json::from_str(stdout).unwrap();
            body["hasChanges"] == json!(true)
                && body["diffVersion"] == json!(1)
                && body["currentRevision"] == json!(6)
                && body["reference"]["kind"] == json!("baseline")
                && body["changes"]
                    == json!([
                        {"field": "title", "before": "Original title", "after": "Renamed title"},
                        {"field": "description", "before": null, "after": "Now described"},
                        {"field": "status", "before": "backlog", "after": "in_progress"},
                    ])
        }));
}

/// Unchanged comparisons exit 0 with an explicit no-changes result;
/// `--fail-on-change` keeps exit 0 when nothing differs.
#[tokio::test]
async fn unchanged_comparison_exits_zero_explicitly() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    mount_item(&server, item("HAM-1", "Same", "backlog", 4)).await;
    base(&server, &dir)
        .args(["work", "diff", "HAM-1", "--save-baseline"])
        .assert()
        .code(0);

    base(&server, &dir)
        .args(["--json", "work", "diff", "HAM-1", "--fail-on-change"])
        .assert()
        .code(0)
        .stdout(predicate::function(|stdout: &str| {
            let body: Value = serde_json::from_str(stdout).unwrap();
            body["hasChanges"] == json!(false) && body["changes"] == json!([])
        }));

    base(&server, &dir)
        .args(["work", "diff", "HAM-1", "--fail-on-change"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("no changes for HAM-1"));
}

/// `--fail-on-change` exits with the conflict exit code when the item differs.
#[tokio::test]
async fn fail_on_change_exits_with_the_conflict_code() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    mount_item(&server, item("HAM-1", "Before", "backlog", 4)).await;
    base(&server, &dir)
        .args(["work", "diff", "HAM-1", "--save-baseline"])
        .assert()
        .code(0);
    mount_item(&server, item("HAM-1", "After", "backlog", 5)).await;

    // wiremock stacks mocks: clear the baseline-save fixture first so the
    // changed item is the one that answers.
    server.reset().await;
    mount_item(&server, item("HAM-1", "After", "backlog", 5)).await;

    base(&server, &dir)
        .args(["work", "diff", "HAM-1", "--fail-on-change"])
        .assert()
        .code(6)
        .stdout(predicate::str::contains("title"));
}

/// A missing baseline fails clearly with remediation (exit 5), before any
/// diff is attempted.
#[tokio::test]
async fn missing_baseline_fails_with_remediation() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    mount_item(&server, item("HAM-1", "Anything", "backlog", 4)).await;

    base(&server, &dir)
        .args(["work", "diff", "HAM-1"])
        .assert()
        .code(5)
        .stderr(predicate::str::contains("no baseline stored for HAM-1"))
        .stderr(predicate::str::contains("--save-baseline"));
}

/// `--since` reports the revision delta with explicit partial coverage: the
/// API exposes no revision snapshots, so the document says so instead of
/// fabricating a diff.
#[tokio::test]
async fn since_mode_reports_partial_coverage_without_revision_snapshots() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    mount_item(&server, item("HAM-1", "Current", "in_progress", 7)).await;
    mount_activity(
        &server,
        json!({
            "items": [
                {"id": "e1", "action": "status_changed", "actor": null,
                 "detail": {"from": "backlog", "to": "in_progress"},
                 "createdAt": "2026-10-18T15:00:00Z"},
                {"id": "e2", "action": "comment_added", "actor": null,
                 "detail": null, "createdAt": "2026-10-18T15:01:00Z"}
            ],
            "page": {"limit": 100, "hasMore": false, "nextCursor": null}
        }),
    )
    .await;

    base(&server, &dir)
        .args(["--json", "work", "diff", "HAM-1", "--since", "4"])
        .assert()
        .code(0)
        .stdout(predicate::function(|stdout: &str| {
            let body: Value = serde_json::from_str(stdout).unwrap();
            body["hasChanges"] == json!(true)
                && body["coverage"] == json!("partial")
                && body["historySupported"] == json!(false)
                && body["revisionDelta"] == json!({"from": 4, "to": 7})
                && body["changes"]
                    == json!([
                        {"field": "status", "action": "status_changed",
                         "before": "backlog", "after": "in_progress",
                         "at": "2026-10-18T15:00:00Z", "actor": null}
                    ])
                && body["note"]
                    .as_str()
                    .unwrap()
                    .contains("no Work Item revision snapshots")
        }));

    // Human rendering surfaces the same evidence.
    base(&server, &dir)
        .args(["work", "diff", "HAM-1", "--since", "4"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("status"))
        .stdout(predicate::str::contains("in_progress"));
}

/// `--since` with the current revision reports no changes; a newer reference
/// is a usage error.
#[tokio::test]
async fn since_mode_edge_cases() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    mount_item(&server, item("HAM-1", "Current", "backlog", 7)).await;

    base(&server, &dir)
        .args(["--json", "work", "diff", "HAM-1", "--since", "7"])
        .assert()
        .code(0)
        .stdout(predicate::function(|stdout: &str| {
            let body: Value = serde_json::from_str(stdout).unwrap();
            body["hasChanges"] == json!(false)
        }));

    base(&server, &dir)
        .args(["--json", "work", "diff", "HAM-1", "--since", "9"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("newer than the current revision"));
}

/// Baselines validate their Organization/Project/key context: a baseline
/// recorded for another context is refused rather than silently compared.
#[tokio::test]
async fn baseline_context_mismatch_is_refused() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    mount_item(&server, item("HAM-1", "Original", "backlog", 4)).await;
    base(&server, &dir)
        .args(["work", "diff", "HAM-1", "--save-baseline"])
        .assert()
        .code(0);

    // Rewrite the recorded context inside the baseline file, simulating a
    // baseline saved under a different Organization/Project.
    let baseline_path = dir
        .path()
        .join("baselines")
        .join("acme")
        .join("HAM")
        .join("HAM-1.json");
    let mut baseline: Value =
        serde_json::from_slice(&std::fs::read(&baseline_path).unwrap()).unwrap();
    baseline["project"] = json!("OTHER");
    std::fs::write(
        &baseline_path,
        serde_json::to_vec_pretty(&baseline).unwrap(),
    )
    .unwrap();

    base(&server, &dir)
        .args(["work", "diff", "HAM-1"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("different Organization/Project"));
}
