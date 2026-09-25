// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work watch` contract tests: incremental activity/comment polling, the
//! JSON event stream, argument bounds, reconnect reporting, and the
//! distinction from `work await`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const ACTIVITY: &str = "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/activity";
const COMMENTS: &str = "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments";

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

fn page(items: Value) -> Value {
    json!({"items": items, "page": {"limit": 200, "hasMore": false, "nextCursor": null}})
}

fn auth_error() -> ResponseTemplate {
    ResponseTemplate::new(401).set_body_json(json!({
        "error": {"code": "AUTH_REQUIRED", "message": "token expired"},
        "requestId": "req-auth"
    }))
}

fn activity_event(id: &str, created_at: &str) -> Value {
    json!({
        "id": id,
        "action": "status_changed",
        "actor": {"publicId": "usr_x", "name": "Ada"},
        "detail": {"from": "todo", "to": "done"},
        "createdAt": created_at
    })
}

fn comment(id: &str, created_at: &str) -> Value {
    json!({
        "id": id,
        "workItemId": "w1",
        "parentCommentId": null,
        "author": {"publicId": "usr_x", "name": "Ada"},
        "body": "hello world",
        "deleted": false,
        "createdAt": created_at,
        "updatedAt": created_at,
        "editedAt": null,
        "mentions": []
    })
}

/// A `--json` watch emits one JSON object per line, only once per event, and
/// stops cleanly on an authentication failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_watch_streams_new_activity_as_json_lines() {
    let server = MockServer::start().await;
    let activity_calls = Arc::new(AtomicUsize::new(0));
    let comments_calls = Arc::new(AtomicUsize::new(0));

    let seen_activity = activity_calls.clone();
    Mock::given(method("GET"))
        .and(path(ACTIVITY))
        .respond_with(move |_request: &Request| {
            let call = seen_activity.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                ResponseTemplate::new(200).set_body_json(page(json!([])))
            } else {
                ResponseTemplate::new(200)
                    .set_body_json(page(json!([activity_event("a1", "2026-01-02T00:00:00Z")])))
            }
        })
        .mount(&server)
        .await;

    let seen_comments = comments_calls.clone();
    Mock::given(method("GET"))
        .and(path(COMMENTS))
        .respond_with(move |_request: &Request| {
            let call = seen_comments.fetch_add(1, Ordering::SeqCst);
            if call >= 2 {
                auth_error()
            } else {
                ResponseTemplate::new(200).set_body_json(page(json!([])))
            }
        })
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "watch",
            "HAM-1",
            "--since",
            "2020-01-01T00:00:00Z",
            "--interval",
            "100ms",
            "--json",
        ])
        .output()
        .unwrap();

    // The auth failure stops the watch with the authentication exit code.
    assert_eq!(output.status.code(), Some(3));

    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "exactly one new event must be streamed");
    let event: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(event["type"], "activity");
    assert_eq!(event["item"], "HAM-1");
    assert_eq!(event["activity"]["id"], "a1");
    assert_eq!(event["activity"]["action"], "status_changed");
}

/// A newly created comment is streamed even though the comments endpoint has
/// no `since` filter; the second read (one poll interval later) surfaces it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_watch_streams_new_comments() {
    let server = MockServer::start().await;
    let comments_calls = Arc::new(AtomicUsize::new(0));

    Mock::given(method("GET"))
        .and(path(ACTIVITY))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;

    let seen_comments = comments_calls.clone();
    Mock::given(method("GET"))
        .and(path(COMMENTS))
        .respond_with(move |_request: &Request| {
            let call = seen_comments.fetch_add(1, Ordering::SeqCst);
            match call {
                0 => ResponseTemplate::new(200).set_body_json(page(json!([]))),
                1 => ResponseTemplate::new(200)
                    .set_body_json(page(json!([comment("c1", "2026-01-02T00:00:00Z")]))),
                _ => auth_error(),
            }
        })
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "watch",
            "HAM-1",
            "--since",
            "2020-01-01T00:00:00Z",
            "--interval",
            "100ms",
            "--json",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
    let event: Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert_eq!(event["type"], "comment");
    assert_eq!(event["comment"]["id"], "c1");
    assert_eq!(event["comment"]["body"], "hello world");
}

/// Without `--since`, the watch starts at the current instant: pre-existing
/// comments are not replayed as if they were new.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_watch_default_start_skips_history() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path(ACTIVITY))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;

    let comments_calls = Arc::new(AtomicUsize::new(0));
    let seen_comments = comments_calls.clone();
    Mock::given(method("GET"))
        .and(path(COMMENTS))
        .respond_with(move |_request: &Request| {
            let call = seen_comments.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                ResponseTemplate::new(200)
                    .set_body_json(page(json!([comment("old", "2020-01-01T00:00:00Z")])))
            } else {
                auth_error()
            }
        })
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "watch",
            "HAM-1",
            "--interval",
            "100ms",
            "--json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(3));
    assert!(
        output.stdout.is_empty(),
        "historical comments must not be replayed by default"
    );
}

/// A 5xx response backs off, is reported on stderr, and reconnects instead of
/// terminating the watch.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_watch_backs_off_and_reports_transient_failure() {
    let server = MockServer::start().await;
    let activity_calls = Arc::new(AtomicUsize::new(0));

    let seen_activity = activity_calls.clone();
    Mock::given(method("GET"))
        .and(path(ACTIVITY))
        .respond_with(move |_request: &Request| {
            let call = seen_activity.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                ResponseTemplate::new(500).set_body_json(json!({
                    "error": {"code": "INTERNAL_ERROR", "message": "boom"},
                    "requestId": "req-1"
                }))
            } else {
                ResponseTemplate::new(200).set_body_json(page(json!([])))
            }
        })
        .mount(&server)
        .await;

    // Stop the watch after the transient failure has been reported.
    Mock::given(method("GET"))
        .and(path(COMMENTS))
        .respond_with(auth_error())
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "watch",
            "HAM-1",
            "--interval",
            "100ms",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("retrying in"));
}

/// `--help` distinguishes the streaming watch from the one-shot await.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_watch_help_distinguishes_from_await() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();

    base(&server, &dir)
        .args(["work", "watch", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("work await"));

    base(&server, &dir)
        .args(["work", "await", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("work watch"));
}

/// An interval outside the documented bounds is a usage error and sends no
/// request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_watch_rejects_out_of_range_interval() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();

    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "watch",
            "HAM-1",
            "--interval",
            "10ms",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("lower bound"));

    assert!(server.received_requests().await.unwrap().is_empty());
}

/// The `--notify` hook receives the event through `HAMSTIK_WATCH_*` variables.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_watch_notify_hook_receives_event_environment() {
    let server = MockServer::start().await;
    let activity_calls = Arc::new(AtomicUsize::new(0));

    let seen_activity = activity_calls.clone();
    Mock::given(method("GET"))
        .and(path(ACTIVITY))
        .respond_with(move |_request: &Request| {
            let call = seen_activity.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                ResponseTemplate::new(200).set_body_json(page(json!([])))
            } else {
                ResponseTemplate::new(200)
                    .set_body_json(page(json!([activity_event("a1", "2026-01-02T00:00:00Z")])))
            }
        })
        .mount(&server)
        .await;

    let comments_calls = Arc::new(AtomicUsize::new(0));
    let seen_comments = comments_calls.clone();
    Mock::given(method("GET"))
        .and(path(COMMENTS))
        .respond_with(move |_request: &Request| {
            let call = seen_comments.fetch_add(1, Ordering::SeqCst);
            if call >= 2 {
                auth_error()
            } else {
                ResponseTemplate::new(200).set_body_json(page(json!([])))
            }
        })
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let marker = dir.path().join("notify.txt");
    let notify = format!(
        "printf '%s|%s' \"$HAMSTIK_WATCH_ITEM\" \"$HAMSTIK_WATCH_ACTION\" > {}",
        marker.display()
    );
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "watch",
            "HAM-1",
            "--since",
            "2020-01-01T00:00:00Z",
            "--interval",
            "100ms",
            "--notify",
            &notify,
        ])
        .assert()
        .code(3);

    let recorded = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(recorded, "HAM-1|status_changed");
}
