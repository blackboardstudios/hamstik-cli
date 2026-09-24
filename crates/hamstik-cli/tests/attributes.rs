// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

#![allow(clippy::expect_used, clippy::unwrap_used)]

//! CLI contract tests for Organization Attributes.

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

fn local(dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env("HAMSTIK_HOST", "http://127.0.0.1:1");
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn work_item_json(revision: i64) -> Value {
    json!({
        "id":"1","key":"HAM-1","projectId":"2","title":"T","description":null,
        "type":"task","status":"todo","priority":"low","assignee":null,"reporter":null,
        "sprint":null,"parent":null,"labels":[],"storyPoints":null,"dueDate":null,
        "archivedAt":null,"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z","revision":revision
    })
}

#[test]
fn create_flags_group_multi_select_and_preserve_boolean_false_in_preview() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
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
            "Classify request",
            "--attribute-boolean",
            "verified=false",
            "--attribute-option",
            "product_area=search",
            "--attribute-option",
            "product_area=mobile",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(preview["operation"], "work.create");
    assert_eq!(preview["body"]["attributes"][0]["key"], "product_area");
    assert_eq!(
        preview["body"]["attributes"][0]["optionKeys"],
        json!(["search", "mobile"])
    );
    assert_eq!(preview["body"]["attributes"][1]["booleanValue"], false);
    assert!(preview["request"]["headers"]["Idempotency-Key"].is_string());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edit_flags_send_false_clear_and_idempotency_in_preview() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-4\"")
                .set_body_json(work_item_json(4)),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--dry-run",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "edit",
            "HAM-1",
            "--attribute-boolean",
            "verified=false",
            "--clear-attribute",
            "customer",
            "--idempotency-key",
            "attribute-edit-preview-1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(preview["request"]["headers"]["If-Match"], "\"wi-4\"");
    assert_eq!(
        preview["request"]["headers"]["Idempotency-Key"],
        "attribute-edit-preview-1"
    );
    assert_eq!(preview["body"]["attributes"][0]["booleanValue"], false);
    assert_eq!(preview["body"]["attributes"][1]["clear"], true);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[test]
fn force_is_rejected_for_attribute_updates_before_network_access() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "edit",
            "HAM-1",
            "--attribute-boolean",
            "verified=false",
            "--force",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("require an exact Work Item revision")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn organization_attribute_list_exposes_supported_types_retired_and_limits() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/attributes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "supportedTypes":["single_select","multi_select","boolean"],
            "limits":{"definitionsPerOrganization":25,"optionsPerDefinition":50,"multiSelectSelections":10},
            "items":[{
                "id":"d1","organizationId":"o1","key":"customer","name":"Customer",
                "type":"single_select","state":"retired","disabledAt":null,"retiredAt":"2026-01-01T00:00:00Z",
                "revision":4,"createdAt":"2025-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z","options":[]
            }]
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "attribute",
            "list",
            "--include-retired",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let catalog: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(catalog["limits"]["definitionsPerOrganization"], 25);
    assert_eq!(
        catalog["supportedTypes"],
        json!(["single_select", "multi_select", "boolean"])
    );
    assert_eq!(catalog["items"][0]["state"], "retired");
    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request.url.query_pairs().next().unwrap().0,
        "includeRetired"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn definition_rename_uses_current_etag_and_idempotency_key() {
    let server = MockServer::start().await;
    let definition = json!({
        "id":"d1","organizationId":"o1","key":"customer","name":"Customer",
        "type":"single_select","state":"active","disabledAt":null,"retiredAt":null,
        "revision":5,"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z","options":[]
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/attributes/customer"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"attribute-5\"")
                .set_body_json(definition.clone()),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/organizations/acme/attributes/customer"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"attribute-6\"")
                .set_body_json(json!({
                    "id":"d1","organizationId":"o1","key":"customer","name":"Client",
                    "type":"single_select","state":"active","disabledAt":null,"retiredAt":null,
                    "revision":6,"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-02-01T00:00:00Z","options":[]
                })),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "attribute",
            "rename",
            "customer",
            "--name",
            "Client",
            "--reason",
            "Customer renamed",
            "--idempotency-key",
            "attribute-cli-rename-1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1]
            .headers
            .get("if-match")
            .unwrap()
            .to_str()
            .unwrap(),
        "\"attribute-5\""
    );
    assert_eq!(
        requests[1]
            .headers
            .get("idempotency-key")
            .unwrap()
            .to_str()
            .unwrap(),
        "attribute-cli-rename-1"
    );
    let body: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(body, json!({"name":"Client","reason":"Customer renamed"}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_view_renders_attribute_false_and_retained_retired_option() {
    let server = MockServer::start().await;
    let mut item = work_item_json(4);
    item["attributes"] = json!([
        {
            "definitionId":"d1","key":"verified","name":"Verified","type":"boolean","state":"active",
            "projectEnabled":true,"booleanValue":false,"setAt":"2026-01-01T00:00:00Z","options":[]
        },
        {
            "definitionId":"d2","key":"product_area","name":"Product Area","type":"multi_select","state":"active",
            "projectEnabled":false,"booleanValue":null,"setAt":"2026-01-01T00:00:00Z",
            "options":[{"id":"o1","key":"legacy","label":"Legacy area","state":"retired","position":0}]
        }
    ]);
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(item))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "view",
            "HAM-1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("attribute verified (Verified)  false"),
        "{text}"
    );
    assert!(text.contains("Legacy area [legacy] (retired)"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_create_and_update_accept_the_same_attribute_change_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/bulk-work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results":[{"index":0,"status":201}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/api/v1/organizations/acme/bulk-work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results":[{"index":0,"status":200}]
        })))
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let create = dir.path().join("create.json");
    let update = dir.path().join("update.json");
    std::fs::write(
        &create,
        r#"[{"projectKey":"HAM","title":"Bulk","attributes":[{"key":"verified","booleanValue":false}]}]"#,
    )
    .unwrap();
    std::fs::write(
        &update,
        r#"[{"projectKey":"HAM","workItemKey":"HAM-1","revision":4,"changes":{"attributes":[{"key":"customer","clear":true}]}}]"#,
    )
    .unwrap();

    let created = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "work",
            "bulk",
            "create",
            "--operations-file",
        ])
        .arg(&create)
        .arg("--idempotency-key")
        .arg("attribute-bulk-create-1")
        .output()
        .unwrap();
    assert_eq!(created.status.code(), Some(0), "{:?}", created.stderr);
    let updated = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "work",
            "bulk",
            "update",
            "--operations-file",
        ])
        .arg(&update)
        .arg("--idempotency-key")
        .arg("attribute-bulk-update-1")
        .output()
        .unwrap();
    assert_eq!(updated.status.code(), Some(0), "{:?}", updated.stderr);

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[0].body).unwrap()["operations"][0]["attributes"]
            [0]["booleanValue"],
        false
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[1].body).unwrap()["operations"][0]["changes"]["attributes"]
            [0]["clear"],
        true
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_restricted_pat_uses_the_exact_etag_from_its_project_catalog() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/attributes/customer"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error":{"code":"NOT_FOUND","message":"resource not found"}
        })))
        .mount(&server)
        .await;
    let definition = json!({
        "id":"d1","organizationId":"o1","key":"customer","name":"Customer",
        "type":"single_select","state":"active","disabledAt":null,"retiredAt":null,
        "revision":12,"etag":"\"attribute-custom-token-12\"","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z","options":[]
    });
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/attributes",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "supportedTypes":["single_select","multi_select","boolean"],
            "available":[definition],
            "enabled":[{"definitionId":"d1","key":"customer","name":"Customer","type":"single_select","state":"active","enabledAt":"2026-01-01T00:00:00Z"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/attributes/customer/enablement",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "key":"customer","enabled":false,"revision":13
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "--project",
            "HAM",
            "attribute",
            "project",
            "disable",
            "customer",
            "--idempotency-key",
            "attribute-project-disable-1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[2]
            .headers
            .get("if-match")
            .unwrap()
            .to_str()
            .unwrap(),
        "\"attribute-custom-token-12\""
    );
    assert_eq!(
        requests[2].url.path(),
        "/api/v1/organizations/acme/projects/HAM/attributes/customer/enablement"
    );
}
