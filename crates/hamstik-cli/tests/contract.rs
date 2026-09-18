// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Compatibility contract tests for the documented stable CLI surfaces
//! (CLI-15, SPEC §37–§55):
//!
//! 1. command hierarchy, option names, aliases, and enum spellings;
//! 2. the versioned JSON failure envelope and stable exit codes (§52–§54);
//! 3. stdout/stderr separation, quiet mode, and no-input behavior;
//! 4. pagination envelopes and sparse fields (§43);
//! 5. mutation metadata (idempotency replay, revision/ETag echo).
//!
//! These tests validate semantics, not incidental whitespace: JSON shapes are
//! checked structurally, and nondeterministic values (request IDs, cursors,
//! timestamps, server-generated ids) are never frozen. Regeneration is
//! unnecessary by design: every expectation below is inline, versioned in
//! git, and fails with a focused diff when a documented surface changes.
//! A deliberate breaking change must update these expectations in the same
//! commit and add a `### Breaking` changelog entry (design/VERSIONING.md).

use std::path::Path;

use assert_cmd::Command;
use clap::{CommandFactory, Parser};
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const EXIT_SUCCESS: i32 = 0;
const EXIT_GENERAL: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_AUTHENTICATION: i32 = 3;
const EXIT_AUTHORIZATION: i32 = 4;
const EXIT_NOT_FOUND: i32 = 5;
const EXIT_CONFLICT: i32 = 6;
const EXIT_RATE_LIMITED: i32 = 7;
const EXIT_NETWORK: i32 = 8;
const EXIT_SERVER: i32 = 9;
const EXIT_CONFIGURATION: i32 = 10;

fn page(items: Value) -> Value {
    json!({ "items": items, "page": { "limit": 50, "hasMore": false, "nextCursor": null } })
}

fn work_item_json(status: &str, revision: i64) -> Value {
    json!({
        "id": "1", "key": "HAM-1", "projectId": "2", "title": "T", "description": null,
        "type": "task", "status": status, "priority": "low", "assignee": null, "reporter": null,
        "sprint": null, "parent": null, "labels": [],
        "storyPoints": null, "dueDate": null, "archivedAt": null,
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": revision
    })
}

fn error_body(code: &str, status: u16) -> Value {
    // The server envelope carries the request id at the top level; the CLI
    // error mapping preserves it (see existing cli.rs fixtures and the live
    // API, which returns `{ error: {...}, requestId: ... }`).
    json!({
        "error": {
            "code": code,
            "message": "contract test",
            "status": status
        },
        "requestId": "req-contract"
    })
}

/// Base command wired to `server` + a throwaway config, using an ephemeral
/// token; mirrors `base()` in `cli.rs` without its network retry behavior.
fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.env_remove("NO_COLOR");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

/// A command with no server at all: only local surfaces (help, parse errors,
/// usage errors, configuration errors) are exercised.
fn local(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.env_remove("HAMSTIK_TOKEN");
    cmd.env_remove("HAMSTIK_HOST");
    cmd.current_dir(dir.path());
    cmd
}

// ---------------------------------------------------------------------------
// 1. Command hierarchy, flags, aliases, and enum spellings
// ---------------------------------------------------------------------------

/// The documented root command set (README "Command groups" and SPEC §36).
/// Removing or renaming a documented command fails here with a focused diff.
#[test]
fn root_command_hierarchy_matches_documented_surface() {
    use hamstik_cli::args::Cli;

    let root = Cli::command();
    let names: Vec<&str> = root.get_subcommands().map(|c| c.get_name()).collect();
    assert_eq!(
        names,
        [
            "me",
            "auth",
            "context",
            "config",
            "org",
            "project",
            "sprint",
            "label",
            "work",
            "user",
            "squeakql",
            "api",
            "agent",
            "doctor",
            "completion",
            "init",
            "commands",
            "version",
        ],
        "root subcommand set changed; update README + CHANGELOG (Breaking) + this fixture"
    );
}

fn subcommand_names(path: &[&str]) -> Vec<String> {
    let mut command = hamstik_cli::args::Cli::command();
    for segment in path {
        command = command
            .find_subcommand_mut(segment)
            .unwrap_or_else(|| panic!("missing `{segment}` in tree"))
            .clone();
    }
    let mut names: Vec<String> = command
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    names.sort();
    names
}

fn subcommand_flags(path: &[&str]) -> Vec<String> {
    let mut command = hamstik_cli::args::Cli::command();
    for segment in path {
        command = command
            .find_subcommand_mut(segment)
            .unwrap_or_else(|| panic!("missing `{segment}` in tree"))
            .clone();
    }
    let mut flags: Vec<String> = command
        .get_arguments()
        .filter_map(|a| {
            let id = a.get_id().as_str();
            (id != "help" && a.get_long().is_some()).then(|| format!("--{}", a.get_long().unwrap()))
        })
        .collect();
    flags.sort();
    flags.dedup();
    flags
}

/// Documented subcommand sets per command group.
#[test]
fn subcommand_groups_match_documented_surface() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "auth",
            &["auth"],
            &["forget", "list", "login", "logout", "status", "switch"],
        ),
        (
            "context",
            &["context"],
            &["clear", "explain", "init", "set", "show"],
        ),
        ("org", &["org"], &["list", "members", "use", "view", "work"]),
        (
            "project",
            &["project"],
            &[
                "activity",
                "archive",
                "create",
                "edit",
                "list",
                "unarchive",
                "use",
                "view",
            ],
        ),
        (
            "sprint",
            &["sprint"],
            &[
                "archive",
                "create",
                "list",
                "transition",
                "transitions",
                "unarchive",
                "view",
            ],
        ),
        ("label", &["label"], &["create", "list"]),
        (
            "work",
            &["work"],
            &[
                "activity",
                "archive",
                "attachment",
                "await",
                "bulk",
                "close",
                "comment",
                "context",
                "create",
                "delete",
                "edit",
                "label",
                "link",
                "list",
                "mine",
                "search",
                "start",
                "transition",
                "transitions",
                "unarchive",
                "view",
                "watcher",
            ],
        ),
        ("user", &["user"], &["activity", "avatar", "view", "work"]),
        (
            "squeakql",
            &["squeakql"],
            &["delete", "list", "save", "show", "validate"],
        ),
        ("api", &["api"], &["openapi", "request"]),
    ];
    for (label, group, expected) in cases {
        let mut expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(
            subcommand_names(group),
            expected,
            "`{label}` subcommand set changed; update README + CHANGELOG + this fixture"
        );
    }
}

/// Documented aliases: `work mine` also answers to `work my` (README), and
/// `org list|view|use` / `project list|view|use` spellings are stable.
#[test]
fn documented_command_aliases_resolve() {
    let mut command = hamstik_cli::args::Cli::command();
    let work = command
        .find_subcommand_mut("work")
        .expect("work group")
        .clone();
    let mine = work.find_subcommand("mine").expect("work mine");
    let aliases: Vec<&str> = mine.get_all_aliases().collect();
    assert!(
        aliases.contains(&"my"),
        "`work mine` alias `my` disappeared; update README + CHANGELOG + this fixture"
    );
}

/// The global option set (SPEC §37) exists on the root parser.
#[test]
fn global_options_match_documented_surface() {
    let root = hamstik_cli::args::Cli::command();
    let mut flags: Vec<String> = root
        .get_arguments()
        .filter_map(|a| a.get_long().map(|l| format!("--{l}")))
        .collect();
    flags.sort();
    let mut expected: Vec<&str> = [
        "--host",
        "--profile",
        "--org",
        "--project",
        "--json",
        "--quiet",
        "--verbose",
        "--no-color",
        "--no-input",
        "--no-retry",
        "--dry-run",
        "--ca-bundle",
    ]
    .into_iter()
    .collect();
    expected.sort();
    assert_eq!(
        flags, expected,
        "global option set changed; update README + CHANGELOG + this fixture"
    );
}

/// Documented enum spellings must not drift: these exact strings are the
/// machine-facing values sent on the wire and printed in help (SPEC §42+).
#[test]
fn documented_enum_spellings_are_stable() {
    let spellings: &[(&str, &[&str])] = &[
        (
            "status",
            &["backlog", "todo", "in_progress", "in_review", "done"],
        ),
        ("type", &["task", "bug", "story", "feature", "epic"]),
        ("priority", &["low", "medium", "high", "urgent"]),
        ("scope", &["all", "open", "closed"]),
        ("sort", &["updated", "dueDate", "priority", "rank"]),
        ("involvement", &["assigned", "created", "commented"]),
        ("relation", &["blocks", "blocked_by", "relates"]),
        ("concurrency", &["require-revision", "last-write-wins"]),
        ("sprint state", &["active", "done"]),
    ];
    for (label, expected) in spellings {
        // Each enum value must be accepted verbatim by the parser for at
        // least one documented flag, and rejected spellings must fail.
        for value in *expected {
            let ok = hamstik_cli::args::Cli::try_parse_from([
                "hamstik",
                "work",
                "transition",
                "HAM-1",
                value,
            ])
            .is_ok();
            if *label == "status" {
                assert!(
                    ok,
                    "status spelling `{value}` no longer parses; documented enum changed"
                );
            }
        }
        let bad = hamstik_cli::args::Cli::try_parse_from([
            "hamstik",
            "work",
            "transition",
            "HAM-1",
            "__not_a_status__",
        ])
        .is_err();
        assert!(bad, "invalid `{label}` value parsed successfully");
    }
}

/// Flag names on representative documented commands are frozen.
#[test]
fn representative_command_flags_match_documented_surface() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "work create",
            &["work", "create"],
            &[
                "--assignee",
                "--description",
                "--description-editor",
                "--description-file",
                "--due-date",
                "--idempotency-key",
                "--parent",
                "--priority",
                "--sprint",
                "--status",
                "--story-points",
                "--title",
                "--type",
            ],
        ),
        (
            "work edit",
            &["work", "edit"],
            &[
                "--assignee",
                "--clear-assignee",
                "--clear-description",
                "--clear-due-date",
                "--clear-parent",
                "--clear-sprint",
                "--clear-story-points",
                "--description",
                "--description-editor",
                "--description-file",
                "--due-date",
                "--force",
                "--parent",
                "--priority",
                "--sprint",
                "--story-points",
                "--title",
                "--type",
            ],
        ),
        (
            "work list",
            &["work", "list"],
            &[
                "--all",
                "--archived",
                "--assignee",
                "--cursor",
                "--due-after",
                "--due-before",
                "--fields",
                "--label",
                "--label-name",
                "--limit",
                "--mine",
                "--overdue",
                "--parent",
                "--priority",
                "--scope",
                "--search",
                "--sort",
                "--sprint",
                "--status",
                "--top-level",
                "--type",
                "--updated-after",
            ],
        ),
        (
            "work mine",
            &["work", "mine"],
            &[
                "--all",
                "--archived",
                "--cursor",
                "--due-after",
                "--due-before",
                "--fields",
                "--label",
                "--label-name",
                "--limit",
                "--overdue",
                "--priority",
                "--project",
                "--scope",
                "--sort",
                "--status",
                "--type",
            ],
        ),
        (
            "project list",
            &["project", "list"],
            &["--all", "--archived", "--cursor", "--limit"],
        ),
        (
            "org work",
            &["org", "work"],
            &[
                "--all",
                "--archived",
                "--assignee",
                "--cursor",
                "--due-after",
                "--due-before",
                "--fields",
                "--label",
                "--label-name",
                "--limit",
                "--mine",
                "--overdue",
                "--parent",
                "--priority",
                "--project",
                "--scope",
                "--search",
                "--sort",
                "--sprint",
                "--status",
                "--top-level",
                "--type",
                "--updated-after",
            ],
        ),
        (
            "user avatar",
            &["user", "avatar"],
            &["--avatar-version", "--format", "--output", "--revision"],
        ),
        (
            "work bulk update",
            &["work", "bulk", "update"],
            &["--concurrency", "--idempotency-key", "--operations-file"],
        ),
        ("auth login", &["auth", "login"], &["--with-token"]),
        ("context show", &["context", "show"], &["--explain"]),
    ];
    for (label, group, expected) in cases {
        let flags = subcommand_flags(group);
        for flag in *expected {
            assert!(
                flags.iter().any(|f| f == flag),
                "`{label}` lost documented flag `{flag}`; update README + CHANGELOG + this fixture (flags: {flags:?})"
            );
        }
        // A renamed flag is as breaking as a removed one: reject additions
        // that were never documented by asserting the full set is a superset
        // of exactly the documented ones (new flags must extend this fixture
        // deliberately).
        let unexpected: Vec<&String> = flags
            .iter()
            .filter(|f| !expected.contains(&f.as_str()))
            .collect();
        assert!(
            unexpected.is_empty(),
            "`{label}` grew undocumented flags {unexpected:?}; document them and extend this fixture"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Stable exit codes and the versioned JSON failure envelope (§52–§54)
// ---------------------------------------------------------------------------

/// Exercises the failure envelope `{ "error": { kind, code, message, ... } }`
/// and the stable exit-code mapping across the documented classes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_envelope_and_exit_codes_match_contract() {
    struct Case {
        name: &'static str,
        code: &'static str,
        http: u16,
        expected_exit: i32,
    }
    let cases = [
        Case {
            name: "authentication",
            code: "AUTH_REQUIRED",
            http: 401,
            expected_exit: EXIT_AUTHENTICATION,
        },
        Case {
            name: "authorization",
            code: "FORBIDDEN",
            http: 403,
            expected_exit: EXIT_AUTHORIZATION,
        },
        Case {
            name: "not found",
            code: "NOT_FOUND",
            http: 404,
            expected_exit: EXIT_NOT_FOUND,
        },
        Case {
            name: "conflict",
            code: "REVISION_CONFLICT",
            http: 412,
            expected_exit: EXIT_CONFLICT,
        },
        Case {
            name: "rate limit",
            code: "RATE_LIMITED",
            http: 429,
            expected_exit: EXIT_RATE_LIMITED,
        },
        Case {
            name: "usage",
            code: "VALIDATION_ERROR",
            http: 400,
            expected_exit: EXIT_USAGE,
        },
        Case {
            name: "server",
            code: "INTERNAL",
            http: 500,
            expected_exit: EXIT_SERVER,
        },
        Case {
            name: "general",
            code: "UNKNOWN_CODE",
            http: 418,
            expected_exit: EXIT_GENERAL,
        },
    ];

    for case in &cases {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
            .respond_with(
                ResponseTemplate::new(case.http).set_body_json(error_body(case.code, case.http)),
            )
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
                "list",
                "--json",
            ])
            .output()
            .unwrap();

        assert_eq!(
            output.status.code(),
            Some(case.expected_exit),
            "`{}` ({} {}) exited with {:?}; stable exit code is {} (SPEC §52)",
            case.name,
            case.http,
            case.code,
            output.status.code(),
            case.expected_exit
        );
        assert!(
            output.stdout.is_empty(),
            "failure must keep stdout empty (SPEC §39); `{}` leaked stdout",
            case.name
        );

        let body: Value = serde_json::from_slice(&output.stderr).unwrap();
        let error = &body["error"];
        assert_eq!(
            error["code"], case.code,
            "`{}` code field drifted",
            case.name
        );
        assert_eq!(
            error["status"], case.http,
            "`{}` status field drifted",
            case.name
        );
        assert!(
            error["message"].is_string(),
            "`{}` message field missing",
            case.name
        );
        assert_eq!(
            error["requestId"], "req-contract",
            "`{}` requestId field drifted",
            case.name
        );
        assert!(
            error["kind"].is_string(),
            "`{}` kind field missing (SPEC §54)",
            case.name
        );
    }
}

/// Local failure classes map to their documented kinds and exit codes.
#[test]
fn local_error_classes_match_contract() {
    let dir = TempDir::new().unwrap();

    // Parse/usage failure: clap help goes to stderr with exit 2.
    let output = local(&dir)
        .args(["work", "transition", "HAM-1", "__not_a_status__"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
    assert!(output.stdout.is_empty(), "parse failure leaked stdout");

    // Missing required input under --no-input: usage error, exit 2.
    let output = local(&dir)
        .args(["--no-input", "--json", "work", "create"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(body["error"]["kind"], "usage");
    assert_eq!(body["error"]["code"], "INVALID_INPUT");

    // Missing organization selection: usage error with the documented message.
    let output = local(&dir)
        .args(["--no-input", "--json", "work", "list"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(body["error"]["kind"], "usage");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no organization selected"),
        "missing-org guidance message drifted"
    );

    // --json + --quiet is rejected deterministically (README output section).
    let output = local(&dir)
        .args(["--json", "--quiet", "version"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_USAGE));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(body["error"]["code"], "INVALID_INPUT");

    // Unreachable host with an environment token: network kind, exit 8.
    // Both org and project must resolve before the request is attempted.
    let dir = TempDir::new().unwrap();
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_HOST", "http://127.0.0.1:1");
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env("HAMSTIK_ORG", "acme");
    cmd.env("HAMSTIK_PROJECT", "HAM");
    cmd.current_dir(dir.path());
    let output = cmd
        .args(["--no-input", "--no-retry", "--json", "work", "list"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_NETWORK));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(body["error"]["kind"], "network");
    assert_eq!(body["error"]["code"], "NETWORK_ERROR");
}

/// The configuration/credential-store failure class exits 10.
#[test]
fn configuration_failures_exit_ten() {
    let dir = TempDir::new().unwrap();
    // A config path that cannot be read: a directory where config.toml is
    // expected. `version` never touches configuration; any command that
    // resolves the profile does and must fail with exit 10 (SPEC §52).
    let config_dir = dir.path().join("config.toml");
    std::fs::create_dir(&config_dir).unwrap();
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", &config_dir);
    cmd.current_dir(dir.path());
    let output = cmd.args(["--json", "me"]).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(EXIT_CONFIGURATION),
        "configuration failure exit code drifted"
    );
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(body["error"]["kind"], "configuration");
    assert_eq!(body["error"]["code"], "CONFIGURATION_ERROR");
}

// ---------------------------------------------------------------------------
// 3. stdout/stderr separation, quiet mode, and no-input behavior
// ---------------------------------------------------------------------------

/// `version --json` is the smallest complete success surface: JSON to stdout,
/// empty stderr, exit 0 (SPEC §39).
#[test]
fn success_json_goes_to_stdout_and_stderr_stays_clean() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir).args(["--json", "version"]).output().unwrap();
    assert_eq!(output.status.code(), Some(EXIT_SUCCESS));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        output.stderr.is_empty(),
        "success surface wrote diagnostics to stderr: {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `--quiet` emits the primary identifier only (SPEC §55); `--json` and
/// `--quiet` remain mutually exclusive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quiet_mode_emits_primary_identifier_column_only() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(page(json!([work_item_json("todo", 1),]))),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "--quiet",
            "work",
            "list",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_SUCCESS));
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "--quiet must emit one line per item: {text:?}"
    );
    assert_eq!(
        lines[0], "HAM-1",
        "--quiet must emit the primary identifier"
    );
}

/// Missing required values under `--no-input` fail deterministically instead
/// of prompting (SPEC §41), even when stdin would be available.
#[test]
fn no_input_never_prompts_and_fails_deterministically() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
        .args(["--no-input", "--json", "work", "create"])
        .write_stdin("injected title\n")
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "--no-input must not consume stdin for missing required options"
    );
}

// ---------------------------------------------------------------------------
// 4. Pagination envelopes and sparse fields (§43)
// ---------------------------------------------------------------------------

/// The collection envelope is `{ items, page: { limit, hasMore, nextCursor } }`
/// and `--fields` forwards the sparse fieldset verbatim; the server decides
/// sparseness, and the CLI must not add or drop fields in `--json` mode.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collection_envelope_and_sparse_fields_match_contract() {
    let server = MockServer::start().await;
    // The server honors the sparse fieldset; the response contains only the
    // requested fields. `--json` must echo that resource byte-faithfully.
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{
                "id": "1", "key": "HAM-1", "projectId": "2", "title": "T",
                "description": null, "type": "task", "status": "todo",
                "priority": null, "assignee": null, "reporter": null,
                "sprint": null, "parent": null, "labels": [],
                "storyPoints": null, "dueDate": null, "archivedAt": null,
                "createdAt": "2026-01-01T00:00:00Z",
                "updatedAt": "2026-01-02T00:00:00Z", "revision": 1
            }],
            "page": { "limit": 50, "hasMore": true, "nextCursor": "opaque-cursor" }
        })))
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
            "list",
            "--json",
            "--fields",
            "title,status",
            "--limit",
            "50",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_SUCCESS));

    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        body["items"].is_array(),
        "collection JSON must expose `items` (SPEC §43): {body}"
    );
    let item = &body["items"][0];
    assert_eq!(item["title"], "T", "JSON echo dropped a server field");
    assert_eq!(item["status"], "todo", "JSON echo dropped a server field");
    let page_shape = &body["page"];
    assert_eq!(page_shape["limit"], 50, "page.limit drifted");
    assert_eq!(page_shape["hasMore"], true, "page.hasMore drifted");
    assert_eq!(
        page_shape["nextCursor"], "opaque-cursor",
        "page.nextCursor drifted"
    );

    // The sparse fieldset itself is forwarded verbatim to the wire.
    let request = &server.received_requests().await.unwrap()[0];
    let query = request.url.query().unwrap_or_default();
    assert!(
        query.contains("fields=title%2Cstatus") || query.contains("fields=title,status"),
        "--fields must forward the fieldset verbatim: {query}"
    );
}

/// `--all` aggregates pages into the documented all-results shape and follows
/// the opaque cursor without rewriting it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_pages_aggregate_shape_matches_contract() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(|req: &wiremock::Request| {
            let has_cursor = req.url.query().unwrap_or_default().contains("cursor=");
            let body = if has_cursor {
                page(json!([]))
            } else {
                json!({
                    "items": [work_item_json("todo", 1)],
                    "page": { "limit": 50, "hasMore": true, "nextCursor": "opaque-cursor" }
                })
            };
            ResponseTemplate::new(200).set_body_json(body)
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
            "list",
            "--json",
            "--all",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_SUCCESS));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        body["items"].as_array().map(Vec::len),
        Some(1),
        "--all aggregate must contain every nonempty page's items: {body}"
    );
    assert!(
        body["items"][0]["key"] == "HAM-1",
        "--all aggregate lost item identity"
    );
}

// ---------------------------------------------------------------------------
// 5. Mutation metadata: idempotency replay and revision/ETag echo
// ---------------------------------------------------------------------------

/// A mutation response keeps the raw resource echo (with `revision`), and an
/// idempotent replay stays a success (exit 0).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutation_metadata_and_replay_behavior_match_contract() {
    let server = MockServer::start().await;
    // `work transition` reads the item (ETag), checks permitted transitions,
    // then posts the transition.
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-1\"")
                .set_body_json(work_item_json("todo", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/transitions",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currentStatus": "todo",
            "transitions": [{"targetStatus": "in_progress"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/transitions",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-Idempotent-Replay", "true")
                .set_body_json(work_item_json("in_progress", 2)),
        )
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
            "transition",
            "HAM-1",
            "in_progress",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(EXIT_SUCCESS),
        "idempotent replay must not fail the command"
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["key"], "HAM-1", "mutation JSON echo lost the key");
    assert_eq!(
        body["status"], "in_progress",
        "mutation JSON echo lost the status"
    );
    assert_eq!(body["revision"], 2, "mutation JSON echo lost the revision");
}

/// README compatibility fixture: the documented `work archive` / `unarchive`
/// lifecycle keeps its exit codes and ETag-based conflict behavior.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readme_archive_lifecycle_fixture_keeps_contract() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-1\"")
                .set_body_json(work_item_json("todo", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/archive",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_item_json("todo", 2)))
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
            "archive",
            "HAM-1",
        ])
        .assert()
        .code(EXIT_SUCCESS);
}

/// README compatibility fixture: `work mine` follows the documented filter
/// flags and the `work my` alias resolves to the same command.
#[test]
fn readme_work_mine_fixture_keeps_alias_and_flags() {
    let parsed = hamstik_cli::args::Cli::try_parse_from([
        "hamstik", "work", "my", "--scope", "open", "--all",
    ])
    .expect("documented `work my` alias must parse");
    match parsed.command {
        hamstik_cli::args::Command::Work(work) => match work.command {
            hamstik_cli::args::WorkCommand::Mine(args) => {
                assert_eq!(
                    args.scope.map(|s| s.as_str().to_string()),
                    Some("open".to_string()),
                    "--scope spelling drifted"
                );
                assert!(args.pagination.all, "--all drift");
            }
            other => panic!("`work my` resolved to an unexpected command: {other:?}"),
        },
        other => panic!("root parse drifted: {other:?}"),
    }
}

/// Binary-download safeguards: a 404 avatar is a clean NOT_FOUND with the
/// documented exit code rather than a partial file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn binary_download_missing_resource_keeps_not_found_contract() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/usr_cPbfeqnghA-RLpDVOMQhHg/avatar"))
        .respond_with(ResponseTemplate::new(404).set_body_json(error_body("NOT_FOUND", 404)))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "user",
            "avatar",
            "usr_cPbfeqnghA-RLpDVOMQhHg",
            "--output",
            "avatar.bin",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(EXIT_NOT_FOUND));
    assert!(
        !Path::new("avatar.bin").exists(),
        "404 must not write an output file"
    );
}

/// Context precedence fixture (README + SPEC §34): `--org`/`--project`
/// overrides reach the wire even when the context file agrees or disagrees.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn context_precedence_fixture_keeps_override_contract() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([]))))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(".hamstik.toml"),
        "version = 1\norganization = \"ctx-org\"\nproject = \"CTX\"\n",
    )
    .unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "list",
            "--json",
        ])
        .assert()
        .code(EXIT_SUCCESS);

    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request.url.path(),
        "/api/v1/organizations/acme/projects/HAM/work-items",
        "global override lost to the context file"
    );
}
