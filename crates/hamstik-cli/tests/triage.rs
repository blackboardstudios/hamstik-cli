// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work triage` composition contract tests: per-section JSON schema,
//! graceful section failure, skipped activity, and human/quiet rendering.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MY_WORK: &str = "/api/v1/my/work";
const ACTIVITY: &str = "/api/v1/organizations/acme/projects/HAM/activity";

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

fn work_item(key: &str, status: &str, due: Option<&str>) -> Value {
    json!({
        "id": format!("id-{key}"),
        "key": key,
        "title": format!("Item {key}"),
        "revision": 1,
        "type": "task",
        "status": status,
        "priority": "high",
        "assignee": {"id": "u1", "name": "Alice"},
        "sprint": null,
        "parentId": null,
        "storyPoints": null,
        "dueDate": due,
        "archivedAt": null,
        "createdAt": "2026-09-01T00:00:00Z",
        "updatedAt": "2026-09-02T00:00:00Z",
        "project": {"id": "p1", "key": "HAM", "name": "Ham", "color": "#00ff00"},
        "organization": {"id": "o1", "slug": "acme", "name": "Acme"}
    })
}

fn page(items: Value) -> Value {
    json!({
        "items": items,
        "page": {"limit": 50, "hasMore": false, "nextCursor": null}
    })
}

fn error_body(code: &str, status: u16) -> Value {
    json!({
        "error": {"code": code, "message": "boom", "status": status},
        "requestId": "req-triage"
    })
}

async fn mount_assigned(server: &MockServer, items: Value) {
    Mock::given(method("GET"))
        .and(path(MY_WORK))
        .and(query_param("scope", "open"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(items)))
        .mount(server)
        .await;
}

async fn mount_overdue(server: &MockServer, items: Value) {
    Mock::given(method("GET"))
        .and(path(MY_WORK))
        .and(query_param("overdue", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(items)))
        .mount(server)
        .await;
}

async fn mount_activity(server: &MockServer, events: Value) {
    Mock::given(method("GET"))
        .and(path(ACTIVITY))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(events)))
        .mount(server)
        .await;
}

async fn call(server: &MockServer, args: &[&str]) -> std::process::Output {
    let dir = TempDir::new().unwrap();
    base(server, &dir).args(args).output().unwrap()
}

async fn call_ok(server: &MockServer, args: &[&str]) -> String {
    let output = call(server, args).await;
    assert!(
        output.status.success(),
        "args {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_composes_all_sections_from_existing_reads() {
    let server = MockServer::start().await;
    mount_assigned(&server, json!([work_item("HAM-1", "todo", None)])).await;
    mount_overdue(
        &server,
        json!([work_item("HAM-2", "todo", Some("2026-08-01T00:00:00Z"))]),
    )
    .await;
    mount_activity(
        &server,
        json!([{
            "id": "a1", "action": "created", "actor": {"id": "u1", "name": "Alice"},
            "detail": null, "createdAt": "2026-09-02T00:00:00Z",
            "workItem": {"id": "1", "key": "HAM-3", "title": "T"}
        }]),
    )
    .await;

    let output = call_ok(
        &server,
        &[
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "triage",
            "--json",
        ],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();

    assert_eq!(doc["triageVersion"], 1);
    assert_eq!(doc["failedSections"], json!([]));

    assert_eq!(doc["sections"]["assignedOpen"]["status"], "ok");
    assert_eq!(doc["sections"]["assignedOpen"]["items"][0]["key"], "HAM-1");
    assert_eq!(doc["sections"]["overdue"]["status"], "ok");
    assert_eq!(doc["sections"]["overdue"]["items"][0]["key"], "HAM-2");
    assert_eq!(doc["sections"]["activity"]["status"], "ok");
    assert_eq!(doc["sections"]["activity"]["items"][0]["action"], "created");

    // The individual reads were exactly the ones the documented commands use.
    let requests = server.received_requests().await.unwrap();
    let urls: Vec<String> = requests.iter().map(|r| r.url.to_string()).collect();
    assert!(
        urls.iter()
            .any(|u| u.contains("my/work") && u.contains("scope=open")),
        "{urls:?}"
    );
    assert!(
        urls.iter()
            .any(|u| u.contains("my/work") && u.contains("overdue=true")),
        "{urls:?}"
    );
    assert!(
        urls.iter().any(|u| u.contains("projects/HAM/activity")),
        "{urls:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_section_is_marked_and_never_fatal() {
    let server = MockServer::start().await;
    mount_assigned(&server, json!([work_item("HAM-1", "todo", None)])).await;
    // Overdue read fails; the command must still succeed and report it.
    Mock::given(method("GET"))
        .and(path(MY_WORK))
        .and(query_param("overdue", "true"))
        .respond_with(ResponseTemplate::new(500).set_body_json(error_body("INTERNAL_ERROR", 500)))
        .mount(&server)
        .await;
    mount_activity(&server, json!([])).await;

    let output = call_ok(
        &server,
        &[
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "triage",
            "--json",
        ],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();

    assert_eq!(doc["sections"]["assignedOpen"]["status"], "ok");
    assert_eq!(doc["sections"]["overdue"]["status"], "error");
    assert_eq!(
        doc["sections"]["overdue"]["error"]["code"],
        "INTERNAL_ERROR"
    );
    assert_eq!(doc["failedSections"], json!(["overdue"]));
    // A neighbouring section still loaded.
    assert_eq!(doc["sections"]["activity"]["status"], "ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_is_skipped_without_project_context() {
    let server = MockServer::start().await;
    mount_assigned(&server, json!([work_item("HAM-1", "todo", None)])).await;
    mount_overdue(&server, json!([])).await;

    let output = call_ok(&server, &["--org", "acme", "work", "triage", "--json"]).await;
    let doc: Value = serde_json::from_str(&output).unwrap();

    assert_eq!(doc["sections"]["activity"]["status"], "skipped");
    assert!(
        doc["sections"]["activity"]["reason"]
            .as_str()
            .unwrap()
            .contains("no project selected")
    );
    // No Project activity request is made when no Project is resolved.
    let requests = server.received_requests().await.unwrap();
    assert!(
        !requests.iter().any(|r| r.url.path().ends_with("/activity")),
        "unexpected activity request"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_section_honors_disable_and_since() {
    let server = MockServer::start().await;
    mount_assigned(&server, json!([])).await;
    mount_overdue(&server, json!([])).await;
    mount_activity(&server, json!([])).await;

    let output = call_ok(
        &server,
        &[
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "triage",
            "--json",
            "--activity",
            "0",
        ],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(doc["sections"]["activity"]["status"], "skipped");
    assert!(
        doc["sections"]["activity"]["reason"]
            .as_str()
            .unwrap()
            .contains("--activity 0")
    );

    let output = call_ok(
        &server,
        &[
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "triage",
            "--json",
            "--since",
            "2026-09-01",
            "--activity",
            "5",
        ],
    )
    .await;
    let _: Value = serde_json::from_str(&output).unwrap();
    let requests = server.received_requests().await.unwrap();
    let activity_request = requests
        .iter()
        .find(|r| r.url.path().ends_with("/activity"))
        .expect("activity request");
    assert!(
        activity_request
            .url
            .query()
            .unwrap_or_default()
            .contains("since="),
        "since filter forwarded"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_output_labels_sections_and_reports_failure() {
    let server = MockServer::start().await;
    mount_assigned(&server, json!([work_item("HAM-1", "todo", None)])).await;
    Mock::given(method("GET"))
        .and(path(MY_WORK))
        .and(query_param("overdue", "true"))
        .respond_with(ResponseTemplate::new(403).set_body_json(error_body("FORBIDDEN", 403)))
        .mount(&server)
        .await;
    mount_activity(&server, json!([])).await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM", "work", "triage"])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stdout.contains("Work triage"));
    assert!(stdout.contains("Assigned (open)"));
    assert!(stdout.contains("HAM-1"));
    assert!(stdout.contains("Overdue — unavailable"));
    assert!(stdout.contains("Recent project activity"));
    assert!(stderr.contains("overdue section failed"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quiet_mode_prints_work_item_identifiers_only() {
    let server = MockServer::start().await;
    mount_assigned(&server, json!([work_item("HAM-1", "todo", None)])).await;
    mount_overdue(
        &server,
        json!([work_item("HAM-2", "todo", Some("2026-08-01T00:00:00Z"))]),
    )
    .await;
    mount_activity(
        &server,
        json!([{
            "id": "a1", "action": "created", "actor": null,
            "detail": null, "createdAt": "2026-09-02T00:00:00Z",
            "workItem": {"id": "1", "key": "HAM-3", "title": "T"}
        }]),
    )
    .await;

    let output = call_ok(
        &server,
        &[
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "triage",
            "--quiet",
        ],
    )
    .await;
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(lines.contains(&"HAM-1"), "{output}");
    assert!(lines.contains(&"HAM-2"), "{output}");
    assert!(!output.contains("Work triage"), "{output}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quiet_and_json_are_mutually_exclusive_before_any_request() {
    let server = MockServer::start().await;
    mount_assigned(&server, json!([])).await;
    mount_overdue(&server, json!([])).await;

    for args in [
        vec!["--json", "--quiet", "work", "triage"],
        vec!["--jsonl", "--quiet", "work", "triage"],
    ] {
        let output = call(&server, &args).await;
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?} wrote stdout");
    }
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "conflicts must not hit the API"
    );
}
