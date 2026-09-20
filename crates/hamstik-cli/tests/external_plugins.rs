// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! External subcommand plugin tests (CLI-33).
//!
//! Verifies git-style plugin discovery, invocation, environment variable
//! passing, and manifest inclusion.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Returns a base command with an isolated config directory.
fn base(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.current_dir(dir.path());
    cmd
}

/// Create a test plugin executable in a temp directory and return its path.
fn make_plugin(dir: &TempDir, name: &str, script: &str) -> std::path::PathBuf {
    let plugin_path = dir.path().join(format!("hamstik-{}", name));
    std::fs::write(&plugin_path, script).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&plugin_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    plugin_path
}

/// Test that an unknown subcommand (no matching plugin) returns a clear error.
#[test]
fn unknown_subcommand_returns_plugin_not_found_error() {
    let dir = TempDir::new().unwrap();
    let mut cmd = base(&dir);
    cmd.arg("nonexistent");
    cmd.assert()
        .code(2)
        .stderr(predicate::str::contains("external plugin not found"))
        .stderr(predicate::str::contains("hamstik-nonexistent"));
}

/// Test that a plugin on PATH is discovered and invoked.
#[test]
fn plugin_on_path_is_invoked() {
    let dir = TempDir::new().unwrap();
    make_plugin(
        &dir,
        "hello",
        r#"#!/bin/sh
echo "Hello from plugin"
echo "PLUGIN=$HAMSTIK_PLUGIN"
"#,
    );
    let mut cmd = base(&dir);
    cmd.current_dir(&dir);
    cmd.env("PATH", dir.path().to_str().unwrap());
    cmd.arg("hello");
    let output = cmd.assert().code(0).get_output().clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Hello from plugin"));
    assert!(stdout.contains("PLUGIN=hello"));
}

/// Test that the plugin receives context environment variables, while
/// credential-bearing variables set in the parent environment never cross
/// into the plugin (the raw PAT is stripped before spawning).
#[test]
fn plugin_receives_context_env_vars() {
    let dir = TempDir::new().unwrap();
    make_plugin(
        &dir,
        "envcheck",
        r#"#!/bin/sh
echo "HOST=$HAMSTIK_HOST"
echo "PROFILE=$HAMSTIK_PROFILE"
echo "ORG=$HAMSTIK_ORG"
echo "PROJECT=$HAMSTIK_PROJECT"
echo "FORMAT=$HAMSTIK_FORMAT"
echo "PLUGIN=$HAMSTIK_PLUGIN"
echo "NO_COLOR=$HAMSTIK_NO_COLOR"
echo "NO_INPUT=$HAMSTIK_NO_INPUT"
echo "NO_RETRY=$HAMSTIK_NO_RETRY"
echo "QUIET=$HAMSTIK_QUIET"
echo "VERBOSE=$HAMSTIK_VERBOSE"
echo "DRY_RUN=$HAMSTIK_DRY_RUN"
echo "TOKEN=$HAMSTIK_TOKEN"
echo "AUTH=$Authorization"
echo "APIKEY=$HAMSTIK_API_KEY"
echo "SECRET=$HAMSTIK_SECRET"
echo "PASSWORD=$HAMSTIK_PASSWORD"
"#,
    );
    let mut cmd = base(&dir);
    // These must not reach the plugin.
    cmd.env("HAMSTIK_TOKEN", "test")
        .env("Authorization", "Bearer")
        .env("HAMSTIK_API_KEY", "apikey")
        .env("HAMSTIK_SECRET", "secret")
        .env("HAMSTIK_PASSWORD", "pw");
    cmd.current_dir(&dir);
    cmd.env("PATH", dir.path().to_str().unwrap());
    cmd.arg("envcheck");
    let output = cmd.assert().code(0).get_output().clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("PLUGIN=envcheck"));
    assert!(stdout.contains("FORMAT=human"));
    // Credentials must NOT be passed to the plugin.
    assert!(!stdout.contains("TOKEN=test"));
    assert!(!stdout.contains("AUTH=Bearer"));
    assert!(!stdout.contains("APIKEY=apikey"));
    assert!(!stdout.contains("SECRET=secret"));
    assert!(!stdout.contains("PASSWORD=pw"));
}

/// Test that the plugin receives flags as env vars.
#[test]
fn plugin_receives_flag_env_vars() {
    let dir = TempDir::new().unwrap();
    make_plugin(
        &dir,
        "flagcheck",
        r#"#!/bin/sh
echo "FORMAT=$HAMSTIK_FORMAT"
echo "NO_COLOR=$HAMSTIK_NO_COLOR"
echo "NO_INPUT=$HAMSTIK_NO_INPUT"
echo "NO_RETRY=$HAMSTIK_NO_RETRY"
echo "QUIET=$HAMSTIK_QUIET"
echo "VERBOSE=$HAMSTIK_VERBOSE"
"#,
    );
    let mut cmd = base(&dir);
    cmd.current_dir(&dir);
    cmd.env("PATH", dir.path().to_str().unwrap());
    cmd.arg("--json")
        .arg("--no-color")
        .arg("--no-input")
        .arg("--no-retry")
        .arg("--verbose")
        .arg("flagcheck");
    let output = cmd.assert().code(0).get_output().clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("FORMAT=json"));
    assert!(stdout.contains("NO_COLOR=1"));
    assert!(stdout.contains("NO_INPUT=1"));
    assert!(stdout.contains("NO_RETRY=1"));
    assert!(stdout.contains("VERBOSE=1"));
}

/// Test that plugin arguments are forwarded correctly.
#[test]
fn plugin_receives_arguments() {
    let dir = TempDir::new().unwrap();
    make_plugin(
        &dir,
        "args",
        r#"#!/bin/sh
echo "ARGS=$@"
"#,
    );
    let mut cmd = base(&dir);
    cmd.current_dir(&dir);
    cmd.env("PATH", dir.path().to_str().unwrap());
    cmd.arg("args")
        .arg("hello")
        .arg("world")
        .arg("--flag=value");
    let output = cmd.assert().code(0).get_output().clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ARGS=hello world --flag=value"));
}

/// Test that the commands manifest includes external plugins.
#[test]
fn manifest_includes_external_plugins() {
    let dir = TempDir::new().unwrap();
    make_plugin(
        &dir,
        "testplugin",
        r#"#!/bin/sh
echo "Test plugin help text"
"#,
    );
    let mut cmd = base(&dir);
    cmd.current_dir(&dir);
    cmd.env("PATH", dir.path().to_str().unwrap());
    cmd.arg("commands").arg("--json");
    let output = cmd.assert().code(0).get_output().clone();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let commands = body["commands"].as_array().unwrap();
    let ext: Vec<&Value> = commands
        .iter()
        .filter(|c| c["external"].as_bool() == Some(true))
        .collect();
    assert!(
        !ext.is_empty(),
        "external plugins should appear in the manifest"
    );
    let testplugin = ext
        .iter()
        .find(|c| c["command"] == json!("hamstik testplugin"));
    assert!(
        testplugin.is_some(),
        "hamstik testplugin should be in the manifest"
    );
    let entry = testplugin.unwrap();
    assert_eq!(entry["external"], json!(true));
    assert_eq!(entry["capabilities"]["json"], json!(true));
    assert_eq!(entry["capabilities"]["noInput"], json!(true));
}

/// Test that the help output lists external plugins, labeled as external,
/// without executing them (introspection must not run plugin code).
#[test]
fn help_lists_external_plugins() {
    let dir = TempDir::new().unwrap();
    make_plugin(
        &dir,
        "helptest",
        r#"#!/bin/sh
touch "$PWD/ran"
echo "This is the helptest plugin"
"#,
    );
    let mut cmd = base(&dir);
    cmd.current_dir(&dir);
    cmd.env("PATH", dir.path().to_str().unwrap());
    cmd.arg("--help");
    let output = cmd.assert().code(0).get_output().clone();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("External plugins:"));
    assert!(stdout.contains("hamstik helptest"));
    assert!(stdout.contains("(external plugin)"));
    // The plugin must not have executed during introspection.
    assert!(!dir.path().join("ran").exists());
}

/// Test that the plugin exit code is propagated.
#[test]
fn plugin_exit_code_is_propagated() {
    let dir = TempDir::new().unwrap();
    make_plugin(
        &dir,
        "fail",
        r#"#!/bin/sh
exit 42
"#,
    );
    let mut cmd = base(&dir);
    cmd.current_dir(&dir);
    cmd.env("PATH", dir.path().to_str().unwrap());
    cmd.arg("fail");
    cmd.assert().code(42);
}

/// Test that `commands --json` never executes discovered plugins, and lists
/// them with the `external` flag.
#[test]
fn commands_manifest_does_not_execute_plugins() {
    let dir = TempDir::new().unwrap();
    make_plugin(
        &dir,
        "marker",
        r#"#!/bin/sh
touch "$PWD/ran"
echo "hi"
"#,
    );
    let mut cmd = base(&dir);
    cmd.current_dir(&dir);
    cmd.env("PATH", dir.path().to_str().unwrap());
    cmd.arg("commands").arg("--json");
    cmd.assert().code(0);
    // Introspection must not have run the plugin.
    assert!(!dir.path().join("ran").exists());
}

/// Test that the plugin exit code of 0 is treated as success.
#[test]
fn plugin_success_exit_code() {
    let dir = TempDir::new().unwrap();
    make_plugin(
        &dir,
        "success",
        r#"#!/bin/sh
exit 0
"#,
    );
    let mut cmd = base(&dir);
    cmd.current_dir(&dir);
    cmd.env("PATH", dir.path().to_str().unwrap());
    cmd.arg("success");
    cmd.assert().code(0);
}
