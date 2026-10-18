// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The shared saved-query library (CLI-57): repository `.hamstik/queries/`
//! and config-dir `queries/` directories with documented precedence over
//! `queries.toml`, collision visibility, and `squeakql run`.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SEARCH: &str = "/api/v1/organizations/acme/work-items/search";
const VALIDATE: &str = "/api/v1/organizations/acme/squeakql/validate";

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
    cmd.arg("--no-input");
    cmd
}

fn valid_response() -> Value {
    json!({"valid": true, "languageVersion": 1, "errors": []})
}

/// Captures one POST body into a temp file and answers `valid`.
async fn mounting_capture(server: &MockServer, endpoint: &'static str, capture: &'static str) {
    Mock::given(method("POST"))
        .and(wiremock::matchers::path(endpoint))
        .respond_with(move |request: &wiremock::Request| {
            let body = String::from_utf8_lossy(&request.body).to_string();
            std::fs::write(capture, body).ok();
            ResponseTemplate::new(200).set_body_json(valid_response())
        })
        .mount(server)
        .await;
}

fn write_query(dir: &TempDir, relative: &str, expression: &str) {
    let path = dir.path().join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, expression).unwrap();
}

/// A repository query is discovered from a nested working directory and
/// runs identically for any checkout: `squeakql run` sends the file's exact
/// expression (after the documented newline policy).
#[tokio::test]
async fn repo_query_runs_from_a_nested_directory() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    write_query(
        &dir,
        "repo/.hamstik/queries/team-open.squeakql",
        "status = 'backlog'\n",
    );

    // The search endpoint answers with the documented search envelope.
    Mock::given(method("POST"))
        .and(path(SEARCH))
        .respond_with(move |request: &wiremock::Request| {
            let body = String::from_utf8_lossy(&request.body).to_string();
            std::fs::write("/tmp/kilo-sq-lib-run.json", body).ok();
            ResponseTemplate::new(200).set_body_json(json!({
                "items": [],
                "page": {"limit": 50, "hasMore": false, "nextCursor": null}
            }))
        })
        .mount(&server)
        .await;

    let mut cmd = base(&server, &dir);
    cmd.current_dir(dir.path().join("repo").join("sub"));
    std::fs::create_dir_all(dir.path().join("repo").join("sub")).unwrap();
    cmd.args(["--org", "acme", "squeakql", "run", "team-open"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("KEY"));

    let captured: Value =
        serde_json::from_slice(&std::fs::read("/tmp/kilo-sq-lib-run.json").unwrap()).unwrap();
    assert_eq!(captured["query"], json!("status = 'backlog'"));
}

/// Precedence is strict and collisions are visible, never silent: the repo
/// wins over the config-dir library, which wins over `queries.toml`; `list`
/// reports every holding source and the winner.
#[tokio::test]
async fn precedence_is_strict_and_collisions_are_visible() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    write_query(
        &dir,
        ".hamstik/queries/shared.squeakql",
        "status = 'from-repo'",
    );
    write_query(&dir, "queries/shared.squeakql", "status = 'from-config'");
    write_query(&dir, "queries/only-dir.squeakql", "status = 'only-dir'");
    // queries.toml with the same name plus a distinct entry.
    std::fs::write(
        dir.path().join("queries.toml"),
        "version = 1\n[queries.shared]\nquery = \"status = 'from-toml'\"\n[queries.only-toml]\nquery = \"status = 'only-toml'\"\n",
    )
    .unwrap();

    let output = base(&server, &dir)
        .args(["--json", "squeakql", "list"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let queries = body["queries"].as_array().unwrap();
    let by_name = |name: &str| {
        queries
            .iter()
            .find(|q| q["name"] == json!(name))
            .unwrap()
            .clone()
    };

    let shared = by_name("shared");
    assert_eq!(shared["source"], json!("repo"));
    assert_eq!(
        shared["sources"],
        json!(["repo", "config-dir", "saved-file"])
    );
    let only_dir = by_name("only-dir");
    assert_eq!(only_dir["source"], json!("config-dir"));
    let only_toml = by_name("only-toml");
    assert_eq!(only_toml["source"], json!("saved-file"));
    assert_eq!(only_toml["query"], json!("status = 'only-toml'"));

    // `show` resolves through the same precedence.
    base(&server, &dir)
        .args(["--json", "squeakql", "show", "shared"])
        .assert()
        .code(0)
        .stdout(predicate::function(|stdout: &str| {
            let body: Value = serde_json::from_str(stdout).unwrap();
            body["query"] == json!("status = 'from-repo'")
                && body["source"] == json!("repo")
                && body["sources"] == json!(["repo", "config-dir", "saved-file"])
        }));
}

/// Without a repository directory, the config-dir library serves; without
/// either, `queries.toml` keeps working unchanged.
#[tokio::test]
async fn config_dir_and_saved_file_fallbacks() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    write_query(&dir, "queries/user-query.squeakql", "status = 'user'");
    std::fs::write(
        dir.path().join("queries.toml"),
        "version = 1\n[queries.toml-only]\nquery = \"status = 'toml'\"\n",
    )
    .unwrap();

    base(&server, &dir)
        .args(["--json", "squeakql", "show", "user-query"])
        .assert()
        .code(0)
        .stdout(predicate::function(|stdout: &str| {
            let body: Value = serde_json::from_str(stdout).unwrap();
            body["query"] == json!("status = 'user'") && body["source"] == json!("config-dir")
        }));

    base(&server, &dir)
        .args(["--json", "squeakql", "show", "toml-only"])
        .assert()
        .code(0)
        .stdout(predicate::function(|stdout: &str| {
            let body: Value = serde_json::from_str(stdout).unwrap();
            body["query"] == json!("status = 'toml'") && body["source"] == json!("saved-file")
        }));
}

/// `squeakql validate --saved NAME` resolves through the library too, and a
/// missing name fails with the actionable listing hint.
#[tokio::test]
async fn validate_resolves_the_library_and_missing_names_fail_clearly() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    write_query(
        &dir,
        ".hamstik/queries/valid.squeakql",
        "status = 'backlog'",
    );

    mounting_capture(&server, VALIDATE, "/tmp/kilo-sq-lib-validate.json").await;

    base(&server, &dir)
        .args(["--org", "acme", "squeakql", "validate", "--saved", "valid"])
        .assert()
        .code(0);
    let captured: Value =
        serde_json::from_slice(&std::fs::read("/tmp/kilo-sq-lib-validate.json").unwrap()).unwrap();
    assert_eq!(captured["query"], json!("status = 'backlog'"));

    base(&server, &dir)
        .args(["--org", "acme", "squeakql", "validate", "--saved", "absent"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no saved query named"))
        .stderr(predicate::str::contains("squeakql list"));
}

/// Files whose stem is not a valid query name are ignored, and the
/// documented newline policy applies to library files.
#[tokio::test]
async fn invalid_stems_are_ignored_and_newline_policy_applies() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    write_query(
        &dir,
        ".hamstik/queries/good-name.squeakql",
        "status = 'ok'\n",
    );
    write_query(&dir, ".hamstik/queries/bad name.squeakql", "status = 'bad'");
    write_query(&dir, ".hamstik/queries/notes.md", "not a query");

    let output = base(&server, &dir)
        .args(["--json", "squeakql", "list"])
        .output()
        .unwrap();
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let names: Vec<&str> = body["queries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["good-name"]);

    mounting_capture(&server, VALIDATE, "/tmp/kilo-sq-lib-policy.json").await;
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "squeakql",
            "validate",
            "--saved",
            "good-name",
        ])
        .assert()
        .code(0);
    let captured: Value =
        serde_json::from_slice(&std::fs::read("/tmp/kilo-sq-lib-policy.json").unwrap()).unwrap();
    assert_eq!(captured["query"], json!("status = 'ok'"));
}
