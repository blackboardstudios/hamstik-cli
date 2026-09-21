// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work tree` contract tests: bounded parent/child traversal, cycle safety,
//! server-reported link annotations, and the stable JSON shape.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WORK_ITEMS: &str = "/api/v1/organizations/acme/projects/HAM/work-items";

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

fn root_item(key: &str, item_type: &str, status: &str) -> Value {
    json!({
        "id": format!("id-{key}"),
        "key": key,
        "projectId": "p1",
        "title": format!("Item {key}"),
        "description": null,
        "type": item_type,
        "status": status,
        "priority": "high",
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
        "revision": 1
    })
}

fn summary(key: &str, item_type: &str, status: &str) -> Value {
    json!({
        "id": format!("id-{key}"),
        "key": key,
        "revision": 1,
        "title": format!("Item {key}"),
        "type": item_type,
        "status": status,
        "priority": "medium"
    })
}

fn page(items: Value) -> Value {
    json!({ "items": items, "page": {"limit": 200, "hasMore": false, "nextCursor": null} })
}

fn page_more(items: Value, next: &str) -> Value {
    json!({ "items": items, "page": {"limit": 200, "hasMore": true, "nextCursor": next} })
}

async fn mount_root(server: &MockServer, key: &str, item_type: &str, status: &str) {
    Mock::given(method("GET"))
        .and(path(format!("{WORK_ITEMS}/{key}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(root_item(key, item_type, status)))
        .mount(server)
        .await;
}

async fn mount_children(server: &MockServer, parent: &str, items: Value) {
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .and(query_param("parent", parent))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(items)))
        .mount(server)
        .await;
}

async fn mount_links(server: &MockServer, key: &str, links: Value) {
    Mock::given(method("GET"))
        .and(path(format!("{WORK_ITEMS}/{key}/links")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": links,
            "page": {"limit": 100, "hasMore": false, "nextCursor": null}
        })))
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

fn common_args<'a>() -> Vec<&'a str> {
    vec![
        "--json",
        "--no-input",
        "--org",
        "acme",
        "--project",
        "HAM",
        "work",
        "tree",
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_tree_is_stable_and_statuses_come_from_the_server() {
    let server = MockServer::start().await;
    mount_root(&server, "HAM-1", "epic", "backlog").await;
    // Deliberately unsorted to prove deterministic key ordering.
    mount_children(
        &server,
        "HAM-1",
        json!([
            summary("HAM-3", "task", "done"),
            summary("HAM-2", "task", "in_progress"),
        ]),
    )
    .await;
    mount_children(&server, "HAM-2", json!([summary("HAM-4", "bug", "todo")])).await;
    mount_children(&server, "HAM-3", json!([])).await;
    mount_children(&server, "HAM-4", json!([])).await;

    let mut args = common_args();
    args.push("HAM-1");
    let stdout = call_ok(&server, &args).await;
    let body: Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(body["treeVersion"], 1);
    assert_eq!(body["organization"], "acme");
    assert_eq!(body["project"], "HAM");
    assert_eq!(body["depth"], 3);
    assert_eq!(body["maxNodes"], 200);
    assert_eq!(body["truncated"], false);
    assert_eq!(body["root"]["key"], "HAM-1");
    assert_eq!(body["root"]["status"], "backlog");
    assert_eq!(body["root"]["parent"], Value::Null);
    assert_eq!(body["root"]["truncated"], json!([]));
    // Children are ordered by key, not server order.
    assert_eq!(body["root"]["children"][0]["key"], "HAM-2");
    assert_eq!(body["root"]["children"][1]["key"], "HAM-3");
    assert_eq!(body["root"]["children"][0]["status"], "in_progress");
    assert_eq!(body["root"]["children"][0]["children"][0]["key"], "HAM-4");
    assert_eq!(body["root"]["children"][0]["children"][0]["type"], "bug");
    // `links` is absent unless requested.
    assert!(body["root"].get("links").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn depth_limit_produces_an_explicit_marker() {
    let server = MockServer::start().await;
    mount_root(&server, "HAM-1", "epic", "backlog").await;
    mount_children(
        &server,
        "HAM-1",
        json!([
            summary("HAM-2", "task", "todo"),
            summary("HAM-3", "task", "todo"),
        ]),
    )
    .await;
    // HAM-2 has a child, HAM-3 does not; the boundary probe distinguishes them.
    mount_children(&server, "HAM-2", json!([summary("HAM-4", "task", "todo")])).await;
    mount_children(&server, "HAM-3", json!([])).await;

    let mut args = common_args();
    args.extend(["--depth", "1", "HAM-1"]);
    let stdout = call_ok(&server, &args).await;
    let body: Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(body["root"]["children"][0]["key"], "HAM-2");
    assert_eq!(
        body["root"]["children"][0]["truncated"][0]["reason"],
        "depth"
    );
    // A leaf at the boundary is not falsely marked as truncated.
    assert_eq!(body["root"]["children"][1]["key"], "HAM-3");
    assert_eq!(body["root"]["children"][1]["truncated"], json!([]));
    assert_eq!(body["truncated"], true);
    // The unexpanded child is not present in the tree.
    assert!(
        body["root"]["children"][0]["children"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cycle_is_detected_and_marked_without_looping() {
    let server = MockServer::start().await;
    mount_root(&server, "HAM-1", "epic", "backlog").await;
    mount_children(&server, "HAM-1", json!([summary("HAM-2", "task", "todo")])).await;
    // Malformed data: the child points back at its ancestor.
    mount_children(
        &server,
        "HAM-2",
        json!([summary("HAM-1", "epic", "backlog")]),
    )
    .await;

    let mut args = common_args();
    args.push("HAM-1");
    let stdout = call_ok(&server, &args).await;
    let body: Value = serde_json::from_str(&stdout).unwrap();

    let cycle = &body["root"]["children"][0]["children"][0];
    assert_eq!(cycle["key"], "HAM-1");
    assert_eq!(cycle["truncated"][0]["reason"], "cycle");
    assert!(cycle["children"].as_array().unwrap().is_empty());
    assert_eq!(body["truncated"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_nodes_caps_growth_with_a_marker() {
    let server = MockServer::start().await;
    mount_root(&server, "HAM-1", "epic", "backlog").await;
    mount_children(
        &server,
        "HAM-1",
        json!([
            summary("HAM-2", "task", "todo"),
            summary("HAM-3", "task", "todo"),
            summary("HAM-4", "task", "todo"),
        ]),
    )
    .await;
    mount_children(&server, "HAM-2", json!([])).await;

    let mut args = common_args();
    args.extend(["--max-nodes", "2", "HAM-1"]);
    let stdout = call_ok(&server, &args).await;
    let body: Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(body["root"]["truncated"][0]["reason"], "nodes");
    assert_eq!(body["root"]["children"].as_array().unwrap().len(), 1);
    assert_eq!(body["truncated"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_node_budget_still_reports_the_cut() {
    let server = MockServer::start().await;
    mount_root(&server, "HAM-1", "epic", "backlog").await;
    mount_children(&server, "HAM-1", json!([summary("HAM-2", "task", "todo")])).await;

    let mut args = common_args();
    args.extend(["--max-nodes", "1", "HAM-1"]);
    let stdout = call_ok(&server, &args).await;
    let body: Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(body["root"]["truncated"][0]["reason"], "nodes");
    assert!(body["root"]["children"].as_array().unwrap().is_empty());
    assert_eq!(body["truncated"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn links_are_server_reported_and_never_computed() {
    let server = MockServer::start().await;
    mount_root(&server, "HAM-1", "story", "in_progress").await;
    mount_children(&server, "HAM-1", json!([summary("HAM-2", "task", "todo")])).await;
    mount_children(&server, "HAM-2", json!([])).await;
    mount_links(
        &server,
        "HAM-1",
        json!([
            {"id": "l1", "relation": "blocks",
             "otherWorkItem": {"id": "9", "key": "HAM-9", "project": {"id": "p1", "key": "HAM", "name": "Ham"},
                               "title": "Blocked thing", "type": "task", "status": "todo"},
             "createdBy": null, "createdAt": "2026-09-01T00:00:00Z"},
            {"id": "l2", "relation": "blocked_by",
             "otherWorkItem": {"id": "7", "key": "HAM-7", "project": {"id": "p1", "key": "HAM", "name": "Ham"},
                               "title": "Blocker", "type": "task", "status": "in_progress"},
             "createdBy": null, "createdAt": "2026-09-01T00:00:00Z"}
        ]),
    )
    .await;
    mount_links(&server, "HAM-2", json!([])).await;

    let mut args = common_args();
    args.extend(["--links", "HAM-1"]);
    let stdout = call_ok(&server, &args).await;
    let body: Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(body["root"]["links"][0]["relation"], "blocks");
    assert_eq!(body["root"]["links"][0]["otherWorkItem"]["key"], "HAM-9");
    assert_eq!(body["root"]["links"][1]["relation"], "blocked_by");
    assert_eq!(body["root"]["children"][0]["links"], json!([]));
    // No client-side readiness/blocking judgment anywhere in the document.
    let text = serde_json::to_string(&body).unwrap();
    for computed in ["\"ready\"", "\"isReady\"", "\"isBlocked\"", "\"blocked\":"] {
        assert!(!text.contains(computed), "computed field found: {computed}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_rendering_uses_ascii_connectors_and_markers() {
    let server = MockServer::start().await;
    mount_root(&server, "HAM-1", "epic", "backlog").await;
    mount_children(
        &server,
        "HAM-1",
        json!([
            summary("HAM-2", "task", "todo"),
            summary("HAM-3", "task", "done"),
        ]),
    )
    .await;
    mount_children(&server, "HAM-2", json!([summary("HAM-4", "bug", "todo")])).await;
    mount_children(&server, "HAM-3", json!([])).await;
    mount_children(&server, "HAM-4", json!([])).await;

    let output = call(
        &server,
        &[
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "tree",
            "HAM-1",
        ],
    )
    .await;
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("HAM-1 Item HAM-1 [epic, backlog, high]"),
        "{text}"
    );
    assert!(
        text.contains("|- HAM-2 Item HAM-2 [task, todo, medium]"),
        "{text}"
    );
    assert!(
        text.contains("|  `- HAM-4 Item HAM-4 [bug, todo, medium]"),
        "{text}"
    );
    assert!(
        text.contains("`- HAM-3 Item HAM-3 [task, done, medium]"),
        "{text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn depth_beyond_the_documented_cap_is_a_usage_error() {
    let server = MockServer::start().await;
    let output = call(
        &server,
        &[
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "tree",
            "--depth",
            "11",
            "HAM-1",
        ],
    )
    .await;
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("11"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn children_pagination_follows_cursors() {
    let server = MockServer::start().await;
    mount_root(&server, "HAM-1", "epic", "backlog").await;
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .and(query_param("parent", "HAM-1"))
        .and(query_param("cursor", "c1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(page(json!([summary("HAM-3", "task", "todo")]))),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .and(query_param("parent", "HAM-1"))
        .and(query_param_is_missing("cursor"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(page_more(json!([summary("HAM-2", "task", "todo")]), "c1")),
        )
        .mount(&server)
        .await;
    mount_children(&server, "HAM-2", json!([])).await;
    mount_children(&server, "HAM-3", json!([])).await;

    let mut args = common_args();
    args.push("HAM-1");
    let stdout = call_ok(&server, &args).await;
    let body: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(body["root"]["children"][0]["key"], "HAM-2");
    assert_eq!(body["root"]["children"][1]["key"], "HAM-3");
}
