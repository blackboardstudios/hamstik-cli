// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Integration tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Transport tests for the Release, Milestone, announcement, and audit
//! operations against a local wiremock.

use std::sync::Arc;
use std::time::Duration;

use hamstik_api_client::retry::NoopSleeper;
use hamstik_api_client::{
    AnnouncementDraftRevisionRequest, BulkReleaseMembershipOperation, BulkReleaseMembershipRequest,
    ClientConfig, CreateOrganizationMilestoneRequest, CreateReleaseVersionRequest,
    EditReleaseAnnouncementDraftRequest, GenerateReleaseAnnouncementDraftRequest,
    GenerateReleaseAuditReportRequest, HamstikApi, HamstikClient, ListMilestonesOptions,
    ListReleaseVersionsOptions, MilestoneDetailQuery, MilestoneEventsQuery, MilestoneReleasesQuery,
    MutateOrganizationMilestoneReleaseRequest, MutateWorkItemReleaseVersionsRequest,
    PublishReleaseAnnouncementRequest, ReleaseScopeQuery, ReleaseVersionLifecycleRequest,
    RetryPolicy, TransitionOrganizationMilestoneRequest, TransitionReleaseVersionRequest,
    UpdateOrganizationMilestoneRequest, UpdateReleaseVersionRequest,
};
use secrecy::SecretString;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_for(url: &str) -> HamstikClient {
    let host = hamstik_api_client::Host::parse(url).expect("loopback host");
    let config = ClientConfig {
        retry: RetryPolicy {
            attempts: 3,
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

fn release_json(state: &str, revision: i64) -> serde_json::Value {
    json!({
        "id": "22222222-2222-2222-2222-222222222222",
        "projectId": "33333333-3333-3333-3333-333333333333",
        "name": "Hamstik 0.4.0",
        "displayVersion": "0.4.0",
        "description": null,
        "ownerId": null,
        "ownerPublicId": null,
        "ownerName": null,
        "createdById": null,
        "creatorPublicId": null,
        "creatorName": null,
        "targetDate": null,
        "releaseDate": null,
        "state": state,
        "stateBeforeArchive": null,
        "firstReleasedAt": null,
        "archivedAt": null,
        "revision": revision,
        "createdAt": "2026-09-01T00:00:00Z",
        "updatedAt": "2026-09-01T00:00:00Z"
    })
}

fn milestone_json(state: &str, revision: i64) -> serde_json::Value {
    json!({
        "id": "44444444-4444-4444-4444-444444444444",
        "organizationId": "55555555-5555-5555-5555-555555555555",
        "name": "Q4 platform hardening",
        "description": null,
        "ownerId": null,
        "ownerPublicId": null,
        "ownerName": null,
        "targetDate": null,
        "state": state,
        "stateBeforeArchive": null,
        "completedAt": null,
        "archivedAt": null,
        "revision": revision,
        "createdAt": "2026-09-01T00:00:00Z",
        "updatedAt": "2026-09-01T00:00:00Z"
    })
}

#[tokio::test]
async fn release_create_sends_idempotency_and_optional_nulls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/releases"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("ETag", "\"release-1\"")
                .set_body_json(release_json("planned", 1)),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body = CreateReleaseVersionRequest {
        name: "Hamstik 0.4.0".to_string(),
        display_version: Some(Some("0.4.0".to_string())),
        description: Some(None),
        owner_id: Some(None),
        target_date: None,
        release_date: None,
    };
    let response = client
        .create_release_version("acme", "HAM", &body, "release-key-01")
        .await
        .unwrap();
    assert_eq!(response.value.state, "planned");

    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request
            .headers
            .get("idempotency-key")
            .unwrap()
            .to_str()
            .unwrap(),
        "release-key-01"
    );
    let sent: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["name"], "Hamstik 0.4.0");
    assert_eq!(sent["displayVersion"], "0.4.0");
    assert_eq!(sent["description"], serde_json::Value::Null);
    assert_eq!(sent["targetDate"], serde_json::Value::Null);
    assert_eq!(sent.get("releaseDate"), None);
}

#[tokio::test]
async fn release_list_sends_state_and_include_archived_filters() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/releases"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [release_json("planned", 1)],
            "page": {"limit": 50, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let response = client
        .list_release_versions(
            "acme",
            "HAM",
            ListReleaseVersionsOptions {
                limit: Some(50),
                cursor: None,
                state: Some("planned".to_string()),
                include_archived: Some(false),
            },
        )
        .await
        .unwrap();
    assert_eq!(response.value.items.len(), 1);

    let request = &server.received_requests().await.unwrap()[0];
    let query = request.url.query().unwrap_or_default();
    assert!(query.contains("state=planned"), "{query}");
    assert!(query.contains("includeArchived=false"), "{query}");
}

#[tokio::test]
async fn release_update_sends_if_match_idempotency_and_patches() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/releases/22222222-2222-2222-2222-222222222222",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"release-2\"")
                .set_body_json(release_json("planned", 2)),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body = UpdateReleaseVersionRequest {
        name: Some(Some("Renamed".to_string())),
        display_version: Some(None),
        ..UpdateReleaseVersionRequest::default()
    };
    let response = client
        .update_release_version(
            "acme",
            "HAM",
            "22222222-2222-2222-2222-222222222222",
            &body,
            "\"release-1\"",
            "release-key-02",
        )
        .await
        .unwrap();
    assert_eq!(response.value.revision, 2);

    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(request.method.as_str(), "PATCH");
    assert_eq!(
        request.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"release-1\""
    );
    assert_eq!(
        request
            .headers
            .get("idempotency-key")
            .unwrap()
            .to_str()
            .unwrap(),
        "release-key-02"
    );
    let sent: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["name"], "Renamed");
    assert_eq!(sent["displayVersion"], serde_json::Value::Null);
    assert_eq!(sent.get("description"), None);
}

#[tokio::test]
async fn release_transition_sends_if_match_confirm_flag_and_reason() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/releases/22222222-2222-2222-2222-222222222222/transitions",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"release-2\"")
                .set_body_json(release_json("released", 2)),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body = TransitionReleaseVersionRequest {
        target_state: "released".to_string(),
        confirm_incomplete_scope: Some(true),
        reason: Some("GA day".to_string()),
    };
    let response = client
        .transition_release_version(
            "acme",
            "HAM",
            "22222222-2222-2222-2222-222222222222",
            &body,
            "\"release-1\"",
            "release-key-03",
        )
        .await
        .unwrap();
    assert_eq!(response.value.state, "released");

    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"release-1\""
    );
    let sent: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["targetState"], "released");
    assert_eq!(sent["confirmIncompleteScope"], true);
    assert_eq!(sent["reason"], "GA day");
}

#[tokio::test]
async fn release_archive_and_restore_post_bodies_with_if_match() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/releases/22222222-2222-2222-2222-222222222222/archive",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"release-2\"")
                .set_body_json(release_json("archived", 2)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/releases/22222222-2222-2222-2222-222222222222/restore",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"release-3\"")
                .set_body_json(release_json("planned", 3)),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body = ReleaseVersionLifecycleRequest {
        reason: Some("superseded by 0.5.0".to_string()),
    };
    let archived = client
        .archive_release_version(
            "acme",
            "HAM",
            "22222222-2222-2222-2222-222222222222",
            &body,
            "\"release-1\"",
            "release-key-04",
        )
        .await
        .unwrap();
    assert_eq!(archived.value.state, "archived");
    let restored = client
        .restore_release_version(
            "acme",
            "HAM",
            "22222222-2222-2222-2222-222222222222",
            &ReleaseVersionLifecycleRequest::default(),
            "\"release-2\"",
            "release-key-05",
        )
        .await
        .unwrap();
    assert_eq!(restored.value.state, "planned");

    let requests = server.received_requests().await.unwrap();
    let archive = &requests
        .iter()
        .find(|request| request.url.path().ends_with("/archive"))
        .unwrap();
    assert_eq!(
        archive.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"release-1\""
    );
    let sent: serde_json::Value = serde_json::from_slice(&archive.body).unwrap();
    assert_eq!(sent["reason"], "superseded by 0.5.0");
    let restore = &requests
        .iter()
        .find(|request| request.url.path().ends_with("/restore"))
        .unwrap();
    let sent: serde_json::Value = serde_json::from_slice(&restore.body).unwrap();
    assert_eq!(sent, json!({}));
}

#[tokio::test]
async fn release_scope_sends_filters_and_parses_typed_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/releases/22222222-2222-2222-2222-222222222222/scope",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "release": {"id": "22222222-2222-2222-2222-222222222222", "state": "planned", "totalWorkItems": 3, "completedWorkItems": 1},
                "items": [{
                    "id": "1", "key": "HAM-1", "title": "Add releases", "type": "feature",
                    "status": "done", "priority": "high", "revision": 4,
                    "assigneeId": null, "archivedAt": null
                }],
                "page": {"limit": 50, "total": 3, "hasMore": true, "nextCursor": "abc"}
            })),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let query = ReleaseScopeQuery {
        limit: Some(50),
        cursor: None,
        query: Some("release".to_string()),
        status: Some("done".to_string()),
        item_type: Some("feature".to_string()),
        priority: Some("high".to_string()),
        assignee_id: Some("usr_x".to_string()),
        sort: Some("status".to_string()),
        direction: Some("asc".to_string()),
    };
    let response = client
        .get_release_version_scope("acme", "HAM", "22222222-2222-2222-2222-222222222222", query)
        .await
        .unwrap();
    assert_eq!(response.value.items.len(), 1);
    assert_eq!(response.value.page.total, 3);
    assert_eq!(response.value.page.next_cursor.as_deref(), Some("abc"));

    let request = &server.received_requests().await.unwrap()[0];
    let query = request.url.query().unwrap_or_default();
    assert!(query.contains("q=release"), "{query}");
    assert!(query.contains("type=feature"), "{query}");
    assert!(query.contains("assigneeId=usr_x"), "{query}");
}

#[tokio::test]
async fn work_item_release_membership_post_sends_mode_and_ids() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/releases",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "workItemId": "1",
            "workItemRevision": 9,
            "releaseVersions": [{
                "id": "22222222-2222-2222-2222-222222222222",
                "name": "Hamstik 0.4.0",
                "displayVersion": "0.4.0",
                "state": "planned",
                "archivedAt": null
            }]
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body = MutateWorkItemReleaseVersionsRequest {
        mode: "add".to_string(),
        release_version_ids: vec!["22222222-2222-2222-2222-222222222222".to_string()],
        confirm_released_scope_correction: Some(true),
        reason: Some("backfilled".to_string()),
    };
    let response = client
        .mutate_work_item_release_versions(
            "acme",
            "HAM",
            "HAM-1",
            &body,
            "\"wi-8\"",
            "release-key-06",
        )
        .await
        .unwrap();
    assert_eq!(response.value.work_item_revision, 9);
    assert_eq!(response.value.release_versions.len(), 1);

    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"wi-8\""
    );
    let sent: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["mode"], "add");
    assert_eq!(sent["confirmReleasedScopeCorrection"], true);
}

#[tokio::test]
async fn bulk_release_membership_posts_operations_envelope() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/release-memberships/bulk",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [{
                "index": 0,
                "status": 200,
                "workItemKey": "HAM-1",
                "workItemRevision": 9,
                "releaseVersions": [],
                "error": null
            }],
            "partial": false
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body = BulkReleaseMembershipRequest {
        operations: vec![BulkReleaseMembershipOperation {
            work_item_key: "HAM-1".to_string(),
            revision: 8,
            mode: "replace".to_string(),
            release_version_ids: vec!["22222222-2222-2222-2222-222222222222".to_string()],
            confirm_released_scope_correction: None,
            reason: Some("bulk fix".to_string()),
        }],
    };
    let response = client
        .bulk_mutate_release_memberships("acme", "HAM", &body, "release-key-07")
        .await
        .unwrap();
    assert!(!response.value.partial);
    assert_eq!(response.value.results[0].status, 200);

    let request = &server.received_requests().await.unwrap()[0];
    let sent: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["operations"][0]["workItemKey"], "HAM-1");
    assert_eq!(sent["operations"][0]["revision"], 8);
}

#[tokio::test]
async fn announcement_draft_lifecycle_round_trips() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/releases/22222222-2222-2222-2222-222222222222/announcements/draft",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "releaseVersionId": "22222222-2222-2222-2222-222222222222",
                "revision": 1,
                "introduction": "This release ships releases.",
                "highlights": ["Release versions"],
                "categories": [{"id": "features", "title": "Features", "workItemIds": ["1"]}],
                "items": [{
                    "id": "1", "key": "HAM-1", "title": "Add releases", "type": "feature",
                    "itemType": "feature", "status": "done", "include": true, "categoryId": "features", "position": 0
                }],
                "sourceReleaseRevision": 5,
                "generatedAt": "2026-09-02T00:00:00Z",
                "updatedAt": "2026-09-02T00:00:00Z",
                "diff": {"added": [], "removed": [], "changed": ["HAM-1"]}
            })),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/releases/22222222-2222-2222-2222-222222222222/announcements/draft",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "releaseVersionId": "22222222-2222-2222-2222-222222222222",
                "revision": 2,
                "introduction": "Edited introduction.",
                "highlights": [],
                "categories": [],
                "items": [],
                "sourceReleaseRevision": 5,
                "generatedAt": "2026-09-02T00:00:00Z",
                "updatedAt": "2026-09-02T01:00:00Z",
                "diff": null
            })),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/releases/22222222-2222-2222-2222-222222222222/announcements/draft",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let draft = client
        .get_release_announcement_draft("acme", "HAM", "22222222-2222-2222-2222-222222222222")
        .await
        .unwrap();
    assert_eq!(draft.value.revision, 1);
    assert!(draft.value.diff.is_some());

    let edit_body = EditReleaseAnnouncementDraftRequest {
        revision: draft.value.revision,
        introduction: "Edited introduction.".to_string(),
        highlights: vec![],
        categories: vec![],
        included_work_item_ids: vec![],
    };
    let edited = client
        .edit_release_announcement_draft(
            "acme",
            "HAM",
            "22222222-2222-2222-2222-222222222222",
            &edit_body,
        )
        .await
        .unwrap();
    assert_eq!(edited.value.revision, 2);

    client
        .archive_release_announcement_draft(
            "acme",
            "HAM",
            "22222222-2222-2222-2222-222222222222",
            &AnnouncementDraftRevisionRequest { revision: 2 },
        )
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let delete = &requests
        .iter()
        .find(|request| request.method.as_str() == "DELETE")
        .unwrap();
    let sent: serde_json::Value = serde_json::from_slice(&delete.body).unwrap();
    assert_eq!(sent["revision"], 2);
}

#[tokio::test]
async fn announcement_generate_and_publish_send_idempotency() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/releases/22222222-2222-2222-2222-222222222222/announcements/draft",
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({
                "releaseVersionId": "22222222-2222-2222-2222-222222222222",
                "revision": 1,
                "introduction": "Generated.",
                "highlights": [],
                "categories": [],
                "items": [],
                "sourceReleaseRevision": 5,
                "generatedAt": "2026-09-02T00:00:00Z",
                "updatedAt": "2026-09-02T00:00:00Z",
                "diff": null
            })),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/releases/22222222-2222-2222-2222-222222222222/announcements/publish",
        ))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({
                "id": "66666666-6666-6666-6666-666666666666",
                "revision": 1
            })),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let generated = client
        .generate_release_announcement_draft(
            "acme",
            "HAM",
            "22222222-2222-2222-2222-222222222222",
            &GenerateReleaseAnnouncementDraftRequest {
                revision: Some(5),
                confirm_replace_organization: Some(true),
            },
            "announcement-key-01",
        )
        .await
        .unwrap();
    assert_eq!(generated.value.revision, 1);

    let published = client
        .publish_release_announcement(
            "acme",
            "HAM",
            "22222222-2222-2222-2222-222222222222",
            &PublishReleaseAnnouncementRequest {
                revision: 1,
                confirm_empty: Some(true),
                reason: Some("GA".to_string()),
            },
            "announcement-key-02",
        )
        .await
        .unwrap();
    assert_eq!(published.value.revision, 1);

    let requests = server.received_requests().await.unwrap();
    let publish = &requests
        .iter()
        .find(|request| request.url.path().ends_with("/publish"))
        .unwrap();
    assert_eq!(
        publish
            .headers
            .get("idempotency-key")
            .unwrap()
            .to_str()
            .unwrap(),
        "announcement-key-02"
    );
    let sent: serde_json::Value = serde_json::from_slice(&publish.body).unwrap();
    assert_eq!(sent["confirmEmpty"], true);
}

#[tokio::test]
async fn audit_report_generate_get_and_download() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/release-audit-reports",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "77777777-7777-7777-7777-777777777777",
            "kind": "dossier",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "generatedAt": "2026-09-02T00:00:00Z"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/release-audit-reports/77777777-7777-7777-7777-777777777777",
        ))
        .and(query_param("format", "json"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"report": "frozen"})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/release-audit-reports/77777777-7777-7777-7777-777777777777",
        ))
        .and(query_param("format", "csv"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/zip")
                .set_body_bytes(b"PK-zip-bytes".to_vec()),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body = GenerateReleaseAuditReportRequest::Dossier {
        release_version_id: "22222222-2222-2222-2222-222222222222".to_string(),
    };
    let generated = client
        .generate_release_audit_report("acme", "HAM", &body, "audit-key-01")
        .await
        .unwrap();
    assert_eq!(generated.value.kind, "dossier");

    let package = client
        .get_release_audit_report("acme", "HAM", "77777777-7777-7777-7777-777777777777")
        .await
        .unwrap();
    assert_eq!(package.value["report"], "frozen");

    let download = client
        .download_release_audit_report("acme", "HAM", "77777777-7777-7777-7777-777777777777")
        .await
        .unwrap();
    assert_eq!(download.bytes, b"PK-zip-bytes");

    let requests = server.received_requests().await.unwrap();
    let post = &requests[0];
    let sent: serde_json::Value = serde_json::from_slice(&post.body).unwrap();
    assert_eq!(sent["kind"], "dossier");
    assert_eq!(
        sent["releaseVersionId"],
        "22222222-2222-2222-2222-222222222222"
    );
    let zip = &requests[2];
    assert_eq!(
        zip.headers.get("accept").unwrap().to_str().unwrap(),
        "application/zip"
    );
    assert!(zip.url.query().unwrap_or_default().contains("format=csv"));
}

#[tokio::test]
async fn audit_report_register_body_and_list_filters() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/release-audit-reports",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "77777777-7777-7777-7777-777777777777",
            "kind": "register",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "generatedAt": "2026-09-02T00:00:00Z"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/release-audit-reports",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{
                "id": "77777777-7777-7777-7777-777777777777",
                "kind": "register",
                "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "generatedAt": "2026-09-02T00:00:00Z"
            }],
            "page": {"limit": 50, "nextCursor": null}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body = GenerateReleaseAuditReportRequest::Register {
        from: "2026-09-01T00:00:00Z".to_string(),
        through: "2026-09-30T00:00:00Z".to_string(),
    };
    let generated = client
        .generate_release_audit_report("acme", "HAM", &body, "audit-key-02")
        .await
        .unwrap();
    assert_eq!(generated.value.kind, "register");

    let listed = client
        .list_release_audit_reports(
            "acme",
            "HAM",
            hamstik_api_client::ListReleaseAuditReportsOptions {
                limit: Some(50),
                cursor: None,
                release_version_id: Some("22222222-2222-2222-2222-222222222222".to_string()),
            },
        )
        .await
        .unwrap();
    assert_eq!(listed.value.items.len(), 1);

    let request = &server.received_requests().await.unwrap()[1];
    let query = request.url.query().unwrap_or_default();
    assert!(query.contains("releaseVersionId=22222222"), "{query}");
}

#[tokio::test]
async fn milestone_lifecycle_sends_if_match_and_state_filters() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/milestones"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("ETag", "\"milestone-1\"")
                .set_body_json(milestone_json("planned", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/milestones"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [milestone_json("planned", 1)],
            "page": {"limit": 100, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/milestones/44444444-4444-4444-4444-444444444444/transitions",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"milestone-2\"")
                .set_body_json(milestone_json("completed", 2)),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let created = client
        .create_organization_milestone(
            "acme",
            &CreateOrganizationMilestoneRequest {
                name: "Q4 platform hardening".to_string(),
                description: Some(None),
                owner_id: None,
                target_date: None,
            },
            "milestone-key-01",
        )
        .await
        .unwrap();
    assert_eq!(created.value.revision, 1);

    client
        .list_organization_milestones(
            "acme",
            ListMilestonesOptions {
                limit: Some(100),
                cursor: None,
                state: Some("planned".to_string()),
                include_archived: Some(false),
            },
        )
        .await
        .unwrap();

    let transitioned = client
        .transition_organization_milestone(
            "acme",
            "44444444-4444-4444-4444-444444444444",
            &TransitionOrganizationMilestoneRequest {
                target_state: "completed".to_string(),
                reason: Some("all releases shipped".to_string()),
            },
            "\"milestone-1\"",
            "milestone-key-02",
        )
        .await
        .unwrap();
    assert_eq!(transitioned.value.state, "completed");

    let requests = server.received_requests().await.unwrap();
    let post = &requests[0];
    assert_eq!(
        post.headers
            .get("idempotency-key")
            .unwrap()
            .to_str()
            .unwrap(),
        "milestone-key-01"
    );
    let list_query = requests[1].url.query().unwrap_or_default();
    assert!(list_query.contains("state=planned"), "{list_query}");
    let transition = &requests[2];
    assert_eq!(
        transition
            .headers
            .get("if-match")
            .unwrap()
            .to_str()
            .unwrap(),
        "\"milestone-1\""
    );
    let sent: serde_json::Value = serde_json::from_slice(&transition.body).unwrap();
    assert_eq!(sent["targetState"], "completed");
    assert_eq!(sent["reason"], "all releases shipped");
}

#[tokio::test]
async fn milestone_update_sends_if_match_and_null_clears() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/milestones/44444444-4444-4444-4444-444444444444",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"milestone-2\"")
                .set_body_json(milestone_json("planned", 2)),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body = UpdateOrganizationMilestoneRequest {
        name: Some(Some("Renamed milestone".to_string())),
        description: Some(None),
        target_date: Some(None),
        ..UpdateOrganizationMilestoneRequest::default()
    };
    let updated = client
        .update_organization_milestone(
            "acme",
            "44444444-4444-4444-4444-444444444444",
            &body,
            "\"milestone-1\"",
            "milestone-key-03",
        )
        .await
        .unwrap();
    assert_eq!(updated.value.revision, 2);

    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(request.method.as_str(), "PATCH");
    assert_eq!(
        request.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"milestone-1\""
    );
    let sent: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["name"], "Renamed milestone");
    assert_eq!(sent["description"], serde_json::Value::Null);
    assert_eq!(sent["targetDate"], serde_json::Value::Null);
}

#[tokio::test]
async fn milestone_detail_releases_and_events_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/milestones/44444444-4444-4444-4444-444444444444",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"milestone-1\"")
                .set_body_json(json!({
                    "id": "44444444-4444-4444-4444-444444444444",
                    "organizationId": "55555555-5555-5555-5555-555555555555",
                    "name": "Q4 platform hardening",
                    "description": null,
                    "ownerId": null,
                    "ownerPublicId": null,
                    "ownerName": null,
                    "targetDate": null,
                    "state": "in_progress",
                    "stateBeforeArchive": null,
                    "completedAt": null,
                    "archivedAt": null,
                    "revision": 3,
                    "createdAt": "2026-09-01T00:00:00Z",
                    "updatedAt": "2026-09-01T00:00:00Z",
                    "releases": [{
                        "id": "22222222-2222-2222-2222-222222222222",
                        "name": "Hamstik 0.4.0",
                        "state": "released",
                        "projectId": "33333333-3333-3333-3333-333333333333",
                        "projectName": "HAM",
                        "projectKey": "HAM",
                        "targetDate": null,
                        "releaseDate": "2026-09-24T00:00:00Z",
                        "totalWorkItems": 12,
                        "completedWorkItems": 12
                    }],
                    "page": {"limit": 20, "hasMore": false, "nextAfter": null},
                    "scope": "authorized_projects"
                })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/milestones/44444444-4444-4444-4444-444444444444/releases",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "items": [],
                "page": {"limit": 50, "hasMore": true, "nextAfter": "33333333-3333-3333-3333-333333333333"},
                "scope": "authorized_projects"
            })),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/milestones/44444444-4444-4444-4444-444444444444/events",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({
                "items": [{"id": "e1", "kind": "milestone.created", "createdAt": "2026-09-01T00:00:00Z"}],
                "nextBeforeEventId": "e1"
            })),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let detail = client
        .get_organization_milestone(
            "acme",
            "44444444-4444-4444-4444-444444444444",
            MilestoneDetailQuery {
                release_after: Some("22222222-2222-2222-2222-222222222222".to_string()),
                limit: Some(20),
            },
        )
        .await
        .unwrap();
    assert_eq!(detail.value.milestone.revision, 3);
    assert_eq!(detail.value.releases.len(), 1);
    assert_eq!(detail.etag.as_deref(), Some("\"milestone-1\""));

    let releases = client
        .list_organization_milestone_releases(
            "acme",
            "44444444-4444-4444-4444-444444444444",
            MilestoneReleasesQuery {
                cursor: None,
                limit: Some(50),
            },
        )
        .await
        .unwrap();
    assert!(releases.value.page.has_more);

    let events = client
        .list_organization_milestone_events(
            "acme",
            "44444444-4444-4444-4444-444444444444",
            MilestoneEventsQuery {
                before_event_id: None,
                limit: Some(25),
            },
        )
        .await
        .unwrap();
    assert_eq!(events.value.next_before_event_id.as_deref(), Some("e1"));

    let detail_request = &server.received_requests().await.unwrap()[0];
    let query = detail_request.url.query().unwrap_or_default();
    assert!(query.contains("releaseAfter=22222222"), "{query}");
    assert!(query.contains("limit=20"), "{query}");
}

#[tokio::test]
async fn milestone_release_membership_sends_confirm_remove() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/milestones/44444444-4444-4444-4444-444444444444/releases",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"milestone-2\"")
                .set_body_json(milestone_json("in_progress", 2)),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body = MutateOrganizationMilestoneReleaseRequest {
        release_version_id: "22222222-2222-2222-2222-222222222222".to_string(),
        mode: "remove".to_string(),
        confirm_remove: Some(true),
        reason: Some("moved to another milestone".to_string()),
    };
    let response = client
        .mutate_organization_milestone_release(
            "acme",
            "44444444-4444-4444-4444-444444444444",
            &body,
            "\"milestone-1\"",
            "milestone-key-03",
        )
        .await
        .unwrap();
    assert_eq!(response.value.revision, 2);

    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"milestone-1\""
    );
    let sent: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["mode"], "remove");
    assert_eq!(sent["confirmRemove"], true);
}
