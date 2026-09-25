// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `context explain` precedence traces, offline behavior, and
//! credential redaction.

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

fn local(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    // Keep test mutations out of the developer's real audit log.
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.env_remove("HAMSTIK_HOST");
    cmd.env_remove("HAMSTIK_TOKEN");
    cmd.current_dir(dir.path());
    cmd.arg("--no-input");
    cmd
}

/// The full precedence chain for one field, from `context explain --json`.
fn chain<'a>(body: &'a Value, field: &str) -> &'a Value {
    &body["values"][field]
}

/// Runs git in `dir`, asserting success. Commit identity comes from the
/// environment so the developer's global git config is not required.
fn git(dir: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "hamstik-test")
        .env("GIT_AUTHOR_EMAIL", "hamstik-test@example.com")
        .env("GIT_COMMITTER_NAME", "hamstik-test")
        .env("GIT_COMMITTER_EMAIL", "hamstik-test@example.com")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// Every configurable value reports its winning source and full chain; the
/// report is local-only and represents HAMSTIK_TOKEN as presence only.
#[test]
fn explain_reports_complete_chains_offline() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
        .env("HAMSTIK_TOKEN", "secret-token-value")
        .env("HAMSTIK_ORG", "env-org")
        .args(["--org", "cli-org", "--json", "context", "explain"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["localOnly"], true);
    assert_eq!(body["schemaVersion"], 1);

    // Organization: CLI wins, env is shadowed, both visible.
    let organization = chain(&body, "organization");
    assert_eq!(organization["winningSource"], "cli");
    let sources = organization["sources"].as_array().unwrap();
    assert_eq!(sources[0]["status"], "winner");
    assert_eq!(sources[0]["value"], "cli-org");
    assert_eq!(sources[1]["status"], "shadowed");
    assert_eq!(sources[1]["value"], "env-org");

    // Project: unset falls through to the default entry.
    let project = chain(&body, "project");
    assert_eq!(project["winningSource"], "default");
    assert_eq!(project["sources"][0]["status"], "unset");

    // Host falls back to the documented default.
    let host = chain(&body, "host");
    assert_eq!(host["winningSource"], "default");
    assert_eq!(host["sources"][0]["value"], "https://hamstik.com");

    // Token: presence only, never the value.
    let token = chain(&body, "token");
    assert_eq!(token["sources"][0]["source"], "environment");
    let token_text = token.to_string();
    assert!(
        !token_text.contains("secret-token-value"),
        "token material leaked: {token_text}"
    );
}

/// Nearest-context-file discovery: a nested context file wins over one in a
/// parent directory, and the discovery path is reported.
#[test]
fn explain_reports_nearest_context_file_discovery() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let nested = root.join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        root.join(".hamstik.toml"),
        "version = 1\norganization = \"parent-org\"\n",
    )
    .unwrap();
    std::fs::write(
        nested.join(".hamstik.toml"),
        "version = 1\norganization = \"nearest-org\"\n",
    )
    .unwrap();

    let output = Command::cargo_bin("hamstik")
        .unwrap()
        .env("HAMSTIK_CONFIG", root.join("config.toml"))
        .current_dir(&nested)
        .args(["--no-input", "--json", "context", "explain"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    // The CLI reports the OS-native path (backslashes on Windows), so compare
    // path components rather than a forward-slash text suffix.
    let found = body["contextDiscovery"]["found"].as_str().unwrap();
    assert!(
        std::path::Path::new(found).ends_with(std::path::Path::new("b").join(".hamstik.toml")),
        "the nearest file must win: {body:?}"
    );
    assert_eq!(
        chain(&body, "organization")["winningSource"],
        "context_file"
    );
    assert_eq!(
        chain(&body, "organization")["sources"][0]["value"],
        "nearest-org"
    );
}

/// A `.hamstik.toml` that exists only in the primary checkout of a git
/// worktree is still discovered from the linked worktree, and `context
/// explain` reports the resolved path.
#[test]
fn explain_reports_context_found_through_linked_worktree() {
    let dir = TempDir::new().unwrap();
    let main = dir.path().join("main");
    let linked = dir.path().join("linked");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q"]);
    std::fs::write(main.join("tracked.txt"), "x\n").unwrap();
    git(&main, &["add", "tracked.txt"]);
    git(&main, &["commit", "-qm", "init"]);
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            linked.to_str().unwrap(),
            "-b",
            "feature",
        ],
    );
    // Untracked, so the linked worktree never sees it via the walk-up.
    std::fs::write(
        main.join(".hamstik.toml"),
        "version = 1\norganization = \"main-org\"\n",
    )
    .unwrap();
    let nested = linked.join("sub").join("dir");
    std::fs::create_dir_all(&nested).unwrap();

    let output = Command::cargo_bin("hamstik")
        .unwrap()
        .env("HAMSTIK_CONFIG", main.join("config.toml"))
        .env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"))
        .env(
            "HAMSTIK_REQUEST_JOURNAL",
            dir.path().join("request-journal.log"),
        )
        .env_remove("HAMSTIK_PROFILE")
        .env_remove("HAMSTIK_ORG")
        .env_remove("HAMSTIK_PROJECT")
        .env_remove("HAMSTIK_HOST")
        .env_remove("HAMSTIK_TOKEN")
        .current_dir(&nested)
        .args(["--no-input", "--json", "context", "explain"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let found = body["contextDiscovery"]["found"].as_str().unwrap();
    let expected = std::fs::canonicalize(&main).unwrap().join(".hamstik.toml");
    assert_eq!(std::path::Path::new(found), expected.as_path(), "{body:?}");
    assert_eq!(
        chain(&body, "organization")["winningSource"],
        "context_file"
    );
    assert_eq!(
        chain(&body, "organization")["sources"][0]["value"],
        "main-org"
    );
}

/// Profile selection precedence mirrors the resolver: --profile beats
/// HAMSTIK_PROFILE beats the config's active profile. Nonexistent profile
/// names still fail selection (the resolver is authoritative), so this test
/// uses existing profiles and verifies the shadowed chain in the JSON.
#[test]
fn explain_reports_profile_precedence_chain() {
    let dir = TempDir::new().unwrap();
    let config = "version = 1\nactive_profile = \"cfg\"\n\n[profiles.cfg]\nhost = \"https://cfg.example\"\nuser_id = \"u\"\nemail = \"u@cfg.example\"\n\n[profiles.env]\nhost = \"https://env.example\"\nuser_id = \"u\"\nemail = \"u@env.example\"\n";
    std::fs::write(dir.path().join("config.toml"), config).unwrap();

    let output = local(&dir)
        .env("HAMSTIK_PROFILE", "env")
        .args(["--profile", "env", "--json", "context", "explain"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let sources = body["values"]["profile"]["sources"].as_array().unwrap();
    assert_eq!(sources[0]["source"], "cli");
    assert_eq!(sources[0]["status"], "winner");
    assert_eq!(sources[1]["source"], "environment");
    assert_eq!(sources[1]["status"], "shadowed");
    assert_eq!(sources[2]["source"], "profile");
    assert_eq!(sources[2]["status"], "shadowed");
}

/// Behavior resolution (color/input/retry) is reported with the same
/// sources the commands actually use.
#[test]
fn explain_reports_behavior_resolution() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
        .args(["--json", "--no-retry", "context", "explain"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["behavior"]["retry"], "disabled (--no-retry)");
    assert!(
        !body["behavior"]["color"]["enabled"]
            .as_bool()
            .unwrap_or(true),
        "piped stdout disables color"
    );
    assert_eq!(
        body["behavior"]["input"],
        "no-input (prompts and editors disabled)"
    );
}

/// Human output names env vars, shows shadowed values, and states local-only.
#[test]
fn explain_human_output_is_actionable_and_local() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
        .env("HAMSTIK_TOKEN", "secret-token-value")
        .args(["--org", "cli-org", "context", "explain"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("env var: HAMSTIK_ORG"), "{text}");
    assert!(text.contains("(--flag)"), "{text}");
    assert!(text.contains("this report is fully local"), "{text}");
    assert!(
        !text.contains("secret-token-value"),
        "token leaked into human output"
    );
}
