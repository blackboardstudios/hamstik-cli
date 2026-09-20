// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Configuration-drift advisory: a resolved `.hamstik.toml` Organization that
//! disagrees with the authenticated user's `GET /me` memberships warns on
//! stderr without changing the exit code, and stays silent when the values
//! agree or membership data is unavailable.

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn me_json(member_slugs: &[&str]) -> Value {
    let organizations: Vec<Value> = member_slugs
        .iter()
        .enumerate()
        .map(|(index, slug)| {
            json!({
                "id": format!("{:08}-0000-4000-8000-000000000000", index + 1),
                "slug": slug,
                "name": slug.to_uppercase(),
                "username": "steven",
            })
        })
        .collect();
    json!({
        "id": "11111111-1111-4111-8111-111111111111",
        "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg",
        "name": "Steven",
        "email": "steven@example.com",
        "authentication": {
            "type": "pat",
            "credentialId": "22222222-2222-4222-8222-222222222222",
            "credentialName": "cli-pat",
            "scopes": ["work:read", "work:write"],
            "expiresAt": "2030-01-01T00:00:00Z",
        },
        "defaultOrganization": null,
        "organizations": organizations,
    })
}

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
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

fn write_context(dir: &TempDir, organization: &str) {
    std::fs::write(
        dir.path().join(".hamstik.toml"),
        format!("version = 1\norganization = \"{organization}\"\n"),
    )
    .unwrap();
}

/// A mismatching resolved Organization warns and names both the configured
/// value and the actual memberships; the command still succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mismatch_warns_and_names_both_values() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json(&["acme", "beta"])))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    write_context(&dir, "other-org");

    let output = base(&server, &dir).arg("me").output().unwrap();
    assert!(output.status.success(), "exit code must be unchanged");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("other-org"), "{stderr}");
    assert!(stderr.contains("acme"), "{stderr}");
    assert!(stderr.contains("beta"), "{stderr}");
    assert!(stderr.contains("configuration drift"), "{stderr}");
}

/// When the resolved Organization is a member, no advisory is emitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn matching_context_is_silent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json(&["acme"])))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    write_context(&dir, "acme");

    let output = base(&server, &dir).arg("me").output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("configuration drift"), "{stderr}");
}

/// The advisory is also emitted on a mutation path that already fetches
/// `GET /me` — `work create --assignee me` — before any write, and the
/// `--dry-run` preview still succeeds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mutation_path_warns_before_write() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json(&["acme"])))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join(".hamstik.toml"),
        "version = 1\norganization = \"other-org\"\nproject = \"HAM\"\n",
    )
    .unwrap();

    let output = base(&server, &dir)
        .args([
            "--dry-run",
            "work",
            "create",
            "--title",
            "Example",
            "--assignee",
            "me",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "dry-run must succeed");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("other-org"), "{stderr}");
    assert!(stderr.contains("acme"), "{stderr}");
    assert!(stderr.contains("configuration drift"), "{stderr}");
}

/// Without membership data (empty `organizations`, as an offline or
/// narrowly-scoped token produces) the check degrades to no warning.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_membership_data_is_silent() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(me_json(&[])))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    write_context(&dir, "other-org");

    let output = base(&server, &dir).arg("me").output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("configuration drift"), "{stderr}");
}
