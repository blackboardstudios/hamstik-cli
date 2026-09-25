// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `hamstik config` contract: `path`/`get`/`set`/`list`/`unset` round-trip the
//! documented non-secret keys with deterministic output and stable exit codes,
//! unknown keys and bad preference spellings fail with actionable errors,
//! credential-like values are refused, and a write never damages the profiles
//! (or context defaults) that share the file.
//!
//! These commands touch no network: the API boundary is irrelevant here, so the
//! tests point `HAMSTIK_HOST` at a closed port to prove nothing is requested.

use std::fs;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

const CONFIGURATION: i32 = 10;

/// Base command with a throwaway config, no credential, and an audit log that
/// stays inside the test's temporary directory.
fn base(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", config_path(dir));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env("HAMSTIK_HOST", "http://127.0.0.1:9");
    cmd.env_remove("HAMSTIK_TOKEN");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn config_path(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("config.toml")
}

/// Runs `hamstik config <args>` and returns the completed output.
fn run(dir: &TempDir, args: &[&str]) -> std::process::Output {
    base(dir).args(args).output().unwrap()
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// Two profiles with context defaults, so a write can be shown to preserve
/// everything it did not touch.
const TWO_PROFILES: &str = r#"version = 2
active_profile = "one"

[profiles.one]
host = "https://one.test"
user_id = "u1"
email = "one@example.com"
default_organization = "one-org"
default_project = "ONE"

[profiles.two]
host = "https://two.test"
user_id = "u2"
email = "two@example.com"
"#;

fn write_config(dir: &TempDir, contents: &str) {
    fs::write(config_path(dir), contents).unwrap();
}

fn assert_fails(output: &std::process::Output, needle: &str) {
    assert_eq!(
        output.status.code(),
        Some(CONFIGURATION),
        "config failures use exit code 10: stdout={} stderr={}",
        stdout(output),
        stderr(output)
    );
    let text = format!("{}{}", stdout(output), stderr(output));
    assert!(text.contains(needle), "expected {needle:?} in: {text}");
    assert!(
        !text.contains("panicked"),
        "config errors are typed failures, never panics: {text}"
    );
}

/// Runs `args` with `--json` and parses the JSON error document on stderr.
fn json_error(dir: &TempDir, args: &[&str]) -> (i32, Value) {
    let mut argv: Vec<&str> = args.to_vec();
    argv.push("--json");
    let output = run(dir, &argv);
    let code = output.status.code().unwrap();
    let body: Value = serde_json::from_slice(&output.stderr).unwrap_or_else(|err| {
        panic!(
            "--json errors are a JSON document on stderr ({err}): stdout={} stderr={}",
            stdout(&output),
            stderr(&output)
        )
    });
    (code, body)
}

#[test]
fn path_reports_the_active_configuration_file() {
    let dir = TempDir::new().unwrap();
    let expected = config_path(&dir);

    let output = run(&dir, &["config", "path"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), expected.to_string_lossy());

    let output = run(&dir, &["config", "path", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["path"], expected.to_string_lossy().as_ref());
}

#[test]
fn missing_config_is_empty_and_lists_nothing() {
    let dir = TempDir::new().unwrap();

    let output = run(&dir, &["config", "list"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "no configuration values are set");

    let output = run(&dir, &["config", "list", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body, serde_json::json!({}));

    // An unset key is a successful read, not an error.
    let output = run(&dir, &["config", "get", "editor"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "editor is not set");

    let output = run(&dir, &["config", "get", "editor", "--json"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["key"], "editor");
    assert_eq!(body["value"], "");
}

#[test]
fn set_get_list_unset_round_trip_every_preference_key() {
    let dir = TempDir::new().unwrap();

    let set = run(&dir, &["config", "set", "editor", "vim"]);
    assert_eq!(set.status.code(), Some(0), "{}", stderr(&set));
    assert_eq!(stdout(&set).trim(), "set editor=vim");

    // Every documented preference key round-trips through the same surface.
    for (key, value) in [
        ("pager", "less"),
        ("output", "jsonl"),
        ("git_branch_template", "hamstik/{key}"),
        ("audit_log", "false"),
    ] {
        let set = run(&dir, &["config", "set", key, value]);
        assert_eq!(set.status.code(), Some(0), "{}", stderr(&set));
        let get = run(&dir, &["config", "get", key]);
        assert_eq!(get.status.code(), Some(0), "{}", stderr(&get));
        assert_eq!(stdout(&get).trim(), value, "{key} must round-trip");
    }

    // `list` emits a fixed key order and only configured values.
    let listed = run(&dir, &["config", "list"]);
    assert_eq!(listed.status.code(), Some(0), "{}", stderr(&listed));
    assert_eq!(
        stdout(&listed),
        "editor=vim\npager=less\noutput=jsonl\ngit_branch_template=hamstik/{key}\naudit_log=false\n"
    );

    let listed = run(&dir, &["config", "list", "--json"]);
    let body: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(body["editor"], "vim");
    assert_eq!(body["output"], "jsonl");
    assert_eq!(body["audit_log"], "false");

    for key in [
        "editor",
        "pager",
        "output",
        "git_branch_template",
        "audit_log",
    ] {
        let unset = run(&dir, &["config", "unset", key]);
        assert_eq!(unset.status.code(), Some(0), "{}", stderr(&unset));
        assert_eq!(stdout(&unset).trim(), format!("unset {key}"));

        let get = run(&dir, &["config", "get", key]);
        assert_eq!(get.status.code(), Some(0), "{}", stderr(&get));
        assert_eq!(stdout(&get).trim(), format!("{key} is not set"));
    }
}

#[test]
fn json_envelopes_are_stable_for_writes() {
    let dir = TempDir::new().unwrap();

    let set = run(&dir, &["config", "set", "editor", "emacs", "--json"]);
    assert_eq!(set.status.code(), Some(0), "{}", stderr(&set));
    let body: Value = serde_json::from_slice(&set.stdout).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"key": "editor", "value": "emacs", "set": true})
    );

    let get = run(&dir, &["config", "get", "editor", "--json"]);
    let body: Value = serde_json::from_slice(&get.stdout).unwrap();
    assert_eq!(body, serde_json::json!({"key": "editor", "value": "emacs"}));

    let unset = run(&dir, &["config", "unset", "editor", "--json"]);
    let body: Value = serde_json::from_slice(&unset.stdout).unwrap();
    assert_eq!(body, serde_json::json!({"key": "editor", "unset": true}));

    // Unsetting a key that was never set says so instead of lying.
    let again = run(&dir, &["config", "unset", "editor", "--json"]);
    assert_eq!(again.status.code(), Some(0), "{}", stderr(&again));
    let body: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(body, serde_json::json!({"key": "editor", "unset": false}));
}

#[test]
fn unset_of_an_unset_key_rewrites_nothing() {
    let dir = TempDir::new().unwrap();
    write_config(&dir, TWO_PROFILES);
    let before = fs::read_to_string(config_path(&dir)).unwrap();

    let output = run(&dir, &["config", "unset", "editor"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "editor was not set");
    assert_eq!(
        fs::read_to_string(config_path(&dir)).unwrap(),
        before,
        "a no-op unset must not rewrite the file"
    );
}

#[test]
fn unknown_keys_fail_with_the_valid_key_list() {
    let dir = TempDir::new().unwrap();

    let output = run(&dir, &["config", "set", "colour", "blue"]);
    assert_fails(&output, "unknown configuration key: colour");

    let output = run(&dir, &["config", "get", "editor.enabled"]);
    assert_fails(&output, "unknown configuration key");

    // The error lists every accepted key, so the next attempt is obvious.
    let output = run(&dir, &["config", "unset", "credential"]);
    assert_fails(&output, "valid keys:");
    assert_fails(&output, "git_branch_template");

    // Structured mode keeps the same exit code and reports a JSON error.
    let (code, body) = json_error(&dir, &["config", "get", "nope"]);
    assert_eq!(code, CONFIGURATION);
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("unknown configuration key"), "{body}");
    assert!(message.contains("audit_log"), "{body}");
}

#[test]
fn credential_like_values_are_refused_with_real_alternatives() {
    let dir = TempDir::new().unwrap();

    let output = run(&dir, &["config", "set", "editor", "pat-super-secret-token"]);
    assert_fails(&output, "credential");
    assert_fails(&output, "HAMSTIK_TOKEN");
    assert_fails(&output, "auth login");
    assert!(
        !config_path(&dir).exists(),
        "a refused write must not create the config file"
    );

    let (code, body) = json_error(&dir, &["config", "set", "pager", "PasswordManager"]);
    assert_eq!(code, CONFIGURATION);
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("credential"),
        "{body}"
    );
    assert!(!config_path(&dir).exists());
}

#[test]
fn local_preference_values_are_validated_before_they_are_written() {
    let dir = TempDir::new().unwrap();

    // Output modes the CLI does not implement are spelling mistakes.
    let output = run(&dir, &["config", "set", "output", "yaml"]);
    assert_fails(&output, "invalid output preference: yaml");
    assert_fails(&output, "human, json, jsonl, tsv, quiet");

    let output = run(&dir, &["config", "set", "audit_log", "maybe"]);
    assert_fails(&output, "audit_log must be true or false");

    // Empty values are `unset`'s job, not a stored empty string.
    let output = run(&dir, &["config", "set", "editor", ""]);
    assert_fails(&output, "must not be empty");
    assert_fails(&output, "config unset editor");

    // Control characters could smuggle a second argument into a spawned
    // command line, and absurd length would bloat the shared file.
    let output = run(&dir, &["config", "set", "pager", "less\nrm -rf /tmp"]);
    assert_fails(&output, "control characters");
    let huge = "v".repeat(1_100);
    let output = run(&dir, &["config", "set", "editor", &huge]);
    assert_fails(&output, "at most 1024 characters");

    assert!(
        !config_path(&dir).exists(),
        "rejected values must never reach disk"
    );

    // Valid spellings still work, including booleans written loosely.
    for (key, value) in [("output", "quiet"), ("audit_log", "TRUE")] {
        let output = run(&dir, &["config", "set", key, value]);
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    }
    let output = run(&dir, &["config", "get", "audit_log"]);
    assert_eq!(stdout(&output).trim(), "true");
}

#[test]
fn writes_preserve_unrelated_profiles_and_context_defaults() {
    let dir = TempDir::new().unwrap();
    write_config(&dir, TWO_PROFILES);

    let output = run(&dir, &["config", "set", "editor", "vim"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let after = fs::read_to_string(config_path(&dir)).unwrap();
    for needle in [
        "active_profile = \"one\"",
        "[profiles.one]",
        "host = \"https://one.test\"",
        "user_id = \"u1\"",
        "email = \"one@example.com\"",
        "default_organization = \"one-org\"",
        "default_project = \"ONE\"",
        "[profiles.two]",
        "host = \"https://two.test\"",
        "user_id = \"u2\"",
    ] {
        assert!(after.contains(needle), "lost {needle:?} in:\n{after}");
    }
}

#[test]
fn profile_scoped_keys_only_touch_the_active_profile() {
    let dir = TempDir::new().unwrap();
    write_config(&dir, TWO_PROFILES);

    // Reads follow the active profile.
    let output = run(&dir, &["config", "get", "profile"]);
    assert_eq!(stdout(&output).trim(), "one");
    let output = run(&dir, &["config", "get", "organization"]);
    assert_eq!(stdout(&output).trim(), "one-org");

    // Switching to a profile that does not exist fails instead of creating one.
    let output = run(&dir, &["config", "set", "profile", "nope"]);
    assert_fails(&output, "no such profile: nope");

    // A switch is explicit and moves the scope writes land in; profile two has
    // no context defaults yet.
    let output = run(&dir, &["config", "set", "profile", "two"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let output = run(&dir, &["config", "get", "organization"]);
    assert_eq!(stdout(&output).trim(), "organization is not set");

    // `organization`/`project` are written to the active profile only, so no
    // other profile's context defaults can drift.
    let output = run(&dir, &["config", "set", "organization", "two-org"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let after = fs::read_to_string(config_path(&dir)).unwrap();
    assert!(
        after.contains("default_organization = \"two-org\""),
        "{after}"
    );
    assert!(
        after.contains("[profiles.one]\nhost = \"https://one.test\"")
            && after.contains("default_organization = \"one-org\"")
            && after.contains("default_project = \"ONE\""),
        "profile one must be untouched:\n{after}"
    );
    let output = run(&dir, &["config", "get", "organization"]);
    assert_eq!(stdout(&output).trim(), "two-org");

    let output = run(&dir, &["config", "unset", "organization"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let after = fs::read_to_string(config_path(&dir)).unwrap();
    assert!(!after.contains("two-org"), "{after}");
    assert!(
        after.contains("default_organization = \"one-org\""),
        "{after}"
    );
    let output = run(&dir, &["config", "unset", "organization"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "organization was not set");

    let output = run(&dir, &["config", "unset", "profile"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let output = run(&dir, &["config", "unset", "profile"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "profile was not set");
    assert!(
        fs::read_to_string(config_path(&dir))
            .unwrap()
            .contains("[profiles.two]"),
        "clearing the active profile must never delete profiles"
    );
}

#[test]
fn strict_configuration_policy_still_applies_to_config_commands() {
    let dir = TempDir::new().unwrap();

    // Unknown fields and malformed TOML name the file and change nothing.
    write_config(&dir, "version = 2\nmystery = true\n");
    let before = fs::read_to_string(config_path(&dir)).unwrap();
    let output = run(&dir, &["config", "list"]);
    assert_fails(&output, "mystery");
    let output = run(&dir, &["config", "set", "editor", "vim"]);
    assert_fails(&output, &config_path(&dir).display().to_string());
    assert_eq!(fs::read_to_string(config_path(&dir)).unwrap(), before);

    write_config(&dir, "version = 99\n");
    let output = run(&dir, &["config", "get", "editor"]);
    assert_fails(&output, "newer than this CLI");

    write_config(&dir, "[[broken\n");
    let output = run(&dir, &["config", "unset", "editor"]);
    assert_fails(&output, "invalid configuration");
}
