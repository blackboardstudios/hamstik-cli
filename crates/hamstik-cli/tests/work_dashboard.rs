// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work dashboard` composition contract tests: the configurable Project set
//! (flags and `.hamstik.toml`), per-Project attribution, per-section failure
//! isolation, `--json` schema, and human/quiet rendering.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const MY_WORK: &str = "/api/v1/my/work";
const HAM_WORK: &str = "/api/v1/organizations/acme/projects/HAM/work-items";
const WEB_WORK: &str = "/api/v1/organizations/acme/projects/WEB/work-items";

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
    cmd
}

fn work_item(key: &str, project: &str, status: &str) -> Value {
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
        "dueDate": "2026-09-30T00:00:00Z",
        "archivedAt": null,
        "createdAt": "2026-09-01T00:00:00Z",
        "updatedAt": "2026-09-02T00:00:00Z",
        "project": {"id": "p1", "key": project, "name": project, "color": "#00ff00"},
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
        "requestId": "req-dashboard"
    })
}

async fn mount_page(server: &MockServer, endpoint: &str, items: Value) {
    Mock::given(method("GET"))
        .and(path(endpoint))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(items)))
        .mount(server)
        .await;
}

async fn call(server: &MockServer, dir: &TempDir, args: &[&str]) -> std::process::Output {
    base(server, dir).args(args).output().unwrap()
}

async fn call_ok(server: &MockServer, dir: &TempDir, args: &[&str]) -> String {
    let output = call(server, dir, args).await;
    assert!(
        output.status.success(),
        "args {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn section<'a>(doc: &'a Value, id: &str) -> &'a Value {
    doc["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["id"] == json!(id))
        .unwrap_or_else(|| panic!("missing section {id}"))
}

/// Acceptance: every included Project's items are attributed and composed from
/// the existing My Work and per-Project list reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_composes_mine_and_each_project() {
    let server = MockServer::start().await;
    mount_page(&server, MY_WORK, json!([work_item("HAM-1", "HAM", "todo")])).await;
    mount_page(
        &server,
        HAM_WORK,
        json!([work_item("HAM-2", "HAM", "in_progress")]),
    )
    .await;
    mount_page(
        &server,
        WEB_WORK,
        json!([work_item("WEB-1", "WEB", "todo")]),
    )
    .await;

    let dir = TempDir::new().unwrap();
    let output = call_ok(
        &server,
        &dir,
        &[
            "--org",
            "acme",
            "work",
            "dashboard",
            "--project",
            "HAM",
            "--project",
            "WEB",
            "--json",
        ],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();

    assert_eq!(doc["dashboardVersion"], 1);
    assert_eq!(doc["organization"], "acme");
    assert_eq!(doc["projects"], json!(["HAM", "WEB"]));
    assert_eq!(doc["mineIncluded"], true);
    assert_eq!(doc["failedSections"], json!([]));

    assert_eq!(section(&doc, "mine")["status"], "ok");
    assert_eq!(section(&doc, "mine")["items"][0]["key"], "HAM-1");
    assert_eq!(section(&doc, "project:HAM")["status"], "ok");
    assert_eq!(section(&doc, "project:HAM")["items"][0]["key"], "HAM-2");
    assert_eq!(section(&doc, "project:WEB")["status"], "ok");
    assert_eq!(section(&doc, "project:WEB")["items"][0]["key"], "WEB-1");

    // The reads are exactly the documented `work mine` and `work list` reads.
    let requests = server.received_requests().await.unwrap();
    let urls: Vec<String> = requests.iter().map(|r| r.url.to_string()).collect();
    assert!(
        urls.iter()
            .any(|u| u.contains("my/work") && u.contains("scope=open")),
        "{urls:?}"
    );
    assert!(
        urls.iter().any(|u| u.contains("projects/HAM/work-items")),
        "{urls:?}"
    );
    assert!(
        urls.iter().any(|u| u.contains("projects/WEB/work-items")),
        "{urls:?}"
    );
}

/// Acceptance: one failing Project is explicit and never hides the others.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_failing_project_does_not_hide_the_others() {
    let server = MockServer::start().await;
    mount_page(&server, MY_WORK, json!([])).await;
    mount_page(
        &server,
        HAM_WORK,
        json!([work_item("HAM-2", "HAM", "todo")]),
    )
    .await;
    Mock::given(method("GET"))
        .and(path(WEB_WORK))
        .respond_with(ResponseTemplate::new(500).set_body_json(error_body("INTERNAL_ERROR", 500)))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = call(
        &server,
        &dir,
        &[
            "--org",
            "acme",
            "work",
            "dashboard",
            "--project",
            "HAM",
            "--project",
            "WEB",
            "--json",
        ],
    )
    .await;
    // A partial snapshot is still usable: the command succeeds and reports the
    // failure in-band instead of aborting.
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let doc: Value = serde_json::from_str(&String::from_utf8(output.stdout).unwrap()).unwrap();

    assert_eq!(section(&doc, "project:HAM")["status"], "ok");
    assert_eq!(section(&doc, "project:HAM")["items"][0]["key"], "HAM-2");
    assert_eq!(section(&doc, "project:WEB")["status"], "error");
    assert_eq!(
        section(&doc, "project:WEB")["error"]["code"],
        "INTERNAL_ERROR"
    );
    assert_eq!(doc["failedSections"], json!(["project:WEB"]));
}

/// The Project set comes from `.hamstik.toml` when no `--project` flag is
/// given, and flags override the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_set_comes_from_context_file() {
    let server = MockServer::start().await;
    mount_page(&server, MY_WORK, json!([])).await;
    mount_page(&server, HAM_WORK, json!([])).await;
    mount_page(&server, WEB_WORK, json!([])).await;

    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(".hamstik.toml"),
        "version = 1\norganization = \"acme\"\ndashboard_projects = [\"HAM\", \"WEB\"]\n",
    )
    .unwrap();

    let output = call_ok(&server, &dir, &["work", "dashboard", "--json"]).await;
    let doc: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(doc["projects"], json!(["HAM", "WEB"]));
    assert!(
        doc["sections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "project:WEB")
    );

    // An explicit flag replaces the configured set entirely.
    let output = call_ok(
        &server,
        &dir,
        &["work", "dashboard", "--project", "WEB", "--json"],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(doc["projects"], json!(["WEB"]));
    assert!(
        !doc["sections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "project:HAM")
    );
}

/// Without `dashboard_projects`, the resolved single Project is the fallback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_set_falls_back_to_resolved_project() {
    let server = MockServer::start().await;
    mount_page(&server, MY_WORK, json!([])).await;
    mount_page(&server, HAM_WORK, json!([])).await;

    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(".hamstik.toml"),
        "version = 1\norganization = \"acme\"\nproject = \"HAM\"\n",
    )
    .unwrap();

    let output = call_ok(&server, &dir, &["work", "dashboard", "--json"]).await;
    let doc: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(doc["projects"], json!(["HAM"]));
    assert_eq!(section(&doc, "project:HAM")["status"], "ok");
}

/// With neither flags nor a configured Project, the command fails as a usage
/// error before contacting the API.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_project_set_is_a_usage_error() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();

    let output = call(
        &server,
        &dir,
        &["--org", "acme", "work", "dashboard", "--json"],
    )
    .await;
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("no projects configured"), "{stderr}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// `--mine false` omits the My Work section and makes no `/my/work` request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mine_can_be_disabled() {
    let server = MockServer::start().await;
    mount_page(&server, HAM_WORK, json!([])).await;

    let dir = TempDir::new().unwrap();
    let output = call_ok(
        &server,
        &dir,
        &[
            "--org",
            "acme",
            "work",
            "dashboard",
            "--project",
            "HAM",
            "--mine",
            "false",
            "--json",
        ],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(doc["mineIncluded"], false);
    assert!(
        !doc["sections"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "mine")
    );
    let requests = server.received_requests().await.unwrap();
    assert!(!requests.iter().any(|r| r.url.path() == MY_WORK));
}

/// The default scope is `open` and `--scope` overrides it on every section.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scope_is_forwarded_to_every_section() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(HAM_WORK))
        .and(query_param("scope", "closed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    call_ok(
        &server,
        &dir,
        &[
            "--org",
            "acme",
            "work",
            "dashboard",
            "--project",
            "HAM",
            "--mine",
            "false",
            "--scope",
            "closed",
            "--json",
        ],
    )
    .await;
}

/// Human output attributes every section to its Project and warns, without
/// hiding a healthy Project, when another fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_output_attributes_projects_and_reports_failures() {
    let server = MockServer::start().await;
    mount_page(
        &server,
        HAM_WORK,
        json!([work_item("HAM-2", "HAM", "todo")]),
    )
    .await;
    Mock::given(method("GET"))
        .and(path(WEB_WORK))
        .respond_with(ResponseTemplate::new(500).set_body_json(error_body("INTERNAL_ERROR", 500)))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = call(
        &server,
        &dir,
        &[
            "--org",
            "acme",
            "work",
            "dashboard",
            "--project",
            "HAM",
            "--project",
            "WEB",
            "--mine",
            "false",
        ],
    )
    .await;
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stdout.contains("Project HAM"), "{stdout}");
    assert!(stdout.contains("HAM-2"), "{stdout}");
    assert!(stdout.contains("Project WEB"), "{stdout}");
    assert!(stdout.contains("unavailable"), "{stdout}");
    assert!(stderr.contains("project:WEB section failed"), "{stderr}");
}

/// `--fields` is the documented server-side sparse fieldset and an unknown
/// name fails locally before any request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_fields_fail_before_any_request() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();

    let output = call(
        &server,
        &dir,
        &[
            "--org",
            "acme",
            "work",
            "dashboard",
            "--project",
            "HAM",
            "--fields",
            "bogus",
            "--json",
        ],
    )
    .await;
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("unknown field"), "{stderr}");
    assert!(server.received_requests().await.unwrap().is_empty());
}
