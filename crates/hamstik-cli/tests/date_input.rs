// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Human-friendly date/time input (CLI-24): convenience expressions resolve to
//! one documented absolute instant, the host time zone (including its DST
//! rules) decides where a timezone-less value lands, `--verbose` reports the
//! resolved value, and an invalid expression fails locally — before any request.

use std::time::{SystemTime, UNIX_EPOCH};

use assert_cmd::Command;
use chrono::{Duration, Local, NaiveDate, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn empty_list() -> Value {
    json!({ "items": [], "page": { "limit": 50, "hasMore": false, "nextCursor": null } })
}

/// A read command pointed at `server`, pinned to a specific time zone.
fn zoned(dir: &TempDir, server: &MockServer, tz: &str) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env("TZ", tz);
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

/// A command with no reachable server: a usage failure must be produced
/// locally instead of a network error.
fn offline(dir: &TempDir, tz: &str) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env("HAMSTIK_HOST", "http://127.0.0.1:1");
    cmd.env("TZ", tz);
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Parses the RFC 3339 instant the CLI put on the wire.
fn parse_rfc3339(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .unwrap_or_else(|err| panic!("{value:?} is not canonical RFC 3339: {err}"))
        .timestamp()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relative_expressions_reach_the_wire_as_utc_instants() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();

    // The server only ever sees an absolute RFC 3339 filter, never `7d`.
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_list()))
        .expect(1)
        .mount(&server)
        .await;

    let before = now_secs();
    let output = zoned(&dir, &server, "UTC")
        .args([
            "--no-input",
            "--verbose",
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "list",
            "--updated-after",
            "7d",
            "--due-before",
            "+2w",
        ])
        .output()
        .unwrap();
    let after = now_secs();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);

    // Assert the resolved query parameters from the recorded request.
    let requests = server.received_requests().await.unwrap();
    let received = requests
        .iter()
        .find(|req| req.url.path().ends_with("/work-items"))
        .expect("a work item list request");
    let updated_after = received
        .url
        .query_pairs()
        .find(|(name, _)| name == "updatedAfter")
        .expect("updatedAfter must be sent")
        .1
        .to_string();
    let due_before = received
        .url
        .query_pairs()
        .find(|(name, _)| name == "dueBefore")
        .expect("dueBefore must be sent")
        .1
        .to_string();

    // `7d` is one week before now, `+2w` two weeks after it.
    let week = i64::from(7 * 24 * 3600);
    let resolved = parse_rfc3339(&updated_after);
    assert!(
        (i64::try_from(before).unwrap() - week - 5..=i64::try_from(after).unwrap() - week + 5)
            .contains(&resolved),
        "7d resolved to {updated_after}, expected ~{}s",
        i64::try_from(after).unwrap() - week
    );
    let future = parse_rfc3339(&due_before);
    assert!(
        (i64::try_from(before).unwrap() + 2 * week - 5
            ..=i64::try_from(after).unwrap() + 2 * week + 5)
            .contains(&future),
        "+2w resolved to {due_before}"
    );

    // --verbose shows every resolved flag so the exact value sent is inspectable.
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(&format!(
            "[verbose] --updated-after resolved to {updated_after}"
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("[verbose] --due-before resolved to {due_before}")),
        "{stderr}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_rfc3339_input_is_sent_unchanged() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .and(query_param("updatedAfter", "2026-09-01T00:00:00Z"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_list()))
        .expect(1)
        .mount(&server)
        .await;

    // A pinned TZ must not move an input that carries its own offset.
    let output = zoned(&dir, &server, "America/Los_Angeles")
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "list",
            "--updated-after",
            "2026-09-01T00:00:00Z",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
}

/// A bare date is local civil time: pin exact instants when children honor
/// `TZ`, otherwise verify the child agrees with the host-resolved local zone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bare_dates_use_the_host_time_zone() {
    let dir = TempDir::new().unwrap();
    if child_honors_tz(&dir, "Asia/Kolkata", "2026-09-01", "2026-08-31T18:30:00Z") {
        let cases = [
            ("UTC", "2026-09-01T00:00:00Z"),
            ("Asia/Kolkata", "2026-08-31T18:30:00Z"),
            ("America/New_York", "2026-09-01T04:00:00Z"),
        ];
        for (tz, expected) in cases {
            let output = offline(&dir, tz)
                .args([
                    "--no-input",
                    "--json",
                    "--dry-run",
                    "--org",
                    "acme",
                    "--project",
                    "HAM",
                    "work",
                    "create",
                    "--title",
                    "Dated",
                    "--due-date",
                    "2026-09-01",
                ])
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
            let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(preview["body"]["dueDate"], expected, "TZ={tz}");
        }
    } else {
        eprintln!("SKIPPED: child process did not honor TZ; checking the host-resolved local zone");

        let local_midnight = NaiveDate::from_ymd_opt(2026, 9, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let expected = resolve_host_local(local_midnight)
            .with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::AutoSi, true);
        assert_eq!(
            preview_due_date_without_tz(&dir, "Dated", "2026-09-01"),
            expected,
            "bare date should resolve to midnight in the host-resolved local zone"
        );
    }
}

/// Runs `work create --dry-run` with `--due-date <value>` against no server and
/// returns the `dueDate` the CLI put in the preview body.
fn preview_due_date(dir: &TempDir, tz: &str, title: &str, value: &str) -> String {
    preview_due_date_with(offline(dir, tz), title, value)
}

/// Returns whether a child resolved a timezone-less input against `tz`.
fn child_honors_tz(dir: &TempDir, tz: &str, input: &str, expected: &str) -> bool {
    preview_due_date(dir, tz, "Probe", input) == expected
}

/// Runs the due-date preview without a `TZ` override, including one inherited
/// by the test harness, so the child uses the host-resolved local zone.
fn preview_due_date_without_tz(dir: &TempDir, title: &str, value: &str) -> String {
    let mut command = offline(dir, "UTC");
    command.env_remove("TZ");
    preview_due_date_with(command, title, value)
}

fn preview_due_date_with(mut command: Command, title: &str, value: &str) -> String {
    let output = command
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "create",
            "--title",
            title,
            "--due-date",
            value,
        ])
        .output()
        .expect("CLI runs");
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    preview["body"]["dueDate"].as_str().unwrap().to_owned()
}

/// Resolves host-local civil time using the CLI's documented overlap and gap
/// rules: take the earliest instant, or move forward hour by hour past a gap.
fn resolve_host_local(naive: NaiveDateTime) -> chrono::DateTime<Local> {
    Local
        .from_local_datetime(&naive)
        .earliest()
        .unwrap_or_else(|| {
            (1..=48)
                .find_map(|hours| {
                    let shifted = naive.checked_add_signed(Duration::hours(hours))?;
                    Local.from_local_datetime(&shifted).earliest()
                })
                .expect("host local time resolves within 48 hours")
        })
}

/// DST edge cases: a clock time the spring-forward skipped moves past the
/// shift, an ambiguous fall-back time takes the earliest instant, and both stay
/// deterministic. The CLI converts locally, so the wire value is always UTC.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daylight_saving_edges_resolve_deterministically() {
    let dir = TempDir::new().unwrap();

    // Children honor `TZ` on unix; Windows consults the system zone instead, in
    // which case these New York specific edges do not apply.
    if !child_honors_tz(
        &dir,
        "America/New_York",
        "2026-06-15 12:00",
        "2026-06-15T16:00:00Z",
    ) {
        eprintln!("SKIPPED: child process did not resolve against America/New_York");
        return;
    }

    // 2026-03-08 02:30 never happened in New York (02:00 jumped to 03:00 EDT),
    // so the reading moves past the gap to 03:30 EDT.
    assert_eq!(
        preview_due_date(&dir, "America/New_York", "Gap", "2026-03-08 02:30"),
        "2026-03-08T07:30:00Z"
    );

    // 2026-11-01 01:30 happened twice; the earliest instant (01:30 EDT) wins.
    assert_eq!(
        preview_due_date(&dir, "America/New_York", "Overlap", "2026-11-01 01:30"),
        "2026-11-01T05:30:00Z"
    );
}

/// Invalid expressions are a usage error (exit 2) raised locally: the host is
/// unreachable here, so any attempt to reach it would surface as exit 8.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_expressions_fail_locally_before_any_request() {
    let dir = TempDir::new().unwrap();
    for bad in ["soon", "7", "2026-13-01", "2026-09-01T99:00", "yesterdyay"] {
        let output = offline(&dir, "UTC")
            .args([
                "--no-input",
                "--json",
                "--org",
                "acme",
                "--project",
                "HAM",
                "work",
                "list",
                "--due-after",
                bad,
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{bad}: {:?}", output.stderr);
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("invalid date/time"), "{bad}: {stderr}");

        // Mutations fail the same way, before a preview is emitted.
        let output = offline(&dir, "UTC")
            .args([
                "--no-input",
                "--json",
                "--dry-run",
                "--org",
                "acme",
                "--project",
                "HAM",
                "sprint",
                "create",
                "--name",
                "Bad",
                "--start-date",
                bad,
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2), "{bad}: {:?}", output.stderr);
        assert!(
            String::from_utf8_lossy(&output.stdout).is_empty(),
            "{bad}: nothing may be previewed"
        );
    }
}
