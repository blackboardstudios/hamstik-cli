// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Shell pipeline ergonomics (CLI-58): `--limit` bounds the total result,
//! `--cursor` / `--since-cursor` resumes a stream from an opaque server cursor,
//! and `--sort KEY[:DIR]` yields an ordering that repeats byte for byte.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const WORK_ITEMS: &str = "/api/v1/organizations/acme/projects/HAM/work-items";
const ORG_WORK_ITEMS: &str = "/api/v1/organizations/acme/work-items";
const COMMENTS: &str = "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments";
const ATTACHMENTS: &str = "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/attachments";

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token-value");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn item(key: &str, priority: &str, due_date: Option<&str>) -> Value {
    json!({
        "id": format!("id-{key}"), "key": key, "projectId": "p1", "title": format!("Title {key}"),
        "type": "task", "status": "todo", "priority": priority,
        "assignee": null, "reporter": {"id": "u1", "name": "Rae"},
        "sprint": null, "parent": null, "labels": [], "storyPoints": null,
        "dueDate": due_date, "archivedAt": null,
        // Identical on purpose: ties must not decide the emitted order.
        "createdAt": "2026-09-01T00:00:00Z", "updatedAt": "2026-09-02T00:00:00Z",
        "revision": 1
    })
}

/// Serves one fixture page per cursor. The empty cursor names the first page,
/// so a fixture set also asserts which cursor the CLI chose to start from.
async fn mount_pages(server: &MockServer, pages: Vec<(&str, Vec<Value>, bool, Option<&str>)>) {
    mount_pages_at(server, WORK_ITEMS, pages).await;
}

async fn mount_pages_at(
    server: &MockServer,
    endpoint: &str,
    pages: Vec<(&str, Vec<Value>, bool, Option<&str>)>,
) {
    let table: Vec<(String, Vec<Value>, bool, Option<String>)> = pages
        .into_iter()
        .map(|(cursor, items, has_more, next)| {
            (
                cursor.to_string(),
                items,
                has_more,
                next.map(str::to_string),
            )
        })
        .collect();
    Mock::given(method("GET"))
        .and(path(endpoint))
        .respond_with(move |request: &Request| {
            let cursor = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "cursor")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            let (_, items, has_more, next) = table
                .iter()
                .find(|(page, _, _, _)| *page == cursor)
                .unwrap_or_else(|| panic!("no fixture page for cursor {cursor:?}"));
            ResponseTemplate::new(200).set_body_json(json!({
                "items": items,
                "page": {"limit": 200, "hasMore": has_more, "nextCursor": next}
            }))
        })
        .mount(server)
        .await;
}

fn query_of(request: &Request, name: &str) -> Option<String> {
    request
        .url
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

fn stdout_json(output: &std::process::Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn ids_of(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn keys(body: &Value) -> Vec<String> {
    body["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["key"].as_str().unwrap_or_default().to_string())
        .collect()
}

async fn run(server: &MockServer, args: &[&str]) -> std::process::Output {
    let dir = TempDir::new().unwrap();
    let output = base(server, &dir)
        .args(["--org", "acme", "--project", "HAM"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn since_cursor_resumes_from_the_checkpoint_instead_of_restarting() {
    let server = MockServer::start().await;
    mount_pages(
        &server,
        vec![
            ("chk", vec![item("HAM-2", "low", None)], true, Some("next")),
            ("next", vec![item("HAM-3", "low", None)], false, None),
        ],
    )
    .await;

    let output = run(
        &server,
        &["work", "list", "--since-cursor", "chk", "--all", "--json"],
    )
    .await;
    assert_eq!(keys(&stdout_json(&output)), vec!["HAM-2", "HAM-3"]);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "one page after the checkpoint each");
    assert_eq!(
        query_of(&requests[0], "cursor").as_deref(),
        Some("chk"),
        "the first request must resume at the checkpoint, not at the first page"
    );
    assert_eq!(query_of(&requests[1], "cursor").as_deref(), Some("next"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_and_since_cursor_name_the_same_resume_point() {
    let server = MockServer::start().await;
    mount_pages(
        &server,
        vec![("chk", vec![item("HAM-2", "low", None)], false, None)],
    )
    .await;

    let aliased = run(
        &server,
        &["work", "list", "--since-cursor", "chk", "--json"],
    )
    .await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(query_of(&requests[0], "cursor").as_deref(), Some("chk"));

    let server = MockServer::start().await;
    mount_pages(
        &server,
        vec![("chk", vec![item("HAM-2", "low", None)], false, None)],
    )
    .await;
    let canonical = run(&server, &["work", "list", "--cursor", "chk", "--json"]).await;

    assert_eq!(
        aliased.stdout, canonical.stdout,
        "--since-cursor must not diverge from --cursor"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cap_cutting_through_a_page_withholds_the_cursor_it_cannot_honor() {
    let server = MockServer::start().await;
    mount_pages(
        &server,
        vec![
            (
                "chk",
                vec![item("HAM-2", "low", None), item("HAM-3", "low", None)],
                true,
                Some("next"),
            ),
            (
                "next",
                vec![item("HAM-4", "low", None), item("HAM-5", "low", None)],
                true,
                Some("more"),
            ),
        ],
    )
    .await;

    let output = run(
        &server,
        &[
            "work",
            "list",
            "--since-cursor",
            "chk",
            "--all",
            "--limit",
            "3",
            "--json",
        ],
    )
    .await;
    let body = stdout_json(&output);
    assert_eq!(keys(&body), vec!["HAM-2", "HAM-3", "HAM-4"]);
    assert!(
        body["page"]["nextCursor"].is_null(),
        "a mid-page cap must not advertise the page-end cursor, which would \
         skip HAM-5's neighbours: {}",
        body["page"]
    );
    assert_eq!(
        body["page"]["hasMore"], true,
        "a capped result must still report that the collection continues"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cap_landing_on_a_page_boundary_keeps_the_resume_cursor() {
    let server = MockServer::start().await;
    mount_pages(
        &server,
        vec![
            (
                "",
                vec![item("HAM-1", "low", None), item("HAM-2", "low", None)],
                true,
                Some("second"),
            ),
            (
                "second",
                vec![item("HAM-3", "low", None), item("HAM-4", "low", None)],
                true,
                Some("third"),
            ),
        ],
    )
    .await;

    let output = run(
        &server,
        &["work", "list", "--all", "--limit", "4", "--json"],
    )
    .await;
    let body = stdout_json(&output);
    assert_eq!(keys(&body).len(), 4);
    assert_eq!(
        body["page"]["nextCursor"], "third",
        "an aligned cap is resumable: the next run continues with no gap"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cap_below_the_page_maximum_needs_one_request() {
    let server = MockServer::start().await;
    mount_pages(
        &server,
        vec![(
            "",
            vec![item("HAM-1", "low", None), item("HAM-2", "low", None)],
            true,
            Some("p2"),
        )],
    )
    .await;

    let output = run(&server, &["work", "list", "--limit", "2", "--json"]).await;
    let body = stdout_json(&output);
    assert_eq!(keys(&body), vec!["HAM-1", "HAM-2"]);
    assert_eq!(body["page"]["nextCursor"], "p2");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(query_of(&requests[0], "limit").as_deref(), Some("2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_oversized_cap_pages_instead_of_being_rejected_by_the_server() {
    let server = MockServer::start().await;
    mount_pages(
        &server,
        vec![(
            "",
            (1..=200)
                .map(|n| item(&format!("HAM-{n}"), "low", None))
                .collect(),
            false,
            None,
        )],
    )
    .await;

    let output = run(&server, &["work", "list", "--limit", "250", "--json"]).await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        query_of(&requests[0], "limit").as_deref(),
        Some("200"),
        "the page size stays inside the documented endpoint maximum"
    );
    let body = stdout_json(&output);
    assert_eq!(
        keys(&body).len(),
        200,
        "a single page may return fewer items than the cap"
    );
}

#[test]
fn a_zero_cap_and_an_empty_cursor_are_usage_errors() {
    let dir = TempDir::new().unwrap();
    for args in [
        vec!["work", "list", "--limit", "0"],
        vec!["work", "list", "--since-cursor", ""],
        vec!["work", "list", "--cursor", ""],
    ] {
        let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
        cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
        cmd.env("HAMSTIK_HOST", "http://127.0.0.1:9");
        cmd.env("HAMSTIK_TOKEN", "secret-token-value");
        cmd.args(["--org", "acme", "--project", "HAM"])
            .args(&args)
            .assert()
            .code(2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sort_direction_orders_the_result_and_repeats_byte_for_byte() {
    let server = MockServer::start().await;
    mount_pages(
        &server,
        vec![(
            "",
            vec![
                item("HAM-1", "low", Some("2026-01-01")),
                item("HAM-2", "urgent", Some("2026-03-01")),
                item("HAM-3", "medium", Some("2026-02-01")),
            ],
            false,
            None,
        )],
    )
    .await;

    let first = run(
        &server,
        &["work", "list", "--sort", "priority:desc", "--json"],
    )
    .await;
    let second = run(
        &server,
        &["work", "list", "--sort", "priority:desc", "--json"],
    )
    .await;
    assert_eq!(
        keys(&stdout_json(&first)),
        vec!["HAM-1", "HAM-3", "HAM-2"],
        "priority:desc must order low to urgent"
    );
    assert_eq!(
        first.stdout, second.stdout,
        "the same query must produce byte-identical output across runs"
    );

    let asc = run(
        &server,
        &["work", "list", "--sort", "priority:asc", "--json"],
    )
    .await;
    assert_eq!(
        keys(&stdout_json(&asc)),
        vec!["HAM-2", "HAM-3", "HAM-1"],
        "priority:asc must order urgent to low"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sort_ties_break_on_the_work_item_key() {
    let server = MockServer::start().await;
    mount_pages(
        &server,
        vec![(
            "",
            vec![
                item("HAM-3", "low", None),
                item("HAM-1", "low", None),
                item("HAM-2", "low", None),
            ],
            false,
            None,
        )],
    )
    .await;

    let output = run(&server, &["work", "list", "--sort", "updated", "--json"]).await;
    assert_eq!(
        keys(&stdout_json(&output)),
        vec!["HAM-1", "HAM-2", "HAM-3"],
        "tied values must not decide the emitted order"
    );

    // The human-readable table lists the same order as --json.
    let dir = TempDir::new().unwrap();
    let table = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "list",
            "--sort",
            "updated",
        ])
        .output()
        .unwrap();
    let table_text = String::from_utf8_lossy(&table.stdout);
    let rows: Vec<&str> = table_text
        .lines()
        .filter(|line| line.contains("HAM-"))
        .map(|line| line.split_whitespace().next().unwrap_or_default())
        .collect();
    assert_eq!(rows, vec!["HAM-1", "HAM-2", "HAM-3"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_work_collection_shares_the_sort_direction() {
    // `org work` builds its query through the same filters, so a direction is
    // honored there too instead of being dropped on the way to the API.
    let server = MockServer::start().await;
    let context = |key: &str, due: &str| {
        json!({
            "id": format!("id-{key}"), "key": key, "revision": 1, "title": format!("T {key}"),
            "status": "todo", "priority": "low", "dueDate": format!("{due}T00:00:00Z"),
            "project": {"id": "p", "key": "HAM", "name": "Ham", "color": "#000000"},
            "organization": {"id": "o", "slug": "acme", "name": "Acme"}
        })
    };
    mount_pages_at(
        &server,
        ORG_WORK_ITEMS,
        vec![(
            "",
            vec![
                context("HAM-3", "2026-01-01"),
                context("HAM-1", "2026-03-01"),
                context("HAM-2", "2026-02-01"),
            ],
            false,
            None,
        )],
    )
    .await;

    let output = run(
        &server,
        &["org", "work", "--sort", "dueDate:desc", "--json"],
    )
    .await;
    assert_eq!(keys(&stdout_json(&output)), vec!["HAM-1", "HAM-2", "HAM-3"]);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        query_of(&requests[0], "sort").as_deref(),
        Some("dueDate"),
        "only the documented key goes on the wire"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn comment_and_attachment_lists_share_the_pipeline_flags() {
    // These two lists were the last commands still treating `--limit` as the
    // transport page size; they now page, resume, and cap like the rest.
    let server = MockServer::start().await;
    let comment = |id: &str| {
        json!({
            "id": id, "workItemId": "w1", "parentCommentId": null,
            "author": {"id": "u1", "name": "Steven"}, "body": format!("body {id}"),
            "deleted": false, "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z", "editedAt": null
        })
    };
    mount_pages_at(
        &server,
        COMMENTS,
        vec![
            ("", vec![comment("c1")], true, Some("c2")),
            ("c2", vec![comment("c2")], true, Some("c3")),
            ("c3", vec![comment("c3")], false, None),
        ],
    )
    .await;

    let output = run(
        &server,
        &[
            "work", "comment", "list", "HAM-1", "--all", "--limit", "2", "--json",
        ],
    )
    .await;
    let body = stdout_json(&output);
    assert_eq!(
        ids_of(&body),
        vec!["c1", "c2"],
        "--limit caps every list command"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "--all walks the cursors");

    // A cap larger than the endpoint page maximum is never sent as-is.
    let server = MockServer::start().await;
    mount_pages_at(
        &server,
        COMMENTS,
        vec![("", vec![comment("c1")], false, None)],
    )
    .await;
    run(
        &server,
        &[
            "work", "comment", "list", "HAM-1", "--limit", "250", "--json",
        ],
    )
    .await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(query_of(&requests[0], "limit").as_deref(), Some("200"));

    // Attachments behave the same way.
    let server = MockServer::start().await;
    let attachment = |id: &str| {
        json!({
            "id": id, "workItemId": "w1", "fileName": format!("{id}.txt"),
            "contentType": "text/plain", "size": 4,
            "createdBy": {"id": "u1", "name": "Steven"},
            "createdAt": "2026-01-01T00:00:00Z"
        })
    };
    mount_pages_at(
        &server,
        ATTACHMENTS,
        vec![
            ("", vec![attachment("a1")], true, Some("a2")),
            ("a2", vec![attachment("a2")], false, None),
        ],
    )
    .await;
    let output = run(
        &server,
        &["work", "attachment", "list", "HAM-1", "--all", "--json"],
    )
    .await;
    assert_eq!(ids_of(&stdout_json(&output)), vec!["a1", "a2"]);
}

#[test]
fn undocumented_sort_keys_and_directions_are_usage_errors() {
    let dir = TempDir::new().unwrap();
    for value in ["owner", "owner:desc", "updated:sideways", "dueDate:"] {
        let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
        cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
        cmd.env("HAMSTIK_HOST", "http://127.0.0.1:9");
        cmd.env("HAMSTIK_TOKEN", "secret-token-value");
        cmd.args(["--org", "acme", "--project", "HAM", "work", "list"])
            .arg("--sort")
            .arg(value)
            .assert()
            .code(2)
            .stderr(predicates::str::contains("sort"));
    }
}
