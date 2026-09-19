// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Local mutation audit log contract: every mutation appends exactly one
//! redacted record, reads append nothing, the documented opt-out is honored,
//! and `doctor` reports the effective location.
//!
//! `HAMSTIK_AUDIT_LOG` pins the log inside each test's temporary directory so
//! the tests never touch a real user's audit log.

use std::fs;

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "super-secret-token";
const TITLE: &str = "Confidential acquisition plan";

fn work_item_json(revision: i64) -> Value {
    json!({
        "id": "1", "key": "HAM-1", "projectId": "2", "title": TITLE, "description": null,
        "type": "task", "status": "todo", "priority": "low", "assignee": null, "reporter": null,
        "sprint": null, "parent": null, "labels": [],
        "storyPoints": null, "dueDate": null, "archivedAt": null,
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": revision
    })
}

fn page(items: Value) -> Value {
    json!({ "items": items, "page": { "limit": 50, "hasMore": false, "nextCursor": null } })
}

/// Base command wired to `server` with a throwaway config, an ephemeral token,
/// and an audit log inside the same temporary directory.
fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", audit_log(dir));
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", TOKEN);
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn audit_log(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("state").join("audit.log")
}

fn write_config(dir: &TempDir, contents: &str) {
    fs::write(dir.path().join("config.toml"), contents).unwrap();
}

fn read_audit(dir: &TempDir) -> String {
    let path = audit_log(dir);
    match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => panic!("cannot read {}: {err}", path.display()),
    }
}

async fn mount_work_create(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("ETag", "\"1\"")
                .insert_header("X-Request-Id", "req-abc-123")
                .set_body_json(work_item_json(1)),
        )
        .mount(server)
        .await;
}

fn create(dir: &TempDir, server: &MockServer) -> String {
    let output = base(server, dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--title",
            TITLE,
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "mutation must succeed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutation_appends_exactly_one_entry() {
    let server = MockServer::start().await;
    mount_work_create(&server).await;
    let dir = TempDir::new().unwrap();

    create(&dir, &server);

    let raw = read_audit(&dir);
    let lines: Vec<&str> = raw.lines().filter(|line| !line.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "exactly one audit record expected: {raw}");

    let entry: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(entry["command"], "work.create");
    assert_eq!(entry["target"], "HAM-1");
    assert!(
        entry["when"].as_str().unwrap().ends_with('Z'),
        "when must be an RFC 3339 timestamp: {entry}"
    );
    assert_eq!(entry["requestId"], "req-abc-123");
    assert_eq!(entry["revisionAfter"], 1);
    assert!(
        entry["revisionBefore"].is_null(),
        "creates have no revision before"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_record_never_contains_credentials_or_bodies() {
    let server = MockServer::start().await;
    mount_work_create(&server).await;
    let dir = TempDir::new().unwrap();

    create(&dir, &server);

    let raw = read_audit(&dir);
    assert!(!raw.is_empty(), "the audit record must still be written");
    for forbidden in [TOKEN, "Bearer", "Authorization", "idempotency", TITLE] {
        assert!(
            !raw.contains(forbidden),
            "audit log must not contain {forbidden:?}; found in: {raw}"
        );
    }

    let entry: Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
    let keys: Vec<&str> = entry
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys.len(), 6, "unexpected audit schema: {entry}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_log_opt_out_writes_nothing() {
    let server = MockServer::start().await;
    mount_work_create(&server).await;
    let dir = TempDir::new().unwrap();
    write_config(&dir, "version = 2\n[settings]\naudit_log = false\n");

    create(&dir, &server);

    assert!(
        !audit_log(&dir).exists(),
        "audit_log = false must not create the log: {}",
        read_audit(&dir)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_commands_write_no_entry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            {"id": "o1", "slug": "acme", "name": "Acme", "suspended": false, "plan": "pro"}
        ]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["org", "list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());

    assert_eq!(read_audit(&dir), "", "reads must not audit");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_reports_the_audit_log_path() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();

    let output = base(&server, &dir)
        .args(["--no-input", "--json", "doctor", "--local-only"])
        .output()
        .unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let audit = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "local.audit_log")
        .unwrap_or_else(|| panic!("doctor must report local.audit_log: {body}"));

    let detail = audit["detail"].as_str().unwrap();
    let expected = audit_log(&dir).display().to_string();
    assert!(
        detail.contains(&expected),
        "audit check must name the effective log path {expected}: {detail}"
    );
    assert_eq!(audit["status"], "pass");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_reports_the_opt_out() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    write_config(&dir, "version = 2\n[settings]\naudit_log = false\n");

    let output = base(&server, &dir)
        .args(["--no-input", "--json", "doctor", "--local-only"])
        .output()
        .unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let audit = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "local.audit_log")
        .unwrap_or_else(|| panic!("doctor must report local.audit_log: {body}"));

    assert_eq!(audit["status"], "skipped");
    assert!(
        audit["detail"]
            .as_str()
            .unwrap()
            .contains("settings.audit_log"),
        "the detail must name the opt-out: {audit}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_set_and_get_roundtrip_the_opt_out() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();

    let set = base(&server, &dir)
        .args(["config", "set", "audit_log", "false", "--json"])
        .output()
        .unwrap();
    assert!(
        set.status.success(),
        "config set audit_log must work: {}{}",
        String::from_utf8_lossy(&set.stdout),
        String::from_utf8_lossy(&set.stderr)
    );

    let get = base(&server, &dir)
        .args(["config", "get", "audit_log", "--json"])
        .output()
        .unwrap();
    assert!(get.status.success());
    let body: Value = serde_json::from_slice(&get.stdout).unwrap();
    assert_eq!(body["value"], "false");

    let listed = base(&server, &dir)
        .args(["config", "list", "--json"])
        .output()
        .unwrap();
    assert!(listed.status.success());
    let body: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(body["audit_log"], "false");
}
/// The reads a guarded `work transition` performs first: the item at revision 7
/// and its allowed transitions.
async fn mount_transition_reads(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"7\"")
                .set_body_json(work_item_json(7)),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/transitions",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currentStatus": "in_progress",
            "transitions": [{ "targetStatus": "done" }]
        })))
        .mount(server)
        .await;
}

/// The single audit record written by a test, parsed.
fn only_entry(dir: &TempDir) -> Value {
    let raw = read_audit(dir);
    let lines: Vec<&str> = raw.lines().filter(|line| !line.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "exactly one audit record expected: {raw}");
    serde_json::from_str(lines[0]).unwrap()
}

/// A `--dry-run` preview performs the reads but sends no mutation, so it is not
/// audited.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dry_run_writes_no_entry() {
    let server = MockServer::start().await;
    mount_transition_reads(&server).await;
    let dir = TempDir::new().unwrap();

    let output = base(&server, &dir)
        .args([
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "transition",
            "HAM-1",
            "done",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mutations = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method.as_str() != "GET")
        .count();
    assert_eq!(mutations, 0, "a dry run must send no mutation");
    assert!(
        !audit_log(&dir).exists(),
        "a dry run must not leave an audit trail: {}",
        read_audit(&dir)
    );
}

/// A mutation that fails leaves no trail either: the log documents changes that
/// actually happened.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_mutation_writes_no_entry() {
    let server = MockServer::start().await;
    mount_transition_reads(&server).await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/transitions",
        ))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "error": {
                "code": "INTERNAL_ERROR",
                "message": "boom",
                "requestId": "req-5xx",
                "retryable": false
            }
        })))
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();

    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "transition",
            "HAM-1",
            "done",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(9),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !audit_log(&dir).exists(),
        "a failed mutation must not leave an audit trail: {}",
        read_audit(&dir)
    );
}

/// A guarded edit records the revision it replaced as `revisionBefore`: one
/// snapshot replaced by the other, not two appended snapshots.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guarded_edit_records_the_revisions_it_replaced() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"7\"")
                .set_body_json(work_item_json(7)),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"8\"")
                .set_body_json(work_item_json(8)),
        )
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();

    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "edit",
            "HAM-1",
            "--title",
            "Renamed",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let entry = only_entry(&dir);
    assert_eq!(entry["command"], "work.edit");
    assert_eq!(entry["target"], "HAM-1");
    assert_eq!(
        entry["revisionBefore"], 7,
        "the revision replaced by the edit"
    );
    assert_eq!(entry["revisionAfter"], 8);
}

/// A rejected transition writes nothing: the change never happened, so there is
/// no change to record (and no half-written revision pair to misread).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_transition_writes_no_entry() {
    let server = MockServer::start().await;
    mount_transition_reads(&server).await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/transitions",
        ))
        .respond_with(
            ResponseTemplate::new(409)
                .insert_header("X-Request-Id", "req-conflict")
                .set_body_json(json!({
                    "error": {
                        "code": "REVISION_CONFLICT",
                        "message": "stale revision",
                        "requestId": "req-conflict",
                        "retryable": false
                    }
                })),
        )
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();

    let output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "transition",
            "HAM-1",
            "done",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(6),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !audit_log(&dir).exists(),
        "a rejected transition must not leave an audit trail: {}",
        read_audit(&dir)
    );
}
