// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Integration tests for `hamstik project stats` and `hamstik sprint stats`:
//! JSON output shape, human rendering, filter forwarding, and truncation
//! reporting.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WORK_ITEMS: &str = "/api/v1/organizations/acme/projects/HAM/work-items";

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

fn item(
    key: &str,
    title: &str,
    status: &str,
    typ: &str,
    priority: &str,
    assignee: Option<&str>,
    story_points: Option<i64>,
) -> Value {
    json!({
        "id": format!("id-{key}"), "key": key, "projectId": "p1", "title": title,
        "description": null, "type": typ, "status": status, "priority": priority,
        "assignee": assignee.map(|name| json!({"id": "u1", "name": name})),
        "reporter": {"id": "u1", "name": "Rae"},
        "sprint": null, "parent": null, "labels": [],
        "storyPoints": story_points,
        "dueDate": null, "archivedAt": null,
        "createdAt": "2026-09-01T00:00:00Z", "updatedAt": "2026-09-02T00:00:00Z",
        "revision": 1
    })
}

async fn mount_items(server: &MockServer, items: Vec<Value>, next_cursor: Option<&str>) {
    let body = json!({
        "items": items,
        "page": {
            "limit": 200,
            "hasMore": next_cursor.is_some(),
            "nextCursor": next_cursor
        }
    });
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// Mount items with status filter awareness for filter testing.
async fn mount_items_filtered(
    server: &MockServer,
    all_items: Vec<Value>,
    next_cursor: Option<&str>,
    status_filter: Option<&str>,
) {
    let filtered: Vec<Value> = if let Some(status) = status_filter {
        all_items
            .into_iter()
            .filter(|item| {
                item.get("status")
                    .and_then(Value::as_str)
                    .map(|s| s == status)
                    .unwrap_or(false)
            })
            .collect()
    } else {
        all_items
    };
    let body = json!({
        "items": filtered,
        "page": {
            "limit": 200,
            "hasMore": next_cursor.is_some(),
            "nextCursor": next_cursor
        }
    });
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn call(server: &MockServer, args: &[&str]) -> std::process::Output {
    let dir = TempDir::new().unwrap();
    base(server, &dir)
        .args(["--org", "acme", "--project", "HAM"])
        .args(args)
        .output()
        .unwrap()
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

#[tokio::test]
async fn project_stats_json_returns_deterministic_counts() {
    let server = MockServer::start().await;
    let items = vec![
        item(
            "HAM-1",
            "First",
            "todo",
            "task",
            "high",
            Some("Alice"),
            Some(3),
        ),
        item("HAM-2", "Second", "done", "bug", "low", Some("Bob"), None),
        item(
            "HAM-3",
            "Third",
            "in_progress",
            "story",
            "medium",
            Some("Alice"),
            Some(5),
        ),
    ];
    mount_items(&server, items.clone(), None).await;

    let output = call_ok(&server, &["project", "stats", "HAM", "--json"]).await;
    let doc: Value = serde_json::from_str(&output).unwrap();

    // Top-level shape
    assert!(doc["summary"].is_object());
    assert_eq!(doc["summary"]["total"], 3);
    assert!(doc["computed"].is_string());
    assert!(doc["computed"].as_str().unwrap().contains("client-side"));
    assert_eq!(doc["itemsFetched"], 3);

    // By status
    assert_eq!(doc["summary"]["byStatus"]["todo"], 1);
    assert_eq!(doc["summary"]["byStatus"]["done"], 1);
    assert_eq!(doc["summary"]["byStatus"]["in_progress"], 1);

    // By type
    assert_eq!(doc["summary"]["byType"]["task"], 1);
    assert_eq!(doc["summary"]["byType"]["bug"], 1);
    assert_eq!(doc["summary"]["byType"]["story"], 1);

    // By priority
    assert_eq!(doc["summary"]["byPriority"]["high"], 1);
    assert_eq!(doc["summary"]["byPriority"]["low"], 1);
    assert_eq!(doc["summary"]["byPriority"]["medium"], 1);

    // By assignee
    assert_eq!(doc["summary"]["byAssignee"]["Alice"], 2);
    assert_eq!(doc["summary"]["byAssignee"]["Bob"], 1);

    // Story points
    assert_eq!(doc["summary"]["totalStoryPoints"], 8);
    assert_eq!(doc["summary"]["estimatedItems"], 2);
}

#[tokio::test]
async fn project_stats_json_with_filter() {
    let server = MockServer::start().await;
    let items = vec![
        item(
            "HAM-1",
            "First",
            "todo",
            "task",
            "high",
            Some("Alice"),
            Some(3),
        ),
        item("HAM-2", "Second", "done", "bug", "low", Some("Bob"), None),
        item(
            "HAM-3",
            "Third",
            "in_progress",
            "story",
            "medium",
            Some("Alice"),
            Some(5),
        ),
    ];
    mount_items_filtered(&server, items, None, Some("todo")).await;

    let output = call_ok(
        &server,
        &["project", "stats", "HAM", "--json", "--status", "todo"],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();

    assert_eq!(doc["summary"]["total"], 1);
    assert_eq!(doc["summary"]["byStatus"]["todo"], 1);
    // Status filter is applied server-side, so only todo items are returned
}

#[tokio::test]
async fn project_stats_human_renders_table() {
    let server = MockServer::start().await;
    let items = vec![
        item(
            "HAM-1",
            "First",
            "todo",
            "task",
            "high",
            Some("Alice"),
            Some(3),
        ),
        item("HAM-2", "Second", "done", "bug", "low", Some("Bob"), None),
    ];
    mount_items(&server, items.clone(), None).await;

    let output = call_ok(&server, &["project", "stats", "HAM"]).await;

    assert!(output.contains("Aggregate summary for: HAM"));
    assert!(output.contains("Total items: 2"));
    assert!(output.contains("By status:"));
    assert!(output.contains("todo"));
    assert!(output.contains("done"));
    assert!(output.contains("By type:"));
    assert!(output.contains("task"));
    assert!(output.contains("bug"));
    assert!(output.contains("By priority:"));
    assert!(output.contains("By assignee:"));
    assert!(output.contains("Alice"));
    assert!(output.contains("Bob"));
    assert!(output.contains("client-side computed summary"));
}

#[tokio::test]
async fn sprint_stats_json_returns_counts() {
    let server = MockServer::start().await;
    let items = vec![
        item(
            "HAM-1",
            "First",
            "todo",
            "task",
            "high",
            Some("Alice"),
            Some(3),
        ),
        item("HAM-2", "Second", "done", "bug", "low", Some("Bob"), None),
    ];
    mount_items(&server, items.clone(), None).await;

    let output = call_ok(&server, &["sprint", "stats", "sprint-uuid-123", "--json"]).await;
    let doc: Value = serde_json::from_str(&output).unwrap();

    assert!(doc["summary"].is_object());
    assert_eq!(doc["summary"]["total"], 2);
    assert!(
        doc["summary"]["title"]
            .as_str()
            .unwrap()
            .contains("sprint-uuid-123")
    );
}

#[tokio::test]
async fn sprint_stats_human_renders_table() {
    let server = MockServer::start().await;
    let items = vec![item(
        "HAM-1",
        "First",
        "todo",
        "task",
        "high",
        Some("Alice"),
        Some(3),
    )];
    mount_items(&server, items.clone(), None).await;

    let output = call_ok(&server, &["sprint", "stats", "sprint-uuid-123"]).await;

    assert!(output.contains("Aggregate summary for: sprint-uuid-123"));
    assert!(output.contains("Total items: 1"));
    assert!(output.contains("client-side computed summary"));
}

#[tokio::test]
async fn project_stats_empty_project() {
    let server = MockServer::start().await;
    mount_items(&server, vec![], None).await;

    let output = call_ok(&server, &["project", "stats", "HAM", "--json"]).await;
    let doc: Value = serde_json::from_str(&output).unwrap();

    assert_eq!(doc["summary"]["total"], 0);
    assert_eq!(doc["summary"]["byStatus"], json!({}));
    assert_eq!(doc["summary"]["totalStoryPoints"], 0);
    assert_eq!(doc["summary"]["estimatedItems"], 0);
    assert_eq!(doc["summary"]["truncated"], false);
}

#[tokio::test]
async fn project_stats_marks_truncation_when_server_has_more_pages() {
    let server = MockServer::start().await;
    mount_items(
        &server,
        vec![item(
            "HAM-1",
            "First",
            "todo",
            "task",
            "high",
            Some("Alice"),
            Some(3),
        )],
        Some("cursor-2"),
    )
    .await;

    // Without `--all` exactly one page is aggregated; the partial aggregate
    // must be labeled truncated in both output modes.
    let output = call_ok(&server, &["project", "stats", "HAM", "--json"]).await;
    let doc: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(doc["summary"]["truncated"], true);
    assert_eq!(doc["itemsFetched"], 1);

    let dir = TempDir::new().unwrap();
    let human = base(&server, &dir)
        .args(["--org", "acme", "--project", "HAM"])
        .args(["project", "stats", "HAM"])
        .output()
        .unwrap();
    assert!(human.status.success());
    assert!(
        String::from_utf8_lossy(&human.stderr).contains("truncated"),
        "expected a truncation note on stderr"
    );
}

#[tokio::test]
async fn project_stats_all_with_limit_reports_truncation() {
    let server = MockServer::start().await;
    mount_items(
        &server,
        vec![item(
            "HAM-1",
            "First",
            "todo",
            "task",
            "high",
            Some("Alice"),
            Some(3),
        )],
        Some("cursor-2"),
    )
    .await;

    let output = call_ok(
        &server,
        &["project", "stats", "HAM", "--json", "--all", "--limit", "1"],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();

    assert_eq!(doc["summary"]["total"], 1);
    assert_eq!(doc["summary"]["truncated"], true);
}

#[tokio::test]
async fn project_stats_clamps_wire_page_size_to_endpoint_maximum() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .and(query_param("limit", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "page": {"limit": 200, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;

    // `--limit` is a total-result cap, not the server page size: the wire
    // request must stay within the documented 200-item endpoint maximum.
    let output = call_ok(
        &server,
        &["project", "stats", "HAM", "--json", "--limit", "500"],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(doc["summary"]["total"], 0);
    assert_eq!(doc["summary"]["truncated"], false);
}
