// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! CLI-38 integration tests: non-interactive ambiguity paths fail fast.
//!
//! `assert_cmd` runs the binary with piped stdin/stdout, so every invocation
//! here is non-TTY. The acceptance contract is that an unknown Organization,
//! Project, or Work Item reference still fails with the server's `NOT_FOUND`
//! (exit 5), and that the CLI never lists candidates or attempts a prompt.
//! Counting the wiremock requests proves the interactive disambiguation path
//! was never entered: a candidate list would add a request.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
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

fn not_found() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({
        "error": {"code": "NOT_FOUND", "message": "no such resource"}
    }))
}

fn org_json(slug: &str) -> serde_json::Value {
    json!({
        "id": "o1", "slug": slug, "name": slug, "role": "member",
        "description": null, "plan": "pro", "isDefault": false, "suspended": false,
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-01T00:00:00Z"
    })
}

/// An unknown Organization reference fails with the exact lookup only: no
/// candidate list request, no prompt, and the same `NOT_FOUND` surface.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_org_reference_fails_fast_without_listing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/missing"))
        .respond_with(not_found())
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["context", "set", "--org", "missing"])
        .assert()
        .code(5)
        .stderr(predicate::str::contains("no such resource"));

    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "only the exact Organization lookup may run"
    );
}

/// An unknown Project reference fails after the Organization validates; again
/// no candidate list request is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_project_reference_fails_fast_without_listing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme"))
        .respond_with(ResponseTemplate::new(200).set_body_json(org_json("acme")))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/missing"))
        .respond_with(not_found())
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args(["context", "set", "--org", "acme", "--project", "missing"])
        .assert()
        .code(5)
        .stderr(predicate::str::contains("no such resource"));

    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "only the Organization and Project exact lookups may run"
    );
}

/// An unknown Work Item reference fails on the exact `get` alone; the picker
/// would otherwise list the Project's Work Items.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_work_item_reference_fails_fast_without_listing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/APP/work-items/MISSING",
        ))
        .respond_with(not_found())
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "APP",
            "work",
            "context",
            "MISSING",
        ])
        .assert()
        .code(5)
        .stderr(predicate::str::contains("no such resource"));

    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "only the exact Work Item lookup may run"
    );
}

/// `--no-input`, `--json`, and `--quiet` keep the fail-fast guarantee even
/// when a terminal might otherwise be interactive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_non_interactive_flags_never_prompt() {
    for flag in ["--no-input", "--json", "--quiet"] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/organizations/missing"))
            .respond_with(not_found())
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        base(&server, &dir)
            .args([flag, "context", "set", "--org", "missing"])
            .assert()
            .code(5);

        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "{flag} must not list candidates or prompt"
        );
    }
}
