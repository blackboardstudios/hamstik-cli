// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

#![allow(clippy::expect_used, clippy::unwrap_used)]

//! Wire coverage for the ten Advanced Reporting Public API operations.

use std::sync::Arc;
use std::time::Duration;

use hamstik_api_client::retry::NoopSleeper;
use hamstik_api_client::{
    AdvancedDashboardFilters, AdvancedDashboardRunRequest, AdvancedReportInput,
    AdvancedReportRunRequest, ClientConfig, HamstikApi, HamstikClient, ListAdvancedOptions,
    ListOptions, RetryPolicy,
};
use secrecy::SecretString;
use serde_json::{Value, json};
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

fn client_for(url: &str) -> HamstikClient {
    HamstikClient::with_sleeper(
        hamstik_api_client::Host::parse(url).expect("loopback host"),
        SecretString::from("test-token"),
        ClientConfig {
            retry: RetryPolicy {
                attempts: 1,
                base: Duration::from_millis(1),
                max_delay: Duration::from_millis(2),
                jitter: false,
            },
            ..ClientConfig::default()
        },
        Arc::new(NoopSleeper),
    )
    .expect("client")
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
async fn report_directory_create_and_get_are_typed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(REPORTS))
        .and(query_param("limit", "25"))
        .and(query_param("cursor", "next"))
        .and(query_param("visibility", "personal"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [report()],
            "page": {"limit": 25, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(REPORTS))
        .and(header("Idempotency-Key", "advanced-create-key"))
        .and(body_json(definition()))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("ETag", "\"advanced-report-2\"")
                .set_body_json(report()),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(REPORT))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"advanced-report-2\"")
                .set_body_json(report()),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let listed = client
        .list_advanced_reports(
            "acme",
            ListAdvancedOptions {
                limit: Some(25),
                cursor: Some("next".into()),
                visibility: Some("personal".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(listed.value.items[0].name, "Flow");

    let body: AdvancedReportInput = serde_json::from_value(definition()).unwrap();
    let created = client
        .create_advanced_report("acme", &body, "advanced-create-key")
        .await
        .unwrap();
    assert_eq!(created.value.revision, 2);
    assert_eq!(created.etag.as_deref(), Some("\"advanced-report-2\""));

    let fetched = client
        .get_advanced_report("acme", "11111111-1111-4111-8111-111111111111")
        .await
        .unwrap();
    assert!(fetched.value.definition.is_some());
}

#[tokio::test]
async fn report_update_delete_and_run_send_revision_contracts() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(REPORT))
        .and(header("If-Match", "\"advanced-report-2\""))
        .and(header("Idempotency-Key", "advanced-update-key"))
        .and(body_json(definition()))
        .respond_with(ResponseTemplate::new(200).set_body_json(report()))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(REPORT))
        .and(header("If-Match", "\"advanced-report-2\""))
        .and(header("Idempotency-Key", "advanced-delete-key"))
        .and(body_json(json!({})))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(REPORT_RUN))
        .and(body_json(json!({"expectedRevision": 2})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "definition": definition(),
            "dataset": null,
            "presentationWarning": null,
            "sourceAvailability": "unavailable",
            "message": "No source data"
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body: AdvancedReportInput = serde_json::from_value(definition()).unwrap();
    client
        .update_advanced_report(
            "acme",
            "11111111-1111-4111-8111-111111111111",
            &body,
            "\"advanced-report-2\"",
            "advanced-update-key",
        )
        .await
        .unwrap();
    client
        .delete_advanced_report(
            "acme",
            "11111111-1111-4111-8111-111111111111",
            "\"advanced-report-2\"",
            "advanced-delete-key",
        )
        .await
        .unwrap();
    let run = client
        .run_advanced_report(
            "acme",
            "11111111-1111-4111-8111-111111111111",
            &AdvancedReportRunRequest {
                expected_revision: 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(run.value.message.as_deref(), Some("No source data"));
}

#[tokio::test]
async fn selection_items_decode_captured_metadata_and_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(SELECTION))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "evaluatedAt": "2026-09-19T00:00:00Z",
            "expiresAt": "2026-09-20T00:00:00Z",
            "totalItems": 1,
            "capturedValue": 3,
            "sampleCount": 1,
            "contributionSum": 3,
            "aggregation": "sum",
            "unit": "points",
            "items": [{
                "key": "HAM-1",
                "project": {"key": "HAM", "name": "Hamstik"},
                "title": "Ship",
                "status": "done",
                "archived": false,
                "deleted": false,
                "capturedContribution": 3,
                "estimateWasNull": false
            }],
            "page": {"limit": 10, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;

    let response = client_for(&server.uri())
        .list_advanced_selection_items(
            "acme",
            "22222222-2222-4222-8222-222222222222",
            "33333333-3333-4333-8333-333333333333",
            ListOptions {
                limit: Some(10),
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(response.value.total_items, 1);
    assert_eq!(response.value.items[0].key, "HAM-1");
}

#[tokio::test]
async fn dashboard_list_get_and_run_are_typed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(DASHBOARDS))
        .and(query_param("visibility", "organization"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [dashboard()],
            "page": {"limit": 50, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(DASHBOARD))
        .respond_with(ResponseTemplate::new(200).set_body_json(dashboard()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(DASHBOARD_RUN))
        .and(body_json(json!({
            "expectedRevision": 4,
            "filters": {"dateRange": {"mode": "rolling_days", "days": 14}}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "version": 1,
            "dashboardId": "44444444-4444-4444-8444-444444444444",
            "dashboardRevision": 4,
            "evaluatedAt": "2026-09-19T00:00:00Z",
            "sourceAvailability": "complete",
            "appliedFilters": {"dateRange": {"mode": "rolling_days", "days": 14}},
            "widgets": []
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let listed = client
        .list_advanced_dashboards(
            "acme",
            ListAdvancedOptions {
                visibility: Some("organization".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(listed.value.items[0].name, "Delivery");
    let fetched = client
        .get_advanced_dashboard("acme", "44444444-4444-4444-8444-444444444444")
        .await
        .unwrap();
    assert_eq!(fetched.value.revision, 4);
    let result = client
        .run_advanced_dashboard(
            "acme",
            "44444444-4444-4444-8444-444444444444",
            &AdvancedDashboardRunRequest {
                expected_revision: 4,
                filters: Some(AdvancedDashboardFilters {
                    date_range: Some(json!({"mode": "rolling_days", "days": 14})),
                    ..Default::default()
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(result.value.dashboard_revision, 4);
}
