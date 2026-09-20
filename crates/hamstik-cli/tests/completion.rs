// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Dynamic completion behavior and generated-script regression tests.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::process::Command as ProcessCommand;

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const INTERNAL_COMMAND: &str = "_hamstik_dyn_complete";

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut command = Command::cargo_bin("hamstik").expect("hamstik binary");
    command
        .env("HAMSTIK_CONFIG", dir.path().join("config.toml"))
        .env("HAMSTIK_HOST", server.uri())
        .env("HAMSTIK_TOKEN", "completion-test-token")
        .env_remove("HAMSTIK_PROFILE")
        .env_remove("HAMSTIK_ORG")
        .env_remove("HAMSTIK_PROJECT")
        .current_dir(dir.path())
        .arg("--no-retry");
    command
}

fn page(items: Value) -> Value {
    json!({
        "items": items,
        "page": {"limit": 100, "hasMore": false, "nextCursor": null}
    })
}

fn page_with_next(items: Value) -> Value {
    json!({
        "items": items,
        "page": {"limit": 100, "hasMore": true, "nextCursor": "must-not-follow"}
    })
}

fn organization(slug: &str) -> Value {
    json!({
        "id": format!("org-{slug}"),
        "slug": slug,
        "name": slug,
        "suspended": false
    })
}

fn project(key: &str, archived: bool) -> Value {
    json!({
        "id": format!("project-{key}"),
        "organizationId": "org-acme",
        "key": key,
        "name": key,
        "description": null,
        "color": "#123456",
        "revision": 1,
        "archivedAt": archived.then_some("2026-01-01T00:00:00Z"),
        "createdAt": "2026-01-01T00:00:00Z",
        "updatedAt": "2026-01-01T00:00:00Z"
    })
}

fn project_work_item(key: &str) -> Value {
    json!({"id": format!("item-{key}"), "key": key, "revision": 1})
}

fn organization_work_item(key: &str) -> Value {
    json!({
        "id": format!("item-{key}"),
        "key": key,
        "revision": 1,
        "project": {"id": "project-APP", "key": "APP", "name": "APP", "color": "#123456"},
        "organization": {"id": "org-acme", "slug": "acme", "name": "acme"}
    })
}

fn label(name: &str) -> Value {
    json!({
        "id": format!("label-{name}"),
        "name": name,
        "color": "#123456",
        "createdAt": "2026-01-01T00:00:00Z"
    })
}

fn assert_silent_success(output: std::process::Output) {
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert!(output.stdout.is_empty(), "stdout: {:?}", output.stdout);
    assert!(output.stderr.is_empty(), "stderr: {:?}", output.stderr);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn organizations_are_filtered_sorted_and_deduplicated_from_one_page() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations"))
        .and(query_param("limit", "100"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(page_with_next(json!([
                organization("zeta"),
                organization("acme"),
                organization("acme"),
                organization("alpha")
            ]))),
        )
        .mount(&server)
        .await;

    let output = base(&server, &dir)
        .args([INTERNAL_COMMAND, "org", "a"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "acme\nalpha\n");
    assert!(output.stderr.is_empty());
    assert!(
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|request| request.method.as_str() == "GET")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projects_union_archived_and_active_pages() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    for (archived, items) in [
        (
            "false",
            json!([project("APP", false), project("BASE", false)]),
        ),
        ("true", json!([project("APP", true), project("OLD", true)])),
    ] {
        Mock::given(method("GET"))
            .and(path("/api/v1/organizations/acme/projects"))
            .and(query_param("archived", archived))
            .and(query_param("limit", "100"))
            .respond_with(ResponseTemplate::new(200).set_body_json(page(items)))
            .mount(&server)
            .await;
    }

    let output = base(&server, &dir)
        .args(["--org", "acme", INTERNAL_COMMAND, "project", ""])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "APP\nBASE\nOLD\n"
    );
    assert!(output.stderr.is_empty());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request.method.as_str() == "GET")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_item_completion_scopes_to_project_or_organization_and_forwards_fields() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/APP/work-items"))
        .and(query_param("fields", "key"))
        .and(query_param("limit", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            project_work_item("APP-2"),
            project_work_item("APP-1")
        ]))))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/work-items"))
        .and(query_param("fields", "key"))
        .and(query_param("limit", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            organization_work_item("ORG-2"),
            organization_work_item("ORG-1")
        ]))))
        .mount(&server)
        .await;

    let project_output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "APP",
            INTERNAL_COMMAND,
            "work-item-key",
            "APP-",
        ])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(project_output.stdout).unwrap(),
        "APP-1\nAPP-2\n"
    );
    assert!(project_output.status.success());
    assert!(project_output.stderr.is_empty());

    let organization_output = base(&server, &dir)
        .args(["--org", "acme", INTERNAL_COMMAND, "work-item-key", "ORG-"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(organization_output.stdout).unwrap(),
        "ORG-1\nORG-2\n"
    );
    assert!(organization_output.status.success());
    assert!(organization_output.stderr.is_empty());

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request.method.as_str() == "GET")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn labels_require_project_and_filter_names() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/APP/labels"))
        .and(query_param("limit", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page(json!([
            label("frontend"),
            label("feature"),
            label("backend")
        ]))))
        .mount(&server)
        .await;

    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "APP",
            INTERNAL_COMMAND,
            "label",
            "fe",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "feature\n");
    assert!(output.stderr.is_empty());

    let missing_project = base(&server, &dir)
        .args(["--org", "acme", INTERNAL_COMMAND, "label", ""])
        .output()
        .unwrap();
    assert_silent_success(missing_project);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[test]
fn static_completion_never_needs_context_or_network() {
    let dir = TempDir::new().unwrap();
    for (typ, prefix, expected) in [
        ("status", "in_p", "in_progress\n"),
        ("type", "fe", "feature\n"),
    ] {
        let mut command = Command::cargo_bin("hamstik").unwrap();
        command
            .env("HAMSTIK_CONFIG", dir.path().join("config.toml"))
            .env_remove("HAMSTIK_TOKEN")
            .current_dir(dir.path());
        let output = command
            .args([INTERNAL_COMMAND, typ, prefix])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        assert!(output.stderr.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_credential_and_context_are_silent_successes() {
    let dir = TempDir::new().unwrap();
    let mut missing_credential = Command::cargo_bin("hamstik").unwrap();
    missing_credential
        .env("HAMSTIK_CONFIG", dir.path().join("config.toml"))
        .env_remove("HAMSTIK_TOKEN")
        .current_dir(dir.path());
    assert_silent_success(
        missing_credential
            .args([INTERNAL_COMMAND, "org", ""])
            .output()
            .unwrap(),
    );

    let server = MockServer::start().await;
    let mut missing_context = Command::cargo_bin("hamstik").unwrap();
    missing_context
        .env("HAMSTIK_CONFIG", dir.path().join("context-config.toml"))
        .env("HAMSTIK_HOST", server.uri())
        .env("HAMSTIK_TOKEN", "completion-test-token")
        .env_remove("HAMSTIK_ORG")
        .current_dir(dir.path());
    assert_silent_success(
        missing_context
            .args([INTERNAL_COMMAND, "project", ""])
            .output()
            .unwrap(),
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_failure_is_silent_and_completion_never_mutates() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let output = base(&server, &dir)
        .args([INTERNAL_COMMAND, "org", ""])
        .output()
        .unwrap();
    assert_silent_success(output);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        requests
            .iter()
            .all(|request| request.method.as_str() == "GET")
    );
}

#[test]
fn generated_scripts_rename_bash_static_function_and_hide_internal_candidates() {
    let dir = TempDir::new().unwrap();
    let script = |shell: &str| -> String {
        let mut command = Command::cargo_bin("hamstik").unwrap();
        command
            .env("HAMSTIK_CONFIG", dir.path().join("config.toml"))
            .current_dir(dir.path());
        String::from_utf8(command.args(["completion", shell]).output().unwrap().stdout).unwrap()
    };

    let bash = script("bash");
    assert!(bash.contains("_hamstik_completion() {"));
    assert!(bash.contains("_hamstik_dynamic_complete()"));
    assert!(
        !bash
            .lines()
            .any(|line| line.trim_start().starts_with("opts=") && line.contains(INTERNAL_COMMAND))
    );
    assert!(!bash.contains("sed -e"));

    let bash_probe = dir.path().join("bash-probe.sh");
    fs::write(&bash_probe, "true\n").unwrap();
    match ProcessCommand::new("bash")
        .arg("-n")
        .arg(&bash_probe)
        .output()
    {
        Ok(probe) if probe.status.success() => {
            let bash_script = dir.path().join("hamstik-completion.bash");
            fs::write(&bash_script, &bash).unwrap();
            let result = ProcessCommand::new("bash")
                .arg("-n")
                .arg(&bash_script)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "bash -n failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        Ok(probe) => eprintln!(
            "SKIPPED: bash could not syntax-check a known-valid script: {}",
            String::from_utf8_lossy(&probe.stderr)
        ),
        Err(err) => eprintln!("SKIPPED: bash syntax check unavailable: {err}"),
    }

    let zsh = script("zsh");
    assert!(!zsh.contains("Placeholder; replaced below"));
    assert!(!zsh.contains("_hamstik_dynamic_complete_for"));
    assert!(
        !zsh.lines()
            .any(|line| line.contains(&format!("'{INTERNAL_COMMAND}:")))
    );

    let fish = script("fish");
    assert!(fish.contains("commandline -opc"));
    assert!(!fish.contains(&format!("-a \"{INTERNAL_COMMAND}\"")));

    let powershell = script("powershell");
    assert!(!powershell.contains(INTERNAL_COMMAND));
}
