// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Integration tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! End-to-end transport tests for [`HamstikClient`] against a local wiremock.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hamstik_api_client::retry::NoopSleeper;
use hamstik_api_client::{
    AttachLabelRequest, AvatarOptions, BulkCreateWorkItemOperation,
    BulkTransitionWorkItemOperation, BulkUpdateWorkItemOperation, ClientConfig,
    CreateCommentRequest, CreateWorkItemRequest, HamstikApi, HamstikClient, ListOptions,
    ListProjectsOptions, PageItems, RetryPolicy, TransitionRequest, UpdateWorkItemRequest,
    WatcherAction, WorkItemWatcher, follow_all,
};
use secrecy::SecretString;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

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

#[tokio::test]
async fn sends_auth_and_accept_and_captures_request_id() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-Request-Id", "req-123")
                .set_body_json(json!({
                    "id": "1",
                    "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg",
                    "name": "Steven",
                    "email": "s@example.com",
                    "authentication": {"type":"pat","credentialId":"c","credentialName":"n","scopes":[],"expiresAt":"2027-01-01T00:00:00Z"},
                    "defaultOrganization": null,
                    "organizations": []
                })),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client.whoami().await.unwrap();

    assert_eq!(resp.value.email, "s@example.com");
    assert_eq!(resp.request_id.as_deref(), Some("req-123"));

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let auth = received[0]
        .headers
        .get("authorization")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(auth, "Bearer test-token");
    assert_eq!(
        received[0].headers.get("accept").unwrap().to_str().unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn openapi_uses_the_public_client_without_authorization() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/openapi.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "openapi": "3.1.1",
            "paths": {}
        })))
        .mount(&server)
        .await;

    let host = hamstik_api_client::Host::parse(&server.uri()).unwrap();
    let client = HamstikClient::new_public(host, ClientConfig::default()).unwrap();
    let response = client.get_open_api().await.unwrap();
    assert_eq!(response.value["openapi"], "3.1.1");

    let request = &server.received_requests().await.unwrap()[0];
    assert!(!request.headers.contains_key("authorization"));
    assert_eq!(
        request.headers.get("accept").unwrap().to_str().unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn squeakql_posts_only_the_documented_json_without_idempotency() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/work-items/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "page": {"limit": 25, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/squeakql/validate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "valid": false,
            "languageVersion": 1,
            "errors": [{
                "code": "SQUEAKQL_UNKNOWN_FIELD",
                "message": "Unknown field",
                "line": 1,
                "column": 1,
                "token": "statuz",
                "suggestion": "status"
            }]
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let search = client
        .search_organization_work_items_with_squeakql(
            "acme",
            &hamstik_api_client::SqueakQlSearchRequest {
                query: "status = todo".into(),
                limit: Some(25),
                cursor: Some("opaque+/cursor==".into()),
            },
        )
        .await
        .unwrap();
    assert!(search.value.items.is_empty());
    let validation = client
        .validate_squeakql(
            "acme",
            &hamstik_api_client::SqueakQlValidateRequest {
                query: "statuz = todo".into(),
            },
        )
        .await
        .unwrap();
    assert!(!validation.value.valid);
    assert_eq!(
        validation.value.errors[0].code.as_str(),
        "SQUEAKQL_UNKNOWN_FIELD"
    );

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert!(request.url.query().is_none());
        assert!(!request.headers.contains_key("idempotency-key"));
        assert!(
            request
                .headers
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("application/json")
        );
    }
    let search_body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(search_body["query"], "status = todo");
    assert_eq!(search_body["limit"], 25);
    assert_eq!(search_body["cursor"], "opaque+/cursor==");
    assert_eq!(search_body.as_object().unwrap().len(), 3);
    let validation_body: serde_json::Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(validation_body, json!({"query": "statuz = todo"}));
}

#[tokio::test]
async fn maps_error_envelope_to_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {
                "code": "NOT_FOUND",
                "message": "no such organization",
                "fieldErrors": {"organizationSlug": ["does not exist"]},
                "details": {"organizationSlug": "acme", "retryable": false}
            },
            "requestId": "err-req"
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let err = client.get_organization("acme").await.unwrap_err();
    let api = err.as_api().expect("api error");
    assert_eq!(api.status, 404);
    assert_eq!(api.code, "NOT_FOUND");
    assert_eq!(api.message, "no such organization");
    assert_eq!(api.request_id.as_deref(), Some("err-req"));
    assert_eq!(api.field_errors["organizationSlug"], ["does not exist"]);
    assert_eq!(api.details.as_ref().unwrap()["organizationSlug"], "acme");
}

#[tokio::test]
async fn retries_transient_failure_then_succeeds() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    let body = json!({
        "id": "1", "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg",
        "name": "Steven", "email": "s@example.com",
        "authentication": {"type":"pat","credentialId":"c","credentialName":"n","scopes":[],"expiresAt":"2027-01-01T00:00:00Z"},
        "defaultOrganization": null,
        "organizations": []
    });
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(move |_: &Request| {
            let n = count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_json(body.clone())
            }
        })
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client.whoami().await.unwrap();
    assert_eq!(resp.value.name, "Steven");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn does_not_retry_patch() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .respond_with(ResponseTemplate::new(503).set_body_json(json!({
            "error": {"code":"INTERNAL_ERROR","message":"down"}, "requestId":"r"
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let body = UpdateWorkItemRequest {
        title: Some("x".to_string()),
        ..Default::default()
    };
    let err = client
        .update_work_item("acme", "HAM", "HAM-1", &body, "*")
        .await
        .unwrap_err();
    assert_eq!(err.as_api().unwrap().status, 503);
    // PATCH must be attempted exactly once.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// A `DELETE` is idempotent: when the first attempt fails transiently and the
/// retry finds the resource already gone, the desired end state holds and the
/// operation must report success instead of a false NOT_FOUND.
#[tokio::test]
async fn delete_treats_404_after_retry_as_success() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments/c1",
        ))
        .respond_with(move |_: &Request| {
            let attempt = count.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(404).set_body_json(json!({
                    "error": {"code": "NOT_FOUND", "message": "already deleted"}
                }))
            }
        })
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let response = client
        .delete_comment("acme", "HAM", "HAM-1", "c1")
        .await
        .unwrap();
    assert!(response.raw.is_null());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

/// A first-attempt 404 is a real not-found and is never retried, so it must
/// keep surfacing as NOT_FOUND.
#[tokio::test]
async fn delete_reports_first_attempt_404() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments/c1",
        ))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "error": {"code": "NOT_FOUND", "message": "no such comment"}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let err = client
        .delete_comment("acme", "HAM", "HAM-1", "c1")
        .await
        .unwrap_err();
    assert_eq!(err.as_api().unwrap().status, 404);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn concurrency_errors_preserve_precondition_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(412)
                .insert_header("X-Request-Id", "header-request")
                .set_body_json(json!({
                    "error": {
                        "code": "REVISION_CONFLICT",
                        "message": "revision changed",
                        "details": {"expectedRevision": 3, "currentRevision": 4}
                    },
                    "requestId": "body-request"
                })),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let error = client
        .update_work_item(
            "acme",
            "HAM",
            "HAM-1",
            &UpdateWorkItemRequest {
                title: Some("Changed".into()),
                ..Default::default()
            },
            "\"wi-3\"",
        )
        .await
        .unwrap_err();
    let api = error.as_api().unwrap();
    assert_eq!(api.status, 412);
    assert_eq!(api.code, "REVISION_CONFLICT");
    assert_eq!(api.request_id.as_deref(), Some("body-request"));
    assert_eq!(api.details.as_ref().unwrap()["currentRevision"], 4);
    let request = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        request.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"wi-3\""
    );
}

#[tokio::test]
async fn bearer_token_is_not_exposed_in_client_errors() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {"code": "INVALID_TOKEN", "message": "invalid credential"},
            "requestId": "auth-request"
        })))
        .mount(&server)
        .await;

    let error = client_for(&server.uri()).whoami().await.unwrap_err();
    assert!(!error.to_string().contains("test-token"));
    assert!(!format!("{error:?}").contains("test-token"));
}

#[tokio::test]
async fn honors_short_retry_after_and_retries_429() {
    let server = MockServer::start().await;
    let count = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations"))
        .respond_with(move |_: &Request| {
            let n = count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                ResponseTemplate::new(429).insert_header("Retry-After", "1")
            } else {
                ResponseTemplate::new(200).set_body_json(
                    json!({"items": [], "page": {"limit":50,"hasMore":false,"nextCursor":null}}),
                )
            }
        })
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client
        .list_organizations(ListOptions::default())
        .await
        .unwrap();
    assert!(resp.value.items.is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn gives_up_immediately_on_long_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "3600")
                .set_body_json(
                    json!({"error":{"code":"RATE_LIMITED","message":"slow down"},"requestId":"r"}),
                ),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let err = client
        .list_organizations(ListOptions::default())
        .await
        .unwrap_err();
    let api = err.as_api().unwrap();
    assert_eq!(api.code, "RATE_LIMITED");
    assert_eq!(api.status, 429);
    // Must not retry a >30s directive.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn captures_etag_and_idempotency_replay() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-7\"")
                .set_body_json(work_item_json()),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("ETag", "\"rev-1\"")
                .insert_header("Idempotency-Replayed", "true")
                .set_body_json(work_item_json()),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let got = client.get_work_item("acme", "HAM", "HAM-1").await.unwrap();
    assert_eq!(got.etag.as_deref(), Some("\"rev-7\""));

    let created = client
        .create_work_item(
            "acme",
            "HAM",
            &CreateWorkItemRequest {
                title: "T".to_string(),
                ..Default::default()
            },
            "key-12345678",
        )
        .await
        .unwrap();
    assert!(created.idempotency_replayed);

    let post = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    assert_eq!(
        post.headers
            .get("idempotency-key")
            .unwrap()
            .to_str()
            .unwrap(),
        "key-12345678"
    );
}

#[tokio::test]
async fn creates_comment_with_idempotency_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id":"c1","workItemId":"w1","parentCommentId":null,
            "author":{"id":"u","name":"U"},"body":"hello","deleted":false,
            "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z",
            "editedAt":null
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client
        .create_comment(
            "acme",
            "HAM",
            "HAM-1",
            &CreateCommentRequest {
                body: "hello".to_string(),
                parent_comment_id: None,
            },
            "comment-key-1",
        )
        .await
        .unwrap();
    assert_eq!(resp.value.body.as_deref(), Some("hello"));
}

#[tokio::test]
async fn transitions_send_if_match_and_idempotency() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/transitions",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"rev-2\"")
                .set_body_json(work_item_json()),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client
        .transition_work_item(
            "acme",
            "HAM",
            "HAM-1",
            &TransitionRequest {
                target_status: "in_progress".to_string(),
            },
            "\"rev-1\"",
            "transition-key-1",
        )
        .await
        .unwrap();
    assert_eq!(resp.etag.as_deref(), Some("\"rev-2\""));

    let req = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        req.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"rev-1\""
    );
    assert!(req.headers.contains_key("idempotency-key"));
}

#[tokio::test]
async fn follows_all_pages_through_trait() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects"))
        .respond_with(|req: &Request| {
            let has_cursor = req.url.query().unwrap_or_default().contains("cursor=next");
            let body = if has_cursor {
                json!({"items":[{"id":"2","organizationId":"o","key":"B","name":"B","description":null,"color":"#000","revision":1,"archivedAt":null,"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}],
                        "page":{"limit":1,"hasMore":false,"nextCursor":null}})
            } else {
                json!({"items":[{"id":"1","organizationId":"o","key":"A","name":"A","description":null,"color":"#000","revision":1,"archivedAt":null,"createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}],
                        "page":{"limit":1,"hasMore":true,"nextCursor":"next"}})
            };
            ResponseTemplate::new(200).set_body_json(body)
        })
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let collected = follow_all(|cursor| {
        let client = client.clone();
        async move {
            let opts = ListProjectsOptions {
                limit: Some(1),
                cursor,
                archived: None,
            };
            let resp = client.list_projects("acme", opts).await?;
            Ok(PageItems::new(resp.value.items, &resp.raw, resp.value.page))
        }
    })
    .await
    .unwrap();

    assert_eq!(collected.items.len(), 2);
    assert_eq!(collected.items[1].key, "B");
}

fn work_item_json() -> serde_json::Value {
    json!({
        "id":"1","key":"HAM-1","projectId":"2","title":"T","description":null,
        "type":"task","status":"todo","priority":"low","assignee":null,"reporter":null,
        "sprint":null,"parent":null,"labels":[],
        "storyPoints":null,"dueDate":null,"archivedAt":null,
        "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z","revision":4
    })
}

fn sprint_json(state: &str, revision: i64) -> serde_json::Value {
    json!({
        "id":"11111111-1111-1111-1111-111111111111","name":"Sprint 1","state":state,
        "startDate":null,"endDate":null,"goal":null,"targetPoints":null,
        "createdAt":"2026-08-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z","revision":revision
    })
}

fn attachment_json() -> serde_json::Value {
    json!({
        "id":"a1","workItemId":"w1","fileName":"design.png","contentType":"image/png",
        "size":11,"createdBy":{"id":"u","name":"U"},"createdAt":"2026-01-01T00:00:00Z"
    })
}

// ---- Hardening behaviors ---------------------------------------------------

/// A redirect response must surface as its own error and never be followed:
/// the bearer token must not travel to another origin, and the response must
/// not be presented as if it came from the original host.
#[tokio::test]
async fn redirects_are_never_followed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/evil"))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    assert!(client.whoami().await.is_err());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// An oversized response body must be rejected mid-stream instead of buffered:
/// a compromised host must not be able to exhaust memory.
#[tokio::test]
async fn oversized_response_body_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![
            b'x';
            hamstik_api_client::client::MAX_BODY_BYTES
                + 1
        ]))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let err = client.whoami().await.unwrap_err();
    assert!(err.to_string().contains("exceeds"), "{err}");
}

/// Terminal escape sequences from a hostile server must not survive into
/// error messages or request ids.
#[tokio::test]
async fn server_error_text_has_control_characters_stripped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "requestId": "req\u{1b}[31m-1",
            "error": {"code": "FORBIDDEN\u{1b}[2J", "message": "no\u{7}pe"}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let err = client.whoami().await.unwrap_err();
    let api = err.as_api().unwrap();
    // The ESC introducer is removed, so a printable tail cannot reassemble
    // into a live escape sequence.
    assert_eq!(api.code, "FORBIDDEN[2J");
    assert_eq!(api.message, "nope");
    assert_eq!(api.request_id.as_deref(), Some("req[31m-1"));
}

/// `--all` aggregation must terminate with an error against a server that
/// always claims there is another page.
#[tokio::test]
async fn follow_all_terminates_on_an_endless_server() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"id":"1","slug":"a","name":"A","suspended":false}],
            "page": {"limit": 1, "hasMore": true, "nextCursor": "same"}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let result = follow_all(|cursor| {
        let client = client.clone();
        async move {
            let resp = client
                .list_organizations(ListOptions {
                    limit: Some(1),
                    cursor,
                })
                .await?;
            Ok(PageItems::new(resp.value.items, &resp.raw, resp.value.page))
        }
    })
    .await;
    assert!(result.is_err(), "endless pagination must fail");
}

// ---- New surface: projects, sprints, labels, comments, attachments -------

#[tokio::test]
async fn creates_project_with_idempotency_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id":"p1","organizationId":"o1","key":"WEB","name":"Website",
            "description":null,"color":"#3b82f6","revision":1,"archivedAt":null,
            "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client
        .create_project(
            "acme",
            &hamstik_api_client::CreateProjectRequest {
                name: "Website".into(),
                key: Some("WEB".into()),
                description: None,
                color: Some("#3b82f6".into()),
            },
            "project-key-01",
        )
        .await
        .unwrap();
    assert_eq!(resp.value.key, "WEB");

    let req = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        req.headers
            .get("idempotency-key")
            .unwrap()
            .to_str()
            .unwrap(),
        "project-key-01"
    );
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["name"], "Website");
    assert_eq!(body["key"], "WEB");
}

#[tokio::test]
async fn sprint_lifecycle_sends_if_match_and_completion_action() {
    let server = MockServer::start().await;
    let sprint_id = "11111111-1111-1111-1111-111111111111";
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/sprints/{sprint_id}"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"sprint-1\"")
                .set_body_json(sprint_json("active", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/sprints/{sprint_id}/transitions"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "currentState":"active",
            "transitions":[{"targetState":"done","requiresCompletionAction":true}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/organizations/acme/projects/HAM/sprints/{sprint_id}/transitions"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"sprint-2\"")
                .set_body_json(sprint_json("done", 2)),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let current = client.get_sprint("acme", "HAM", sprint_id).await.unwrap();
    assert_eq!(current.etag.as_deref(), Some("\"sprint-1\""));

    let resp = client
        .transition_sprint(
            "acme",
            "HAM",
            sprint_id,
            &hamstik_api_client::TransitionSprintRequest {
                target_state: "done".to_string(),
                completion_action: Some(hamstik_api_client::CompletionAction::Backlog),
            },
            "\"sprint-1\"",
            "sprint-key-01",
        )
        .await
        .unwrap();
    assert_eq!(resp.value.state, "done");

    let req = &server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "POST")
        .unwrap();
    assert_eq!(
        req.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"sprint-1\""
    );
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["targetState"], "done");
    assert_eq!(body["completionAction"]["mode"], "backlog");
}

#[tokio::test]
async fn labels_list_create_and_attach_detach() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/labels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items":[{"id":"l1","name":"api","color":"#6366f1","createdAt":"2026-01-01T00:00:00Z"}],
            "page":{"limit":50,"hasMore":false,"nextCursor":null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/labels"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!(
            {"id":"l2","name":"cli","color":"#6366f1","createdAt":"2026-01-01T00:00:00Z"}
        )))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/labels",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-5\"")
                .set_body_json(work_item_json()),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/labels/l1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-6\"")
                .set_body_json(work_item_json()),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let listed = client
        .list_labels("acme", "HAM", ListOptions::default())
        .await
        .unwrap();
    assert_eq!(listed.value.items[0].name, "api");

    client
        .create_label(
            "acme",
            "HAM",
            &hamstik_api_client::CreateLabelRequest {
                name: "cli".into(),
                color: None,
            },
            "label-key-01",
        )
        .await
        .unwrap();

    let attached = client
        .attach_label(
            "acme",
            "HAM",
            "HAM-1",
            &AttachLabelRequest {
                label_id: Some("l1".into()),
                label: None,
            },
            "\"wi-4\"",
            "attach-key-01",
        )
        .await
        .unwrap();
    assert_eq!(attached.etag.as_deref(), Some("\"wi-5\""));

    let detached = client
        .detach_label("acme", "HAM", "HAM-1", "l1", "\"wi-5\"", "detach-key-01")
        .await
        .unwrap();
    assert!(detached.value.key == "HAM-1");

    let reqs = server.received_requests().await.unwrap();
    let label_post = reqs
        .iter()
        .find(|r| r.url.path().ends_with("/labels") && r.method.as_str() == "POST")
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&label_post.body).unwrap();
    assert_eq!(body["name"], "cli");
    // Attachments of labels use If-Match and idempotency keys.
    let attach = reqs
        .iter()
        .find(|r| r.url.path().ends_with("/work-items/HAM-1/labels"))
        .unwrap();
    assert_eq!(
        attach.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"wi-4\""
    );
}

#[tokio::test]
async fn deletes_comment_returns_void() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments/c1",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    client
        .delete_comment("acme", "HAM", "HAM-1", "c1")
        .await
        .unwrap();
}

#[tokio::test]
async fn comment_delete_maps_conflict() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments/c1",
        ))
        .respond_with(ResponseTemplate::new(409).set_body_json(json!({
            "error":{"code":"CONFLICT","message":"The Comment has replies."},"requestId":"r"
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let err = client
        .delete_comment("acme", "HAM", "HAM-1", "c1")
        .await
        .unwrap_err();
    assert_eq!(err.as_api().unwrap().code, "CONFLICT");
}

#[tokio::test]
async fn uploads_attachment_as_multipart_with_idempotency() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/attachments",
        ))
        .respond_with(
            ResponseTemplate::new(201)
                .insert_header("Location", "/api/v1/attachments/a1")
                .set_body_json(attachment_json()),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client
        .upload_attachment(
            "acme",
            "HAM",
            "HAM-1",
            &hamstik_api_client::client::MultipartFile {
                file_name: "design.png".to_string(),
                content_type: Some("image/png".to_string()),
                bytes: vec![1, 2, 3, 4],
            },
            "attach-key-01",
        )
        .await
        .unwrap();
    assert_eq!(resp.value.file_name, "design.png");
    assert_eq!(resp.location.as_deref(), Some("/api/v1/attachments/a1"));

    let req = &server.received_requests().await.unwrap()[0];
    let content_type = req.headers.get("content-type").unwrap().to_str().unwrap();
    assert!(
        content_type.starts_with("multipart/form-data"),
        "{content_type}"
    );
    assert_eq!(
        req.headers
            .get("idempotency-key")
            .unwrap()
            .to_str()
            .unwrap(),
        "attach-key-01"
    );
    let body = String::from_utf8_lossy(&req.body);
    assert!(body.contains("filename=\"design.png\""), "{body}");
    assert!(body.contains("form-data; name=\"file\""), "{body}");
}

#[tokio::test]
async fn downloads_attachment_bytes_and_file_name() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/attachments/a1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header(
                    "Content-Disposition",
                    "attachment; filename*=UTF-8''design%20one.png",
                )
                .insert_header("Content-Type", "image/png")
                .insert_header("X-Request-Id", "req-dl")
                .set_body_bytes(vec![1, 2, 3, 4]),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let download = client
        .download_attachment("acme", "HAM", "HAM-1", "a1")
        .await
        .unwrap();
    assert_eq!(download.bytes, vec![1, 2, 3, 4]);
    assert_eq!(download.file_name.as_deref(), Some("design one.png"));
    assert_eq!(download.content_type.as_deref(), Some("image/png"));
    assert_eq!(download.request_id.as_deref(), Some("req-dl"));
}

#[tokio::test]
async fn deletes_attachment_returns_void() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/attachments/a1",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    client
        .delete_attachment("acme", "HAM", "HAM-1", "a1")
        .await
        .unwrap();
}

#[tokio::test]
async fn sprint_create_sends_payload() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/sprints"))
        .respond_with(ResponseTemplate::new(201).set_body_json(sprint_json("future", 1)))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client
        .create_sprint(
            "acme",
            "HAM",
            &hamstik_api_client::CreateSprintRequest {
                name: "Sprint 1".into(),
                start_date: Some(Some("2026-09-01T00:00:00Z".into())),
                ..Default::default()
            },
            "sprint-create-1",
        )
        .await
        .unwrap();
    assert_eq!(resp.value.state, "future");

    let body: serde_json::Value =
        serde_json::from_slice(&server.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(body["name"], "Sprint 1");
    assert_eq!(body["startDate"], "2026-09-01T00:00:00Z");
    assert!(body.get("targetPoints").is_none());
}

#[tokio::test]
async fn me_exposes_public_id_and_memberships() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id":"u1","publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"N","email":"n@x",
            "authentication":{"type":"pat","credentialId":"c","credentialName":"n","scopes":[],"expiresAt":"2027-01-01T00:00:00Z"},
            "defaultOrganization":null,
            "organizations":[{"id":"o1","slug":"acme","name":"Acme","username":"n"}]
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let me = client.whoami().await.unwrap();
    assert_eq!(me.value.public_id, "usr_cPbfeqnghA-RLpDVOMQhHg");
    assert_eq!(me.value.organizations[0].username.as_deref(), Some("n"));
}

// ---- Updated API surface: profiles, directories, lifecycle, links, bulk ---

fn work_item_public_json(status: &str, revision: i64) -> serde_json::Value {
    json!({
        "id":"1","key":"HAM-1","projectId":"2","title":"T","description":null,
        "type":"task","status":status,"priority":"low",
        "assignee":{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"A"},"reporter":null,
        "sprint":null,"parent":null,"labels":[],
        "storyPoints":null,"dueDate":null,"archivedAt":null,
        "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z","revision":revision
    })
}

#[tokio::test]
async fn lists_organization_users_with_query() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/users"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items":[{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"Steven","username":"steven"}],
            "page":{"limit":50,"hasMore":false,"nextCursor":null}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client
        .list_organization_users(
            "acme",
            hamstik_api_client::ListOrganizationUsersOptions {
                q: Some("ste".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(resp.value.items[0].public_id, "usr_cPbfeqnghA-RLpDVOMQhHg");
    let url = &server.received_requests().await.unwrap()[0].url;
    assert!(url.query().unwrap_or_default().contains("q=ste"));
}

#[tokio::test]
async fn lists_my_work_with_context() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/my/work"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items":[{
                "id":"1","key":"HAM-1","revision":2,"title":"T","status":"todo",
                "assignee":{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"A"},
                "project":{"id":"p","key":"HAM","name":"Ham","color":"#000000"},
                "organization":{"id":"o","slug":"acme","name":"Acme"}
            }],
            "page":{"limit":50,"hasMore":false,"nextCursor":null}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client
        .list_my_work(hamstik_api_client::ListWorkItemsQuery {
            scope: Some("open".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(resp.value.items[0].project.key, "HAM");
    assert_eq!(resp.value.items[0].organization.slug, "acme");
}

#[tokio::test]
async fn lists_profile_work_with_reporter() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/usr_cPbfeqnghA-RLpDVOMQhHg/work"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items":[{
                "id":"1","key":"HAM-1","revision":2,"title":"T","status":"todo",
                "assignee":{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"A"},
                "reporter":{"publicId":"usr_OtherUserPublicIdAAAAQ","name":"R"},
                "project":{"id":"p","key":"HAM","name":"Ham","color":"#000000"},
                "organization":{"id":"o","slug":"acme","name":"Acme"}
            }],
            "page":{"limit":50,"hasMore":false,"nextCursor":null}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client
        .list_user_profile_work(
            "usr_cPbfeqnghA-RLpDVOMQhHg",
            hamstik_api_client::ListWorkItemsQuery::default(),
        )
        .await
        .unwrap();
    assert_eq!(resp.value.items[0].summary.project.key, "HAM");
    let reporter = resp.value.items[0].reporter.as_ref().unwrap();
    assert_eq!(reporter.public_id(), Some("usr_OtherUserPublicIdAAAAQ"));
    assert_eq!(reporter.name(), "R");
}

/// The `archived` query parameter is an archived-state filter, not an
/// "include archived" union. These boundary tests pin the exact serialization
/// for every Public API operation that accepts `archived`: omitted stays
/// absent, `false`/`true` serialize literally, and the server treats an
/// omitted value as `false` (only unarchived resources).
#[tokio::test]
async fn archived_filter_serialization_is_exact_across_collections() {
    // (operation name, request future factory) pairs keep the three states
    // symmetric without five copies of the same test body.
    enum Collection {
        ProjectWorkItems,
        OrganizationWorkItems,
        MyWork,
        ProfileWork,
        Projects,
    }

    async fn run_case(
        server: &MockServer,
        collection: Collection,
        archived: Option<bool>,
        expected_query: Option<&str>,
    ) {
        let client = client_for(&server.uri());
        let work_query = || hamstik_api_client::ListWorkItemsQuery {
            archived,
            ..Default::default()
        };
        let project_query = || hamstik_api_client::ListProjectsOptions {
            archived,
            ..Default::default()
        };
        match collection {
            Collection::ProjectWorkItems => {
                client
                    .list_work_items("acme", "HAM", work_query())
                    .await
                    .unwrap();
            }
            Collection::OrganizationWorkItems => {
                client
                    .list_organization_work_items("acme", work_query())
                    .await
                    .unwrap();
            }
            Collection::MyWork => {
                client.list_my_work(work_query()).await.unwrap();
            }
            Collection::ProfileWork => {
                client
                    .list_user_profile_work("usr_cPbfeqnghA-RLpDVOMQhHg", work_query())
                    .await
                    .unwrap();
            }
            Collection::Projects => {
                client.list_projects("acme", project_query()).await.unwrap();
            }
        }

        let actual = server
            .received_requests()
            .await
            .unwrap()
            .last()
            .unwrap()
            .url
            .query()
            .unwrap_or_default()
            .to_string();
        match expected_query {
            Some(expected) => {
                assert_eq!(actual, expected, "unexpected query for {archived:?}");
            }
            None => {
                assert!(!actual.contains("archived"), "unexpected query: {actual}");
            }
        }
    }

    let work_body = json!({"items":[],"page":{"limit":50,"hasMore":false,"nextCursor":null}});

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_body.clone()))
        .mount(&server)
        .await;
    run_case(&server, Collection::ProjectWorkItems, None, None).await;
    run_case(
        &server,
        Collection::ProjectWorkItems,
        Some(false),
        Some("archived=false"),
    )
    .await;
    run_case(
        &server,
        Collection::ProjectWorkItems,
        Some(true),
        Some("archived=true"),
    )
    .await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_body.clone()))
        .mount(&server)
        .await;
    run_case(&server, Collection::OrganizationWorkItems, None, None).await;
    run_case(
        &server,
        Collection::OrganizationWorkItems,
        Some(false),
        Some("archived=false"),
    )
    .await;
    run_case(
        &server,
        Collection::OrganizationWorkItems,
        Some(true),
        Some("archived=true"),
    )
    .await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/my/work"))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_body.clone()))
        .mount(&server)
        .await;
    run_case(&server, Collection::MyWork, None, None).await;
    run_case(
        &server,
        Collection::MyWork,
        Some(false),
        Some("archived=false"),
    )
    .await;
    run_case(
        &server,
        Collection::MyWork,
        Some(true),
        Some("archived=true"),
    )
    .await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/usr_cPbfeqnghA-RLpDVOMQhHg/work"))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_body.clone()))
        .mount(&server)
        .await;
    run_case(&server, Collection::ProfileWork, None, None).await;
    run_case(
        &server,
        Collection::ProfileWork,
        Some(false),
        Some("archived=false"),
    )
    .await;
    run_case(
        &server,
        Collection::ProfileWork,
        Some(true),
        Some("archived=true"),
    )
    .await;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items":[],
            "page":{"limit":50,"hasMore":false,"nextCursor":null}
        })))
        .mount(&server)
        .await;
    run_case(&server, Collection::Projects, None, None).await;
    run_case(
        &server,
        Collection::Projects,
        Some(false),
        Some("archived=false"),
    )
    .await;
    run_case(
        &server,
        Collection::Projects,
        Some(true),
        Some("archived=true"),
    )
    .await;
}

#[tokio::test]
async fn profile_work_serializes_repeated_filters_booleans_fields_and_opaque_cursor() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/usr_cPbfeqnghA-RLpDVOMQhHg/work"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "page": {"limit": 17, "hasMore": false, "nextCursor": null}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    client
        .list_user_profile_work(
            "usr_cPbfeqnghA-RLpDVOMQhHg",
            hamstik_api_client::ListWorkItemsQuery {
                limit: Some(17),
                cursor: Some("opaque+/cursor==".into()),
                q: Some("launch plan".into()),
                status: vec!["todo".into(), "in_progress".into()],
                scope: Some("open".into()),
                item_type: vec!["task".into(), "bug".into()],
                priority: vec!["high".into(), "urgent".into()],
                sprint: Some("none".into()),
                label: vec!["label-1".into(), "label-2".into()],
                label_name: vec!["api".into(), "frontend".into()],
                parent: Some("HAM-1".into()),
                top_level: Some(true),
                updated_after: Some("2026-09-01T00:00:00Z".into()),
                projects: vec!["HAM".into(), "WEB".into()],
                organizations: vec!["acme".into(), "labs".into()],
                involvement: vec!["assigned".into(), "commented".into()],
                overdue: Some(false),
                due_before: Some("2026-10-01T00:00:00Z".into()),
                due_after: Some("2026-09-01T00:00:00Z".into()),
                sort: Some("dueDate".into()),
                archived: Some(true),
                fields: Some("title,status,dueDate".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let request = &server.received_requests().await.unwrap()[0];
    let pairs: Vec<(String, String)> = request
        .url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(pairs.iter().filter(|(key, _)| key == "status").count(), 2);
    assert_eq!(pairs.iter().filter(|(key, _)| key == "type").count(), 2);
    assert_eq!(pairs.iter().filter(|(key, _)| key == "label").count(), 2);
    assert_eq!(
        pairs
            .iter()
            .filter(|(key, _)| key == "organization")
            .count(),
        2
    );
    assert!(pairs.contains(&("topLevel".into(), "true".into())));
    assert!(pairs.contains(&("overdue".into(), "false".into())));
    assert!(pairs.contains(&("archived".into(), "true".into())));
    assert!(pairs.contains(&("cursor".into(), "opaque+/cursor==".into())));
    assert!(pairs.contains(&("fields".into(), "title,status,dueDate".into())));
}

#[tokio::test]
async fn reads_user_profile() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/usr_cPbfeqnghA-RLpDVOMQhHg"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"Steven",
            "avatarUrl":null,"joinedAt":"2026-01-15T14:30:00.000Z",
            "isCurrentUser":true,"sharedOrganizations":[],
            "stats":{"projects":1,"workItemsAssigned":2,"workItemsCreated":3,"workItemsCompleted":4,"comments":5}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let profile = client
        .get_user_profile("usr_cPbfeqnghA-RLpDVOMQhHg")
        .await
        .unwrap();
    assert!(profile.value.is_current_user);
    assert_eq!(profile.value.stats.work_items_assigned, 2);
}

#[tokio::test]
async fn downloads_avatar_bytes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/users/usr_cPbfeqnghA-RLpDVOMQhHg/avatar"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "image/png")
                .insert_header("Cache-Control", "public, max-age=3600")
                .insert_header("X-Request-Id", "avatar-request")
                .set_body_bytes(vec![9, 9, 9]),
        )
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let avatar = client
        .get_user_profile_avatar(
            "usr_cPbfeqnghA-RLpDVOMQhHg",
            AvatarOptions {
                version: Some("v2".into()),
                format: Some("webp".into()),
                revision: Some("opaque-rev".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(avatar.bytes, vec![9, 9, 9]);
    assert_eq!(avatar.content_type.as_deref(), Some("image/png"));
    assert_eq!(
        avatar.cache_control.as_deref(),
        Some("public, max-age=3600")
    );
    assert_eq!(avatar.request_id.as_deref(), Some("avatar-request"));
    let request = &server.received_requests().await.unwrap()[0];
    let pairs: Vec<_> = request.url.query_pairs().collect();
    assert!(pairs.iter().any(|pair| pair == &("v".into(), "v2".into())));
    assert!(
        pairs
            .iter()
            .any(|pair| pair == &("format".into(), "webp".into()))
    );
    assert!(
        pairs
            .iter()
            .any(|pair| pair == &("rev".into(), "opaque-rev".into()))
    );
    assert_eq!(
        request.headers.get("accept").unwrap().to_str().unwrap(),
        "image/jpeg, image/png, image/webp, application/json"
    );
}

#[tokio::test]
async fn project_lifecycle_updates_and_archives() {
    let server = MockServer::start().await;
    let project_json = json!({
        "id":"p1","organizationId":"o1","key":"WEB","name":"Website",
        "description":null,"color":"#3b82f6","revision":2,"archivedAt":null,
        "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"
    });
    Mock::given(method("PATCH"))
        .and(path("/api/v1/organizations/acme/projects/WEB"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"project-2\"")
                .set_body_json(project_json.clone()),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/WEB/archive"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id":"p1","organizationId":"o1","key":"WEB","name":"Website",
            "description":null,"color":"#3b82f6","revision":3,
            "archivedAt":"2026-04-01T00:00:00Z",
            "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-04-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let updated = client
        .update_project(
            "acme",
            "WEB",
            &hamstik_api_client::UpdateProjectRequest {
                name: Some("Website".into()),
                ..Default::default()
            },
            "\"project-1\"",
            "project-update-01",
        )
        .await
        .unwrap();
    assert_eq!(updated.etag.as_deref(), Some("\"project-2\""));

    let archived = client
        .archive_project("acme", "WEB", "*", "project-archive-01")
        .await
        .unwrap();
    assert!(archived.value.archived_at.is_some());

    let reqs = server.received_requests().await.unwrap();
    let patch = reqs.iter().find(|r| r.method.as_str() == "PATCH").unwrap();
    assert_eq!(
        patch.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"project-1\""
    );
    let body: serde_json::Value = serde_json::from_slice(&patch.body).unwrap();
    assert_eq!(body["name"], "Website");
    let archive = reqs
        .iter()
        .find(|r| r.url.path().ends_with("/archive"))
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&archive.body).unwrap();
    assert_eq!(body, json!({}));
}

#[tokio::test]
async fn work_item_links_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/links"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items":[{
                "id":"l1","relation":"blocks",
                "otherWorkItem":{"id":"2","key":"HAM-2","project":{"id":"p","key":"HAM","name":"Ham"},"title":"Other","type":"task","status":"todo"},
                "createdBy":{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"A"},"createdAt":"2026-01-01T00:00:00Z"
            }],
            "page":{"limit":50,"hasMore":false,"nextCursor":null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/links"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id":"l2","relation":"relates",
            "otherWorkItem":{"id":"3","key":"HAM-3","project":{"id":"p","key":"HAM","name":"Ham"},"title":"Third","type":"bug","status":"todo"},
            "createdBy":null,"createdAt":"2026-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/links/l2",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let links = client
        .list_work_item_links("acme", "HAM", "HAM-1", ListOptions::default())
        .await
        .unwrap();
    assert_eq!(links.value.items[0].other_work_item.key, "HAM-2");

    let created = client
        .create_work_item_link(
            "acme",
            "HAM",
            "HAM-1",
            &hamstik_api_client::CreateWorkItemLinkRequest {
                target_key: Some("HAM-3".into()),
                relation: "relates".into(),
                ..Default::default()
            },
            "link-create-01",
        )
        .await
        .unwrap();
    assert_eq!(created.value.relation, "relates");

    client
        .delete_work_item_link("acme", "HAM", "HAM-1", "l2", "link-delete-01")
        .await
        .unwrap();

    let body: serde_json::Value = serde_json::from_slice(
        &server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.method.as_str() == "POST")
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(body["targetKey"], "HAM-3");
    assert_eq!(body["relation"], "relates");
}

#[tokio::test]
async fn work_item_and_project_activity_parse() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/activity"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items":[{"id":"a1","action":"created","actor":null,"detail":null,"createdAt":"2026-01-01T00:00:00Z"}],
            "page":{"limit":50,"hasMore":false,"nextCursor":null}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/projects/HAM/activity"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items":[{"id":"a2","action":"status_changed","actor":{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"A"},"detail":{"from":"todo","to":"done"},"createdAt":"2026-01-02T00:00:00Z","workItem":{"id":"1","key":"HAM-1","title":"T"}}],
            "page":{"limit":50,"hasMore":false,"nextCursor":null}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let wi_activity = client
        .list_work_item_activity(
            "acme",
            "HAM",
            "HAM-1",
            hamstik_api_client::ActivityOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(wi_activity.value.items[0].action, "created");

    let project_activity = client
        .list_project_activity(
            "acme",
            "HAM",
            hamstik_api_client::ActivityOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(project_activity.value.items[0].work_item.key, "HAM-1");
}

#[tokio::test]
async fn work_item_delete_sends_cascade_and_headers() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    client
        .delete_work_item("acme", "HAM", "HAM-1", true, "\"wi-4\"", "delete-key-01")
        .await
        .unwrap();

    let req = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        req.headers.get("if-match").unwrap().to_str().unwrap(),
        "\"wi-4\""
    );
    assert!(req.headers.contains_key("idempotency-key"));
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["cascade"], true);
}

#[tokio::test]
async fn work_item_archive_unarchive_send_empty_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/archive",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-5\"")
                .set_body_json(work_item_public_json("done", 5)),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/unarchive",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_item_public_json("done", 6)))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let archived = client
        .archive_work_item("acme", "HAM", "HAM-1", "\"wi-4\"", "archive-key-01")
        .await
        .unwrap();
    assert_eq!(archived.etag.as_deref(), Some("\"wi-5\""));
    // The public mutation projection carries publicId user summaries.
    assert_eq!(
        archived.value.assignee.as_ref().unwrap().public_id(),
        Some("usr_cPbfeqnghA-RLpDVOMQhHg")
    );
    client
        .unarchive_work_item("acme", "HAM", "HAM-1", "\"wi-5\"", "unarchive-key-01")
        .await
        .unwrap();
}

#[tokio::test]
async fn comment_edit_sends_body_and_idempotency() {
    let server = MockServer::start().await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments/c1",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id":"c1","workItemId":"w1","parentCommentId":null,
            "author":{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"A"},
            "body":"edited","deleted":false,
            "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z",
            "editedAt":"2026-01-02T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client
        .update_comment(
            "acme",
            "HAM",
            "HAM-1",
            "c1",
            &hamstik_api_client::UpdateCommentRequest {
                body: "edited".into(),
            },
            "edit-key-01",
        )
        .await
        .unwrap();
    assert_eq!(
        resp.value.edited_at.as_deref(),
        Some("2026-01-02T00:00:00Z")
    );
}

#[tokio::test]
async fn idempotent_patch_retry_reuses_the_same_key_and_body() {
    let server = MockServer::start().await;
    let attempts = Arc::new(AtomicUsize::new(0));
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments/c1",
        ))
        .respond_with(move |_: &Request| {
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(503)
            } else {
                ResponseTemplate::new(200).set_body_json(json!({
                    "id":"c1","workItemId":"w1","parentCommentId":null,
                    "author":{"id":"u","name":"U"},"body":"edited","deleted":false,
                    "createdAt":"2026-01-01T00:00:00Z",
                    "updatedAt":"2026-01-02T00:00:00Z","editedAt":"2026-01-02T00:00:00Z"
                }))
            }
        })
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    client
        .update_comment(
            "acme",
            "HAM",
            "HAM-1",
            "c1",
            &hamstik_api_client::UpdateCommentRequest {
                body: "edited".into(),
            },
            "same-patch-key",
        )
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].body, requests[1].body);
    for request in &requests {
        assert_eq!(
            request
                .headers
                .get("idempotency-key")
                .unwrap()
                .to_str()
                .unwrap(),
            "same-patch-key"
        );
    }
}

#[tokio::test]
async fn bulk_routes_send_envelopes_and_parse_results() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/bulk-work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results":[{"index":0,"status":201,"workItem":work_item_public_json("todo", 1)}]
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
    Mock::given(method("POST"))
        .and(path("/api/v1/organizations/acme/bulk-work-item-transitions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results":[{"index":0,"status":409,"error":{"code":"INVALID_STATUS_TRANSITION","message":"no"}}]
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let created = client
        .bulk_create_work_items(
            "acme",
            &hamstik_api_client::BulkCreateEnvelope {
                operations: vec![BulkCreateWorkItemOperation {
                    project_key: "HAM".into(),
                    title: "First".into(),
                }],
            },
            "bulk-create-01",
        )
        .await
        .unwrap();
    assert_eq!(created.value.results[0].status, 201);

    let updated = client
        .bulk_update_work_items(
            "acme",
            &hamstik_api_client::BulkUpdateEnvelope {
                concurrency: "last-write-wins".into(),
                operations: vec![BulkUpdateWorkItemOperation {
                    project_key: "HAM".into(),
                    work_item_key: "HAM-1".into(),
                    revision: None,
                    changes: UpdateWorkItemRequest {
                        priority: Some("high".into()),
                        ..Default::default()
                    },
                }],
            },
            "bulk-update-01",
        )
        .await
        .unwrap();
    assert_eq!(updated.value.results[0].status, 200);

    let transitioned = client
        .bulk_transition_work_items(
            "acme",
            &hamstik_api_client::BulkTransitionEnvelope {
                concurrency: None,
                operations: vec![BulkTransitionWorkItemOperation {
                    project_key: "HAM".into(),
                    work_item_key: "HAM-1".into(),
                    revision: Some(1),
                    target_status: "done".into(),
                }],
            },
            "bulk-transition-01",
        )
        .await
        .unwrap();
    assert_eq!(
        transitioned.value.results[0].error.as_ref().unwrap()["code"],
        "INVALID_STATUS_TRANSITION"
    );

    let reqs = server.received_requests().await.unwrap();
    let create_body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(create_body["operations"][0]["projectKey"], "HAM");
    let update_body: serde_json::Value = serde_json::from_slice(&reqs[1].body).unwrap();
    assert_eq!(update_body["concurrency"], "last-write-wins");
}

#[tokio::test]
async fn org_and_project_work_collections_parse() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme/work-items"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items":[{"id":"1","key":"HAM-1","revision":1,
                "project":{"id":"p","key":"HAM","name":"Ham","color":"#000000"},
                "organization":{"id":"o","slug":"acme","name":"Acme"}}],
            "page":{"limit":50,"hasMore":false,"nextCursor":null}
        })))
        .mount(&server)
        .await;

    let client = client_for(&server.uri());
    let resp = client
        .list_organization_work_items(
            "acme",
            hamstik_api_client::ListWorkItemsQuery {
                projects: vec!["HAM".into()],
                sort: Some("dueDate".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(resp.value.items[0].project.key, "HAM");
    let url = &server.received_requests().await.unwrap()[0].url;
    let query = url.query().unwrap_or_default();
    assert!(query.contains("project=HAM"), "{query}");
    assert!(query.contains("sort=dueDate"), "{query}");
}

#[tokio::test]
async fn watcher_get_and_action_send_documented_requests() {
    let watcher = WorkItemWatcher {
        work_item_id: "33333333-3333-4333-8333-333333333333".to_string(),
        watched: true,
        manual_watch: true,
        assignee_origin: false,
        muted: false,
        assigned: true,
    };

    // GET: read-only, no idempotency header, no If-Match.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/watcher",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-Request-Id", "watch-req-1")
                .set_body_json(&watcher),
        )
        .mount(&server)
        .await;
    let client = client_for(&server.uri());
    let resp = client
        .get_work_item_watcher("acme", "HAM", "HAM-1")
        .await
        .unwrap();
    assert_eq!(resp.value, watcher);
    assert_eq!(resp.request_id.as_deref(), Some("watch-req-1"));
    let req = &server.received_requests().await.unwrap()[0];
    assert!(!req.headers.contains_key("idempotency-key"));
    assert!(!req.headers.contains_key("if-match"));

    // POST: action body serialized as the documented enum spelling, with the
    // required Idempotency-Key and no revision guard.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/watcher",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("X-Request-Id", "watch-req-2")
                .set_body_json(&watcher),
        )
        .mount(&server)
        .await;
    let client = client_for(&server.uri());
    let resp = client
        .update_work_item_watcher("acme", "HAM", "HAM-1", WatcherAction::Mute, "watcher-key-1")
        .await
        .unwrap();
    assert!(resp.value.watched);
    assert!(!resp.value.muted);
    let req = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        req.headers
            .get("idempotency-key")
            .unwrap()
            .to_str()
            .unwrap(),
        "watcher-key-1"
    );
    let sent: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(
        sent["action"], "mute",
        "the action must serialize as the documented string"
    );
    assert!(
        sent.get("workItemId").is_none(),
        "the server sets the id; the request carries only the action"
    );
}
