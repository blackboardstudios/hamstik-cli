// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik board view` end-to-end tests: column derivation from
//! server-reported statuses, the explicit missing-status bucket, the single
//! existing Work Item list read, sprint scoping, pagination, and human/quiet
//! rendering.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

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

fn item(key: &str, title: &str, status: Option<&str>, points: Option<i64>) -> Value {
    json!({
        "id": format!("id-{key}"),
        "key": key,
        "title": title,
        "revision": 1,
        "type": "task",
        "status": status,
        "priority": "high",
        "assignee": {"id": "u1", "name": "Alice"},
        "sprint": null,
        "parentId": null,
        "storyPoints": points,
        "dueDate": null,
        "archivedAt": null,
        "createdAt": "2026-09-01T00:00:00Z",
        "updatedAt": "2026-09-02T00:00:00Z"
    })
}

fn page(items: Value, has_more: bool, next_cursor: Option<&str>) -> Value {
    json!({
        "items": items,
        "page": {
            "limit": 50,
            "hasMore": has_more,
            "nextCursor": next_cursor
        }
    })
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

/// Acceptance criterion: columns derive from the statuses the server reports,
/// and items with a missing status render in an explicit bucket. The only
/// network call is the existing Work Item list read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_derives_columns_and_buckets_missing_statuses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(
            json!([
                item("HAM-1", "First", Some("done"), Some(5)),
                item("HAM-2", "Second", Some("todo"), Some(2)),
                item("HAM-3", "Third", None, None),
                item("HAM-4", "Fourth", Some(""), None),
                item("HAM-5", "Fifth", Some("todo"), None),
                item("HAM-6", "Sixth", Some("blocked"), Some(8)),
            ]),
            false,
            None,
        )))
        .mount(&server)
        .await;

    let output = call_ok(
        &server,
        &[
            "--org",
            "acme",
            "board",
            "view",
            "--project",
            "HAM",
            "--json",
        ],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();

    assert_eq!(doc["boardVersion"], 1);
    assert_eq!(doc["scope"]["kind"], "project");
    assert_eq!(doc["scope"]["project"], "HAM");
    assert_eq!(doc["scope"]["sprint"], Value::Null);
    assert_eq!(doc["itemsFetched"], 6);
    assert_eq!(doc["truncated"], false);

    // Canonical order for known statuses, then unknown statuses, then the
    // explicit missing bucket (null and blank both count as missing).
    let columns = doc["columns"].as_array().unwrap();
    let statuses: Vec<Option<&str>> = columns
        .iter()
        .map(|column| column["status"].as_str())
        .collect();
    assert_eq!(
        statuses,
        vec![Some("todo"), Some("done"), Some("blocked"), None]
    );
    assert_eq!(columns[0]["count"], 2);
    assert_eq!(columns[0]["storyPoints"], 2);
    assert_eq!(columns[1]["count"], 1);
    assert_eq!(columns[1]["storyPoints"], 5);
    assert_eq!(columns[2]["count"], 1);
    assert_eq!(columns[3]["count"], 2, "null and blank are both missing");

    // No network calls beyond the existing list endpoint.
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].url.path().ends_with("/work-items"));
}

/// A Sprint board scopes the Work Item list read with `sprint=<id>` and never
/// calls a Sprint resource endpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sprint_scope_filters_the_single_list_read() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .and(query_param("sprint", "sprint-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(
            json!([item("HAM-9", "Sprint work", Some("in_progress"), Some(3))]),
            false,
            None,
        )))
        .mount(&server)
        .await;

    let output = call_ok(
        &server,
        &[
            "--org",
            "acme",
            "--project",
            "HAM",
            "board",
            "view",
            "--sprint",
            "sprint-1",
            "--json",
        ],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(doc["scope"]["kind"], "sprint");
    assert_eq!(doc["scope"]["sprint"], "sprint-1");
    assert_eq!(doc["columns"][0]["status"], "in_progress");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].url.path().ends_with("/work-items"));
    assert!(!requests[0].url.path().contains("/sprints"));
}

/// `--all` follows the server cursor; `--fields` is forwarded as the sparse
/// fieldset.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_follows_cursor_pages() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .respond_with(move |request: &Request| {
            let cursor = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "cursor")
                .map(|(_, value)| value.into_owned());
            match cursor.as_deref() {
                None => ResponseTemplate::new(200).set_body_json(page(
                    json!([item("HAM-1", "Page one", Some("todo"), None)]),
                    true,
                    Some("page-two"),
                )),
                Some("page-two") => ResponseTemplate::new(200).set_body_json(page(
                    json!([item("HAM-2", "Page two", Some("done"), None)]),
                    false,
                    None,
                )),
                Some(other) => panic!("unexpected cursor {other}"),
            }
        })
        .mount(&server)
        .await;

    let output = call_ok(
        &server,
        &[
            "--org",
            "acme",
            "board",
            "view",
            "--project",
            "HAM",
            "--all",
            "--json",
        ],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(doc["itemsFetched"], 2);
    assert_eq!(doc["truncated"], false);
    let columns = doc["columns"].as_array().unwrap();
    assert_eq!(columns.len(), 2);
}

/// `--fields` reaches the server as the documented sparse fieldset; a sparse
/// result with no `status` still renders in the missing bucket.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fields_is_forwarded_to_the_list_read() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .and(query_param("fields", "key,title"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(
            json!([{"id": "id-HAM-1", "key": "HAM-1", "title": "Sparse", "revision": 1}]),
            false,
            None,
        )))
        .mount(&server)
        .await;

    let output = call_ok(
        &server,
        &[
            "--org",
            "acme",
            "board",
            "view",
            "--project",
            "HAM",
            "--fields",
            "key,title",
            "--json",
        ],
    )
    .await;
    let doc: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(doc["columns"][0]["status"], Value::Null);
    assert_eq!(doc["columns"][0]["items"][0]["key"], "HAM-1");
}

/// Unknown `--fields` names fail as a local usage error (exit 2) before any
/// request is sent, matching the documented global `--fields` behavior on
/// Work Item list reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_fields_fail_as_usage_without_a_request() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(
            json!([item("HAM-1", "First", Some("todo"), None)]),
            false,
            None,
        )))
        .mount(&server)
        .await;

    let output = call(
        &server,
        &[
            "--org",
            "acme",
            "board",
            "view",
            "--project",
            "HAM",
            "--fields",
            "key,nonsense",
            "--json",
        ],
    )
    .await;
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("nonsense"), "{stderr}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// Human rendering is width-aware and names the missing-status bucket; quiet
/// mode emits only keys.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn human_and_quiet_render_the_board() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(
            json!([
                item(
                    "HAM-1",
                    "A deliberately long title that must wrap inside a narrow column",
                    Some("todo"),
                    Some(2)
                ),
                item("HAM-2", "No status here", None, None),
            ]),
            false,
            None,
        )))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .env("COLUMNS", "44")
        .args(["--org", "acme", "board", "view", "--project", "HAM"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let human = String::from_utf8(output.stdout).unwrap();
    assert!(human.contains("Board — Project HAM"), "{human}");
    assert!(human.contains("(no status)"), "{human}");
    assert!(human.contains("HAM-1"), "{human}");
    assert!(human.contains("HAM-2"), "{human}");
    // No rendered line may exceed the requested terminal width.
    for line in human.lines() {
        assert!(
            hamstik_cli::output::visible_width(line) <= 44,
            "line over width: {line:?}"
        );
    }

    let quiet = call_ok(
        &server,
        &[
            "--org",
            "acme",
            "board",
            "view",
            "--project",
            "HAM",
            "--quiet",
        ],
    )
    .await;
    assert_eq!(quiet.lines().collect::<Vec<_>>(), vec!["HAM-1", "HAM-2"]);
}
