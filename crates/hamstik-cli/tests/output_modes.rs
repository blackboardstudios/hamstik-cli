// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Line-oriented and table output modes (CLI-9): `--jsonl`, `--tsv`,
//! `--columns`, `--no-header`, and the embedded `--jq` filter.
//!
//! These tests are the golden contract for the modes: exact stdout bytes where
//! the format is byte-defined (TSV, jq JSONL), parsed equality where key order
//! is incidental (JSON), plus the startup-only conflict rejections that must
//! never reach the network.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const WORK_ITEMS: &str = "/api/v1/organizations/acme/projects/HAM/work-items";
const WORK_ITEM: &str = "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1";

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token-value");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn item(key: &str, title: &str) -> Value {
    json!({
        "id": format!("id-{key}"), "key": key, "projectId": "p1", "title": title,
        "description": null, "type": "task", "status": "todo", "priority": "low",
        "assignee": null, "reporter": {"id": "u1", "name": "Rae"},
        "sprint": null, "parent": null, "labels": [], "storyPoints": null,
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

async fn mount_item(server: &MockServer, value: Value) {
    Mock::given(method("GET"))
        .and(path(WORK_ITEM))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
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

async fn call_fails(server: &MockServer, args: &[&str]) -> (i32, String) {
    let output = call(server, args).await;
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

async fn request_count(server: &MockServer) -> usize {
    server.received_requests().await.unwrap().len()
}

fn lines_of(text: &str) -> Vec<&str> {
    text.lines().collect()
}

// ---------------------------------------------------------------------------
// JSON Lines
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jsonl_emits_each_resource_as_one_line_with_no_envelope() {
    let server = MockServer::start().await;
    mount_items(
        &server,
        vec![item("HAM-1", "First"), item("HAM-2", "Second")],
        None,
    )
    .await;

    let stdout = call_ok(&server, &["work", "list", "--jsonl"]).await;
    let lines = lines_of(&stdout);
    assert_eq!(lines.len(), 2, "one line per resource: {stdout:?}");
    assert!(!stdout.contains("\"items\""), "envelope leaked: {stdout}");
    assert!(!stdout.contains("\"page\""), "pagination leaked: {stdout}");

    // Each line is independently valid JSON and keeps server field names and
    // JSON types (no human table cells, no stringified numbers).
    let first: Value = serde_json::from_str(lines[0]).unwrap();
    let second: Value = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(first["key"], json!("HAM-1"));
    assert_eq!(first["revision"], json!(1));
    assert_eq!(first["labels"], json!([]));
    assert_eq!(second["key"], json!("HAM-2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jsonl_streams_every_page_in_api_order() {
    let server = MockServer::start().await;
    // Fixtures are served by cursor, so this also proves `--all` walks pages in
    // server order and never re-sorts them.
    let table: Vec<(String, Vec<Value>, Option<String>)> = vec![
        (
            String::new(),
            vec![item("HAM-1", "First")],
            Some("p2".to_string()),
        ),
        ("p2".to_string(), vec![item("HAM-2", "Second")], None),
    ];
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .respond_with(move |request: &Request| {
            let cursor = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "cursor")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            let (_, items, next) = table
                .iter()
                .find(|(page, _, _)| *page == cursor)
                .unwrap_or_else(|| panic!("no fixture page for cursor {cursor:?}"));
            ResponseTemplate::new(200).set_body_json(json!({
                "items": items,
                "page": {"limit": 200, "hasMore": next.is_some(), "nextCursor": next}
            }))
        })
        .mount(&server)
        .await;

    let stdout = call_ok(&server, &["work", "list", "--all", "--jsonl"]).await;
    let keys: Vec<String> = lines_of(&stdout)
        .into_iter()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["key"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(keys, vec!["HAM-1", "HAM-2"]);
    assert_eq!(request_count(&server).await, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jsonl_preserves_a_sparse_projection() {
    let server = MockServer::start().await;
    let sparse = json!({"id": "id-1", "key": "HAM-1", "title": "Sparse", "revision": 4});
    Mock::given(method("GET"))
        .and(path(WORK_ITEMS))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [sparse],
            "page": {"limit": 200, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;

    let stdout = call_ok(
        &server,
        &["work", "list", "--fields", "key,title", "--jsonl"],
    )
    .await;
    let requests = server.received_requests().await.unwrap();
    let query = requests[0].url.query().unwrap_or_default().to_string();
    assert!(
        query.contains("fields=key") && query.contains("title"),
        "the sparse fieldset must be forwarded verbatim: {query}"
    );
    let lines = lines_of(&stdout);
    assert_eq!(lines.len(), 1);
    let value: Value = serde_json::from_str(lines[0]).unwrap();
    // Absent optional projections stay absent; identity fields survive.
    let object = value.as_object().unwrap();
    assert_eq!(
        object.keys().cloned().collect::<Vec<_>>(),
        ["id", "key", "revision", "title"]
    );
    assert_eq!(object["revision"], json!(4));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jsonl_and_tsv_emit_a_single_resource_as_one_machine_line() {
    let server = MockServer::start().await;
    mount_item(&server, item("HAM-1", "First")).await;

    for flag in ["--jsonl", "--tsv"] {
        let stdout = call_ok(&server, &["work", "view", "HAM-1", flag]).await;
        let lines = lines_of(&stdout);
        assert_eq!(
            lines.len(),
            1,
            "{flag} must emit exactly one line: {stdout:?}"
        );
        let value: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(value["key"], json!("HAM-1"));
    }
}

// ---------------------------------------------------------------------------
// TSV
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tsv_emits_documented_column_order_then_rows() {
    let server = MockServer::start().await;
    mount_items(
        &server,
        vec![item("HAM-1", "First"), item("HAM-2", "Second")],
        None,
    )
    .await;

    let stdout = call_ok(&server, &["work", "list", "--tsv"]).await;
    assert_eq!(
        stdout,
        concat!(
            "KEY\tTITLE\tSTATUS\tTYPE\tPRIORITY\tASSIGNEE\n",
            "HAM-1\tFirst\ttodo\ttask\tlow\t-\n",
            "HAM-2\tSecond\ttodo\ttask\tlow\t-\n"
        )
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tsv_escapes_every_cell_breaking_character() {
    let server = MockServer::start().await;
    let nasty = "tab\there\nnewline\r\nquote\"back\\slash\u{7}unicode\u{2713}\u{6f22}\u{5b57}";
    mount_items(&server, vec![item("HAM-1", nasty)], None).await;

    let stdout = call_ok(&server, &["work", "list", "--tsv", "--no-header"]).await;
    let lines = lines_of(&stdout);
    assert_eq!(
        lines.len(),
        1,
        "a cell broke onto multiple lines: {stdout:?}"
    );
    assert_eq!(
        lines[0],
        concat!(
            "HAM-1\ttab\\there\\nnewline\\r\\nquote\"back\\\\slash",
            "\u{fffd}unicode\u{2713}\u{6f22}\u{5b57}\ttodo\ttask\tlow\t-"
        )
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tsv_never_emits_terminal_escape_sequences() {
    let server = MockServer::start().await;
    let swatch = "red\u{1b}[31mSWATCH\u{1b}[0m".to_string();
    mount_items(&server, vec![item("HAM-1", &swatch)], None).await;

    let stdout = call_ok(&server, &["work", "list", "--tsv"]).await;
    assert!(
        !stdout.contains('\u{1b}'),
        "machine output carried an escape: {stdout:?}"
    );
    assert!(stdout.contains("SWATCH"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn columns_select_and_reorder_and_no_header_suppresses_the_header() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "First")], None).await;

    let stdout = call_ok(
        &server,
        &["work", "list", "--tsv", "--columns", "status", "key"],
    )
    .await;
    assert_eq!(stdout, "STATUS\tKEY\ntodo\tHAM-1\n");

    let stdout = call_ok(
        &server,
        &[
            "work",
            "list",
            "--tsv",
            "--columns",
            "key",
            "status",
            "--no-header",
        ],
    )
    .await;
    assert_eq!(stdout, "HAM-1\ttodo\n");

    // Human tables honour the same projection and header suppression.
    let stdout = call_ok(
        &server,
        &["work", "list", "--columns", "KEY", "--no-header"],
    )
    .await;
    let lines = lines_of(&stdout);
    assert_eq!(lines.len(), 1, "no header, one row: {stdout:?}");
    assert!(lines[0].starts_with("HAM-1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_column_is_rejected_with_the_available_names() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "First")], None).await;

    for args in [
        vec!["work", "list", "--tsv", "--columns", "nope"],
        vec!["work", "list", "--columns", "nope"],
    ] {
        let (code, stderr) = call_fails(&server, &args).await;
        assert_eq!(code, 2, "unknown column must be a usage error: {stderr}");
        assert!(stderr.contains("unknown column"), "{stderr}");
        assert!(stderr.contains("TITLE"), "must list columns: {stderr}");
    }

    // A comma-typed name gets the separator hint, not just the name list.
    let (code, stderr) = call_fails(
        &server,
        &["work", "list", "--tsv", "--columns", "key,title"],
    )
    .await;
    assert_eq!(code, 2);
    assert!(stderr.contains("repeat --columns"), "{stderr}");
}

// ---------------------------------------------------------------------------
// --jq
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jq_filters_the_envelope_in_every_structured_mode() {
    let server = MockServer::start().await;
    mount_items(
        &server,
        vec![item("HAM-1", "First"), item("HAM-2", "Second")],
        None,
    )
    .await;

    // --json coalesces multiple results into one valid JSON document.
    let stdout = call_ok(&server, &["work", "list", "--json", "--jq", ".items[].key"]).await;
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap(),
        json!(["HAM-1", "HAM-2"])
    );
    let stdout = call_ok(
        &server,
        &["work", "list", "--json", "--jq", ".items | length"],
    )
    .await;
    assert_eq!(serde_json::from_str::<Value>(&stdout).unwrap(), json!(2));
    // Zero results still leave one valid JSON document on stdout.
    let stdout = call_ok(
        &server,
        &[
            "work",
            "list",
            "--json",
            "--jq",
            ".items[] | select(.key == \"NOPE\")",
        ],
    )
    .await;
    assert_eq!(serde_json::from_str::<Value>(&stdout).unwrap(), Value::Null);

    // --jsonl emits one result per line.
    let stdout = call_ok(
        &server,
        &["work", "list", "--jsonl", "--jq", ".items[].key"],
    )
    .await;
    assert_eq!(stdout, "\"HAM-1\"\n\"HAM-2\"\n");
    let stdout = call_ok(
        &server,
        &["work", "list", "--jsonl", "--jq", ".page.hasMore"],
    )
    .await;
    assert_eq!(stdout, "false\n");

    // --tsv turns each array result into cells of one row.
    let stdout = call_ok(
        &server,
        &[
            "work",
            "list",
            "--tsv",
            "--jq",
            ".items[] | [.key, .status, .storyPoints]",
        ],
    )
    .await;
    assert_eq!(stdout, "HAM-1\ttodo\t\nHAM-2\ttodo\t\n");
    assert!(stdout.lines().all(|line| line.split('\t').count() == 3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jq_applies_to_single_resource_output_too() {
    let server = MockServer::start().await;
    mount_item(&server, item("HAM-1", "First")).await;

    let stdout = call_ok(
        &server,
        &["work", "view", "HAM-1", "--json", "--jq", ".key"],
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(&stdout).unwrap(),
        json!("HAM-1")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_jq_expression_fails_before_any_request() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "First")], None).await;

    let output = call(&server, &["work", "list", "--json", "--jq", ".[oops"]).await;
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(stderr.contains("--jq filter error"), "stderr: {stderr}");
    assert_eq!(
        request_count(&server).await,
        0,
        "a bad filter must not hit the API"
    );
}

// ---------------------------------------------------------------------------
// Mode conflicts (rejected before any network call)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_mode_conflicts_fail_before_any_request() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "First")], None).await;

    let cases: Vec<Vec<&str>> = vec![
        vec!["--jsonl", "--quiet", "work", "list"],
        vec!["--tsv", "--quiet", "work", "list"],
        vec!["--jsonl", "--tsv", "work", "list"],
        vec!["--json", "--jsonl", "work", "list"],
        vec!["--jq", ".items", "work", "list"],
        vec!["--json", "--columns", "KEY", "work", "list"],
        vec!["--jsonl", "--no-header", "work", "list"],
        vec!["--json", "--jq", "--columns", "KEY", "work", "list"],
    ];
    for args in cases {
        let output = call(&server, &args).await;
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?} must be a usage error, stdout: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(output.stdout.is_empty(), "{args:?} wrote stdout");
    }
    assert_eq!(
        request_count(&server).await,
        0,
        "conflicts must not hit the API"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn structured_modes_keep_stdout_machine_only() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "First")], Some("p2")).await;

    let stdout = call_ok(&server, &["work", "list", "--jsonl", "--verbose"]).await;
    let lines = lines_of(&stdout);
    assert_eq!(lines.len(), 1);
    let value: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(value["key"], json!("HAM-1"));
}

// ---------------------------------------------------------------------------
// --format umbrella (CLI-59)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_markdown_renders_a_github_flavored_table() {
    let server = MockServer::start().await;
    mount_items(
        &server,
        vec![item("HAM-1", "First"), item("HAM-2", "Second")],
        None,
    )
    .await;

    let stdout = call_ok(&server, &["work", "list", "--format", "markdown"]).await;
    assert_eq!(
        stdout,
        concat!(
            "| KEY | TITLE | STATUS | TYPE | PRIORITY | ASSIGNEE |\n",
            "| --- | --- | --- | --- | --- | --- |\n",
            "| HAM-1 | First | todo | task | low | - |\n",
            "| HAM-2 | Second | todo | task | low | - |\n"
        )
    );

    // The projection applies to Markdown like any other table mode.
    let stdout = call_ok(
        &server,
        &[
            "work",
            "list",
            "--format",
            "markdown",
            "--columns",
            "key",
            "title",
        ],
    )
    .await;
    assert_eq!(
        stdout,
        concat!(
            "| KEY | TITLE |\n",
            "| --- | --- |\n",
            "| HAM-1 | First |\n",
            "| HAM-2 | Second |\n"
        )
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_markdown_escapes_pipes_and_newlines() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "pipe | break\nnext")], None).await;

    let stdout = call_ok(
        &server,
        &["work", "list", "--format", "markdown", "--columns", "title"],
    )
    .await;
    assert!(stdout.contains("| pipe \\| break<br>next |"), "{stdout:?}");
    assert_eq!(stdout.lines().count(), 3, "one table row: {stdout:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_csv_quotes_cells_per_rfc4180() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "comma, and \"quote\"")], None).await;

    let stdout = call_ok(
        &server,
        &[
            "work",
            "list",
            "--format",
            "csv",
            "--columns",
            "key",
            "title",
        ],
    )
    .await;
    assert_eq!(
        stdout,
        concat!("KEY,TITLE\n", "HAM-1,\"comma, and \"\"quote\"\"\"\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_aliases_map_onto_the_dedicated_modes() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "First")], None).await;

    // ndjson is jsonl.
    let jsonl = call_ok(&server, &["work", "list", "--jsonl"]).await;
    let ndjson = call_ok(&server, &["work", "list", "--format", "ndjson"]).await;
    assert_eq!(ndjson, jsonl);
    let jsonl = call_ok(&server, &["work", "list", "--format", "jsonl"]).await;
    assert_eq!(ndjson, jsonl);

    // tsv is the tab-separated table.
    let tsv = call_ok(&server, &["work", "list", "--tsv"]).await;
    let formatted = call_ok(&server, &["work", "list", "--format", "tsv"]).await;
    assert_eq!(formatted, tsv);

    // table is the aligned human table.
    let human = call_ok(&server, &["work", "list"]).await;
    let table = call_ok(&server, &["work", "list", "--format", "table"]).await;
    assert_eq!(table, human);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_does_not_change_json_output() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "First")], None).await;

    let stdout = call_ok(&server, &["work", "list", "--json"]).await;
    let body: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(body["items"][0]["key"], json!("HAM-1"));
    assert!(body["page"].is_object(), "pagination stays in --json");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_conflicts_and_invalid_values_fail_before_any_request() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "First")], None).await;

    for args in [
        vec!["work", "list", "--format", "markdown", "--json"],
        vec!["work", "list", "--format", "csv", "--tsv"],
        vec!["work", "list", "--format", "table", "--quiet"],
        vec!["work", "list", "--format", "bogus"],
    ] {
        let (code, _) = call_fails(&server, &args).await;
        assert_eq!(code, 2, "{args:?} must be a usage error");
    }
    assert_eq!(
        request_count(&server).await,
        0,
        "format conflicts must not hit the API"
    );
}

// ---------------------------------------------------------------------------
// --fields projection (CLI-59)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_work_item_sparse_field_is_rejected_with_valid_names() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "First")], None).await;

    let (code, stderr) = call_fails(&server, &["work", "list", "--fields", "nope"]).await;
    assert_eq!(code, 2, "unknown field must be a usage error: {stderr}");
    assert!(stderr.contains("unknown field"), "{stderr}");
    assert!(stderr.contains("title"), "must list valid fields: {stderr}");
    assert_eq!(
        request_count(&server).await,
        0,
        "an unknown field must not hit the API"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_single_resource_field_is_rejected_with_valid_names() {
    let server = MockServer::start().await;
    mount_item(&server, item("HAM-1", "First")).await;

    let (code, stderr) = call_fails(&server, &["work", "view", "HAM-1", "--fields", "nope"]).await;
    assert_eq!(code, 2, "unknown field must be a usage error: {stderr}");
    assert!(stderr.contains("unknown field"), "{stderr}");
    assert!(stderr.contains("title"), "must list valid fields: {stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_csv_projects_and_orders_columns() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "First")], None).await;

    let stdout = call_ok(
        &server,
        &[
            "work",
            "list",
            "--format",
            "csv",
            "--columns",
            "status",
            "key",
        ],
    )
    .await;
    assert_eq!(stdout, "STATUS,KEY\ntodo,HAM-1\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn format_markdown_combines_with_fields_projection() {
    let server = MockServer::start().await;
    mount_items(&server, vec![item("HAM-1", "First")], None).await;

    // On the Work Item list commands `--fields` is both the server-side sparse
    // fieldset and the table projection, so a markdown paste stays sparse.
    let stdout = call_ok(
        &server,
        &[
            "work",
            "list",
            "--format",
            "markdown",
            "--fields",
            "key,title",
        ],
    )
    .await;
    assert_eq!(
        stdout,
        concat!(
            "| KEY | TITLE |\n",
            "| --- | --- |\n",
            "| HAM-1 | First |\n"
        )
    );
    let request = &server.received_requests().await.unwrap()[0];
    let query = request.url.query().unwrap_or_default().to_string();
    assert!(
        query.contains("fields=key") && query.contains("title"),
        "the sparse fieldset must still reach the server: {query}"
    );
}
