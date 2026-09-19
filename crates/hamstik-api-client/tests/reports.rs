// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Integration tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Wire tests for the two server report operations
//! (`getProjectReport`, `getProjectSprintReport`).

use std::sync::Arc;
use std::time::Duration;

use hamstik_api_client::retry::NoopSleeper;
use hamstik_api_client::{
    ClientConfig, HamstikApi, HamstikClient, ListOptions, ProjectReportOptions, RetryPolicy,
};
use secrecy::SecretString;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_for(url: &str) -> HamstikClient {
    let host = hamstik_api_client::Host::parse(url).expect("loopback host");
    let config = ClientConfig {
        retry: RetryPolicy {
            attempts: 1,
            base: Duration::from_millis(1),
            max_delay: Duration::from_millis(2),
            jitter: false,
        },
        ..ClientConfig::default()
    };
    HamstikClient::with_sleeper(
        host,
        SecretString::from("test-token"),
        config,
        Arc::new(NoopSleeper),
    )
    .expect("client")
}

/// The sorted `key=value` query pairs of the single received request.
async fn query_pairs(server: &MockServer) -> Vec<String> {
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "exactly one request");
    let query = requests[0].url.query().unwrap_or("");
    let mut pairs: Vec<String> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    pairs.sort();
    pairs
}

fn velocity_report() -> Value {
    json!({
        "kind": "velocity",
        "items": [{"sprintId": "s1", "committed": 10, "completed": 8}],
        "data": [
            {"label": "Sprint 1", "committed": 10, "completed": 8, "carried": 2},
            {"label": "Sprint 2", "committed": 12, "completed": 12, "carried": 0}
        ],
        "page": {"limit": 50, "hasMore": false, "nextCursor": null},
        "limitations": ["Unestimated items count as zero points."]
    })
}

#[tokio::test]
async fn project_report_sends_path_and_options_and_decodes_payload() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/reports/velocity",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-Request-Id", "req-report")
                .set_body_json(velocity_report()),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let response = client
        .get_project_report(
            "acme",
            "HAM",
            "velocity",
            ProjectReportOptions {
                limit: Some(25),
                range: Some(6),
                group_by: Some("assignee".to_string()),
                start: Some("2026-01-01".to_string()),
                end: Some("2026-06-30".to_string()),
                work_type: Some("story".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("report");

    assert_eq!(response.value.kind, "velocity");
    assert_eq!(response.value.data.len(), 2);
    assert_eq!(response.value.items.len(), 1);
    assert_eq!(response.value.limitations.len(), 1);
    assert!(!response.value.page.has_more);
    assert_eq!(response.request_id.as_deref(), Some("req-report"));
    // The raw body is preserved verbatim for `--json` passthrough fidelity.
    assert_eq!(response.raw, velocity_report());

    let mut pairs = query_pairs(&server).await;
    pairs.retain(|pair| !pair.is_empty());
    assert!(pairs.contains(&"limit=25".to_string()), "{pairs:?}");
    assert!(pairs.contains(&"range=6".to_string()), "{pairs:?}");
    assert!(pairs.contains(&"groupBy=assignee".to_string()), "{pairs:?}");
    assert!(pairs.contains(&"start=2026-01-01".to_string()), "{pairs:?}");
    assert!(pairs.contains(&"end=2026-06-30".to_string()), "{pairs:?}");
    assert!(pairs.contains(&"type=story".to_string()), "{pairs:?}");
}

#[tokio::test]
async fn project_report_sends_only_the_options_that_were_set() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/reports/epic-progress",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "kind": "epic-progress",
            "items": [],
            "data": [],
            "page": {"limit": 50, "hasMore": false, "nextCursor": null},
            "limitations": []
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    client
        .get_project_report(
            "acme",
            "HAM",
            "epic-progress",
            ProjectReportOptions {
                cursor: Some("cursor-abc".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("report");

    assert_eq!(
        query_pairs(&server).await,
        vec!["cursor=cursor-abc".to_string()]
    );
}

/// An unknown report type is the server's decision: the client surfaces the
/// error envelope (code, message, request id) instead of guessing.
#[tokio::test]
async fn project_report_unknown_type_surfaces_the_server_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/reports/burndown",
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {
                "code": "VALIDATION_ERROR",
                "message": "unknown report type: burndown",
                "details": {"supported": ["velocity", "cumulative-flow"]}
            },
            "requestId": "req-unknown-type"
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let error = client
        .get_project_report("acme", "HAM", "burndown", ProjectReportOptions::default())
        .await
        .unwrap_err();
    let api = error.as_api().expect("api error");
    assert_eq!(api.status, 400);
    assert_eq!(api.code, "VALIDATION_ERROR");
    assert_eq!(api.message, "unknown report type: burndown");
    assert_eq!(api.request_id.as_deref(), Some("req-unknown-type"));
}

#[tokio::test]
async fn sprint_report_decodes_report_sections_and_forwards_pagination() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/sprints/sprint-1/report",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(sprint_report()))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let response = client
        .get_sprint_report(
            "acme",
            "HAM",
            "sprint-1",
            ListOptions {
                limit: Some(10),
                cursor: None,
            },
        )
        .await
        .expect("sprint report");
    let report = &response.value;

    assert_eq!(report.sprint.name, "Sprint 12");
    assert_eq!(report.mode, "active");
    assert!(report.stable);
    assert_eq!(report.commitment.points, 34);
    assert_eq!(report.completion.completed.item_count, 6);
    assert_eq!(
        report.completion.percentages.final_scope.point_percent,
        Some(75)
    );
    assert_eq!(report.scope_changes.added.len(), 1);
    assert_eq!(report.scope_changes.added[0].key, "HAM-21");
    assert_eq!(report.carryover.len(), 1);
    assert_eq!(report.carryover[0].destination.name, "Backlog");
    assert_eq!(report.statuses[0].item_count, 3);
    assert!(report.burndown.available);
    assert_eq!(report.burndown.display.len(), 2);
    assert_eq!(report.burndown.display[1].remaining, 10);
    assert_eq!(report.burndown.points[0].remaining_points, 34);
    assert_eq!(report.remaining.points, 10);
    assert_eq!(report.items.len(), 2);
    assert_eq!(report.items[0].change_type.as_deref(), Some("added"));
    assert_eq!(
        report.items[1]
            .destination
            .as_ref()
            .map(|d| d.kind.as_str()),
        Some("backlog")
    );
    // The raw body is preserved verbatim for `--json` passthrough fidelity.
    assert_eq!(response.raw, sprint_report());
    assert_eq!(query_pairs(&server).await, vec!["limit=10".to_string()]);
}

/// A burndown the server marks unavailable still decodes and renders.
#[tokio::test]
async fn sprint_report_accepts_an_unavailable_burndown() {
    let server = MockServer::start().await;
    let mut payload = sprint_report();
    payload["burndown"] = json!({"available": false, "points": [], "display": []});
    payload["limitations"] = json!(["Sprint has no start date."]);
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/sprints/sprint-2/report",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(payload))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let response = client
        .get_sprint_report("acme", "HAM", "sprint-2", ListOptions::default())
        .await
        .expect("sprint report");
    assert!(!response.value.burndown.available);
    assert!(response.value.burndown.display.is_empty());
    assert_eq!(response.value.limitations, ["Sprint has no start date."]);
    // With no pagination options the request carries no query string at all.
    assert!(query_pairs(&server).await.is_empty());
}

fn sprint_report() -> Value {
    json!({
        "sprint": {
            "id": "0f9c1a2e-3f4b-4a5c-9d6e-7f8a9b0c1d2e",
            "name": "Sprint 12",
            "state": "active",
            "startDate": "2026-09-01T00:00:00.000Z",
            "endDate": "2026-09-14T00:00:00.000Z",
            "goal": "Ship reports",
            "targetPoints": 34,
            "updatedAt": "2026-09-08T10:00:00.000Z",
            "reportEndAt": null
        },
        "mode": "active",
        "stable": true,
        "commitment": {"available": true, "itemCount": 12, "points": 34},
        "plannedScope": {"itemCount": 12, "points": 34},
        "completion": {
            "originalCommitment": {"itemCount": 12, "points": 34},
            "finalScope": {"itemCount": 13, "points": 40},
            "completed": {"itemCount": 6, "points": 30},
            "estimatedItemCount": 11,
            "itemIds": {
                "originalCommitment": ["a1", "a2"],
                "finalScope": ["a1", "a2", "a3"],
                "completed": ["a1"]
            },
            "percentages": {
                "finalScope": {"itemPercent": 46, "pointPercent": 75},
                "originalCommitment": {"itemPercent": 50, "pointPercent": 88}
            }
        },
        "scopeChanges": {
            "added": [{
                "id": "row-1", "workItemId": "wi-21", "key": "HAM-21", "title": "Added",
                "status": "todo", "storyPoints": 3, "occurredAt": "2026-09-03T09:00:00.000Z",
                "kind": "added"
            }],
            "removed": []
        },
        "carryover": [{
            "id": "row-2", "workItemId": "wi-22", "key": "HAM-22", "title": "Carried",
            "status": "todo", "storyPoints": null, "occurredAt": "2026-09-14T09:00:00.000Z",
            "destination": {"kind": "backlog", "sprintId": null, "name": "Backlog"}
        }],
        "statuses": [
            {"status": "done", "itemCount": 3, "points": 30},
            {"status": "in_progress", "itemCount": 2, "points": 10}
        ],
        "burndown": {
            "available": true,
            "points": [
                {"date": "2026-09-01", "remainingPoints": 34},
                {"date": "2026-09-02", "remainingPoints": 30}
            ],
            "display": [
                {"date": "2026-09-01", "label": "Sep 1", "remaining": 34, "ideal": 34},
                {"date": "2026-09-02", "label": "Sep 2", "remaining": 10, "ideal": 30}
            ]
        },
        "remaining": {"itemCount": 7, "points": 10},
        "limitations": [],
        "items": [
            {
                "id": "row-1", "workItemId": "wi-21", "key": "HAM-21", "title": "Added",
                "status": "todo", "storyPoints": 3, "occurredAt": "2026-09-03T09:00:00.000Z",
                "kind": "scope_change", "changeType": "added"
            },
            {
                "id": "row-2", "workItemId": "wi-22", "key": "HAM-22", "title": "Carried",
                "status": "todo", "storyPoints": null, "occurredAt": "2026-09-14T09:00:00.000Z",
                "kind": "carryover",
                "destination": {"kind": "backlog", "sprintId": null, "name": "Backlog"}
            }
        ],
        "page": {"limit": 10, "hasMore": false, "nextCursor": null}
    })
}
