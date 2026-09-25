// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

#![allow(clippy::expect_used, clippy::unwrap_used)]

//! End-to-end CLI coverage for Advanced Reports and Dashboards.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REPORTS: &str = "/api/v1/organizations/acme/advanced-reports";
const REPORT: &str =
    "/api/v1/organizations/acme/advanced-reports/11111111-1111-4111-8111-111111111111";
const REPORT_RUN: &str =
    "/api/v1/organizations/acme/advanced-reports/11111111-1111-4111-8111-111111111111/runs";
const SELECTION: &str = "/api/v1/organizations/acme/advanced-report-runs/22222222-2222-4222-8222-222222222222/cells/33333333-3333-4333-8333-333333333333/items";
const DASHBOARDS: &str = "/api/v1/organizations/acme/advanced-dashboards";
const DASHBOARD: &str =
    "/api/v1/organizations/acme/advanced-dashboards/44444444-4444-4444-8444-444444444444";
const DASHBOARD_RUN: &str =
    "/api/v1/organizations/acme/advanced-dashboards/44444444-4444-4444-8444-444444444444/runs";

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut command = Command::cargo_bin("hamstik").expect("hamstik binary");
    command
        .env("HAMSTIK_CONFIG", dir.path().join("config.toml"))
        .env(
            "HAMSTIK_REQUEST_JOURNAL",
            dir.path().join("request-journal.log"),
        )
        .env("HAMSTIK_HOST", server.uri())
        .env("HAMSTIK_TOKEN", "test-token")
        .env_remove("HAMSTIK_PROFILE")
        .env_remove("HAMSTIK_ORG")
        .env_remove("HAMSTIK_PROJECT")
        .current_dir(dir.path())
        .arg("--no-retry");
    command
}

fn definition() -> Value {
    json!({
        "version": 1,
        "name": "Flow",
        "description": "Flow report",
        "organizationId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "visibility": "personal",
        "dataset": {
            "version": 2,
            "projects": {"mode": "all_authorized"},
            "dateRange": {"mode": "rolling_days", "days": 30},
            "timeZone": "America/Chicago",
            "filter": {},
            "measure": "item_count",
            "groupBy": "status",
            "dimensionTime": "current",
            "timeBucket": "none",
            "bucketBy": "none"
        },
        "visualization": {
            "type": "bar",
            "config": {"showLegend": true, "showValues": false}
        }
    })
}

fn report() -> Value {
    json!({
        "id": "11111111-1111-4111-8111-111111111111",
        "name": "Flow",
        "description": "Flow report",
        "visibility": "personal",
        "owner": {"publicId": "usr_cPbfeqnghA-RLpDVOMQhHg"},
        "revision": 2,
        "createdAt": "2026-09-18T00:00:00Z",
        "updatedAt": "2026-09-19T00:00:00Z",
        "sourceAvailability": "complete",
        "permissions": {"edit": true, "delete": true, "duplicate": true},
        "definition": definition()
    })
}

fn dashboard() -> Value {
    json!({
        "id": "44444444-4444-4444-8444-444444444444",
        "name": "Delivery",
        "description": "Delivery dashboard",
        "visibility": "organization",
        "owner": {"publicId": "usr_cPbfeqnghA-RLpDVOMQhHg"},
        "revision": 4,
        "createdAt": "2026-09-18T00:00:00Z",
        "updatedAt": "2026-09-19T00:00:00Z",
        "sourceAvailability": "complete",
        "permissions": {"edit": false, "delete": false, "duplicate": true},
        "version": 1,
        "organizationId": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "defaultFilters": {},
        "widgets": []
    })
}

#[tokio::test]
async fn report_list_and_view_preserve_server_json() {
    let server = MockServer::start().await;
    let list = json!({
        "items": [report()],
        "page": {"limit": 25, "hasMore": false, "nextCursor": null}
    });
    Mock::given(method("GET"))
        .and(path(REPORTS))
        .and(query_param("visibility", "personal"))
        .and(query_param("limit", "25"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(REPORT))
        .respond_with(ResponseTemplate::new(200).set_body_json(report()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let list_output = base(&server, &dir)
        .args(["--json", "--org", "acme", "report", "list"])
        .args(["--visibility", "personal", "--limit", "25"])
        .output()
        .unwrap();
    assert!(list_output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&list_output.stdout).unwrap(),
        list
    );

    let view_output = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "report",
            "view",
            "11111111-1111-4111-8111-111111111111",
        ])
        .output()
        .unwrap();
    assert!(view_output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&view_output.stdout).unwrap(),
        report()
    );
}

#[tokio::test]
async fn report_create_edit_and_delete_apply_idempotency_and_etags() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(REPORTS))
        .and(header("Idempotency-Key", "report-create-key"))
        .and(body_json(definition()))
        .respond_with(ResponseTemplate::new(201).set_body_json(report()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(REPORT))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"advanced-report-2\"")
                .set_body_json(report()),
        )
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(REPORT))
        .and(header("If-Match", "\"advanced-report-2\""))
        .and(header("Idempotency-Key", "report-update-key"))
        .and(body_json(definition()))
        .respond_with(ResponseTemplate::new(200).set_body_json(report()))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(REPORT))
        .and(header("If-Match", "\"advanced-report-2\""))
        .and(header("Idempotency-Key", "report-delete-key"))
        .and(body_json(json!({})))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let input = dir.path().join("report.json");
    std::fs::write(&input, serde_json::to_vec(&definition()).unwrap()).unwrap();
    let input = input.to_str().unwrap();

    base(&server, &dir)
        .args(["--json", "--org", "acme", "report", "create"])
        .args(["--file", input, "--idempotency-key", "report-create-key"])
        .assert()
        .success();
    base(&server, &dir)
        .args(["--json", "--org", "acme", "report", "edit"])
        .arg("11111111-1111-4111-8111-111111111111")
        .args(["--file", input, "--idempotency-key", "report-update-key"])
        .assert()
        .success();
    base(&server, &dir)
        .args(["--json", "--org", "acme", "report", "delete"])
        .arg("11111111-1111-4111-8111-111111111111")
        .args(["--idempotency-key", "report-delete-key"])
        .assert()
        .success()
        .stdout(predicate::eq(
            "{\n  \"deleted\": true,\n  \"id\": \"11111111-1111-4111-8111-111111111111\"\n}\n",
        ));
}

#[tokio::test]
async fn report_run_reads_revision_and_selection_items_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPORT))
        .respond_with(ResponseTemplate::new(200).set_body_json(report()))
        .mount(&server)
        .await;
    let run_result = json!({
        "definition": definition(),
        "dataset": null,
        "presentationWarning": null,
        "sourceAvailability": "unavailable",
        "message": "No source data"
    });
    Mock::given(method("POST"))
        .and(path(REPORT_RUN))
        .and(body_json(json!({"expectedRevision": 2})))
        .respond_with(ResponseTemplate::new(200).set_body_json(run_result.clone()))
        .mount(&server)
        .await;
    let selection = json!({
        "evaluatedAt": "2026-09-19T00:00:00Z",
        "expiresAt": "2026-09-20T00:00:00Z",
        "totalItems": 1,
        "capturedValue": 1,
        "sampleCount": 1,
        "contributionSum": 1,
        "aggregation": "count",
        "unit": "items",
        "items": [{
            "key": "HAM-1",
            "project": {"key": "HAM", "name": "Hamstik"},
            "title": "Ship",
            "status": "done",
            "archived": false,
            "deleted": false,
            "capturedContribution": 1,
            "estimateWasNull": false
        }],
        "page": {"limit": 10, "hasMore": false, "nextCursor": null}
    });
    Mock::given(method("GET"))
        .and(path(SELECTION))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(selection.clone()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let run = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "report",
            "run",
            "11111111-1111-4111-8111-111111111111",
        ])
        .output()
        .unwrap();
    assert!(run.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&run.stdout).unwrap(),
        run_result
    );

    let items = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "report",
            "selection-items",
            "22222222-2222-4222-8222-222222222222",
            "33333333-3333-4333-8333-333333333333",
            "--limit",
            "10",
        ])
        .output()
        .unwrap();
    assert!(items.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&items.stdout).unwrap(),
        selection
    );
}

#[tokio::test]
async fn dashboard_list_view_and_run_use_current_revision_and_filter_file() {
    let server = MockServer::start().await;
    let list = json!({
        "items": [dashboard()],
        "page": {"limit": 50, "hasMore": false, "nextCursor": null}
    });
    Mock::given(method("GET"))
        .and(path(DASHBOARDS))
        .and(query_param("visibility", "organization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(list.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(DASHBOARD))
        .respond_with(ResponseTemplate::new(200).set_body_json(dashboard()))
        .expect(2)
        .mount(&server)
        .await;
    let run_result = json!({
        "version": 1,
        "dashboardId": "44444444-4444-4444-8444-444444444444",
        "dashboardRevision": 4,
        "evaluatedAt": "2026-09-19T00:00:00Z",
        "sourceAvailability": "complete",
        "appliedFilters": {"dateRange": {"mode": "rolling_days", "days": 14}},
        "widgets": []
    });
    Mock::given(method("POST"))
        .and(path(DASHBOARD_RUN))
        .and(body_json(json!({
            "expectedRevision": 4,
            "filters": {"dateRange": {"mode": "rolling_days", "days": 14}}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(run_result.clone()))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let filters = dir.path().join("filters.json");
    std::fs::write(
        &filters,
        serde_json::to_vec(&json!({
            "dateRange": {"mode": "rolling_days", "days": 14}
        }))
        .unwrap(),
    )
    .unwrap();

    let listed = base(&server, &dir)
        .args(["--json", "--org", "acme", "dashboard", "list"])
        .args(["--visibility", "organization"])
        .output()
        .unwrap();
    assert!(listed.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&listed.stdout).unwrap(),
        list
    );

    base(&server, &dir)
        .args([
            "--org",
            "acme",
            "dashboard",
            "view",
            "44444444-4444-4444-8444-444444444444",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Delivery"));

    let run = base(&server, &dir)
        .args([
            "--json",
            "--org",
            "acme",
            "dashboard",
            "run",
            "44444444-4444-4444-8444-444444444444",
            "--filters-file",
            filters.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(run.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&run.stdout).unwrap(),
        run_result
    );
}

#[tokio::test]
async fn malformed_report_input_fails_before_network() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, b"{not-json").unwrap();

    base(&server, &dir)
        .args(["--org", "acme", "report", "create", "--file"])
        .arg(&bad)
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("invalid JSON"));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn forced_report_edit_dry_run_needs_no_network() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let input = dir.path().join("report.json");
    std::fs::write(&input, serde_json::to_vec(&definition()).unwrap()).unwrap();

    let output = base(&server, &dir)
        .args(["--json", "--dry-run", "--org", "acme", "report", "edit"])
        .arg("11111111-1111-4111-8111-111111111111")
        .args(["--file", input.to_str().unwrap(), "--force"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let preview: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(preview["operation"], "report.edit");
    assert_eq!(preview["request"]["method"], "PATCH");
    assert_eq!(preview["request"]["headers"]["If-Match"], "*");
    assert!(server.received_requests().await.unwrap().is_empty());
}
