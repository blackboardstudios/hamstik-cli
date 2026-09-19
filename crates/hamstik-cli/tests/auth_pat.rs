// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! PAT onboarding, expiry, scope-readiness, and credential-precedence tests
//!.

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn me_json(scopes: &[&str], expires_at: &str) -> Value {
    json!({
        "id": "11111111-1111-4111-8111-111111111111",
        "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg",
        "name": "Steven", "email": "steven@example.com",
        "authentication": {"type": "pat", "credentialId": "22222222-2222-4222-8222-222222222222", "credentialName": "cli-pat",
                           "scopes": scopes, "expiresAt": expires_at},
        "defaultOrganization": null,
        "organizations": [{"id": "44444444-4444-4444-8444-444444444444", "slug": "acme", "name": "Acme", "username": "steven"}]
    })
}

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    // Keep test mutations out of the developer's real audit log.
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

/// An RFC 3339 timestamp `seconds` from now (UTC).
fn timestamp(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let secs_of_day = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// `auth status --json` reports credential type, expiry, source, and the
/// structured scope report without any secret material; near-expiry produces
/// a nonblocking warning while expiry data stays intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_status_reports_readiness_without_secrets() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    for (name, expires_at, expect_success) in [
        ("healthy", timestamp(now + 30 * 86_400), true),
        // One hour of slack: `days_until` counts whole 24-hour periods from
        // the CLI's clock read, which happens after this test captured `now`.
        // Without slack the count flips to 2 whenever the CLI reads the clock
        // in the next wall-clock second (the CI flake this guards against).
        ("near-expiry", timestamp(now + 3 * 86_400 + 3_600), true),
        ("expired", timestamp(now - 3_600), false),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/me"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(me_json(&["read", "work:write"], &expires_at)),
            )
            .mount(&server)
            .await;

        let dir = TempDir::new().unwrap();
        let output = base(&server, &dir)
            .args(["auth", "status", "--json"])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            if expect_success { Some(0) } else { Some(3) },
            "`{name}` exit code drifted: {:?}",
            output.stderr
        );
        let body: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(body["credential"]["type"], "pat");
        assert_eq!(body["credential"]["name"], "cli-pat");
        assert_eq!(body["credential"]["expiresAt"], expires_at);
        assert_eq!(
            body["credentialSource"],
            "environment (HAMSTIK_TOKEN; ephemeral, never stored)"
        );
        assert_eq!(body["scopes"]["count"], 2);
        assert_eq!(body["scopes"]["granted"][1], "work:write");
        assert!(
            body["scopes"]["note"]
                .as_str()
                .unwrap()
                .contains("the server decides"),
            "scope report must defer authority to the server"
        );
        let text = String::from_utf8_lossy(&output.stderr);
        assert!(
            !text.contains("secret-token"),
            "the PAT value must never appear in diagnostics: {text}"
        );
        if name == "near-expiry" {
            assert!(
                text.contains("expires in 3 day(s)"),
                "near-expiry warning missing: {text}"
            );
        }
        if name == "expired" {
            assert!(text.contains("EXPIRED"), "expired warning missing: {text}");
        }
    }
}

/// Expired-credential failures carry the stable exit code, server code,
/// HTTP status, and request ID; the CLI adds remediation to the message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_credentials_preserve_structured_error_data() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {"code": "AUTH_REQUIRED", "message": "invalid token", "status": 401},
            "requestId": "auth-401"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["auth", "status", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    let error = &body["error"];
    assert_eq!(error["code"], "AUTH_REQUIRED");
    assert_eq!(error["status"], 401);
    assert_eq!(error["requestId"], "auth-401");
    assert!(
        error["message"]
            .as_str()
            .unwrap_or_default()
            .contains("create a new PAT"),
        "remediation must be CLI-added to the message"
    );

    // Human output keeps the same structured code and adds remediation.
    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["auth", "status"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(text.contains("AUTH_REQUIRED"), "{text}");
    assert!(text.contains("create a new PAT"), "{text}");
    assert!(!text.contains("secret-token"));
}

/// Environment-token precedence: the request carries the env token and the
/// persistent store is neither read nor modified by any auth surface.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn environment_token_takes_precedence_without_store_access() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(me_json(&["read"], "2099-01-01T00:00:00Z")),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["auth", "status", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let req = &server.received_requests().await.unwrap()[0];
    assert_eq!(
        req.headers.get("authorization").unwrap().to_str().unwrap(),
        "Bearer secret-token",
        "the environment token must be used verbatim"
    );

    // The config file was never created by a status call: no profile was
    // written and no store entry exists (the store itself is not even
    // addressable for an ephemeral token).
    assert!(
        !dir.path().join("config.toml").exists(),
        "auth status with an environment token must not create profile state"
    );
}

/// `auth login --with-token` reads stdin (never argv), stores the credential,
/// and reports scopes in the success envelope without echoing the token.
///
/// Hosts without an OS credential service (headless Linux CI runners) fail
/// closed with exit 10 / `CREDENTIAL_STORE_ERROR` (SPEC §26); hosts with a
/// real keyring complete the login. Either way the token read from stdin must
/// reach credential handling and never be echoed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_with_token_reads_stdin_and_never_echoes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(me_json(&["read"], "2099-01-01T00:00:00Z")),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let mut cmd = base(&server, &dir);
    cmd.env_remove("HAMSTIK_TOKEN");
    cmd.args(["--no-input", "--json", "auth", "login", "--with-token"]);
    cmd.write_stdin("stdin-pat-value-4\n");
    let output = cmd.output().unwrap();

    let stderr_text = String::from_utf8_lossy(&output.stderr);
    let stdout_text = String::from_utf8_lossy(&output.stdout);
    match output.status.code() {
        Some(0) => {
            let body: Value =
                serde_json::from_slice(&output.stdout).expect("login success is JSON");
            assert_eq!(body["authenticated"], true);
        }
        Some(10) => {
            let body: Value = serde_json::from_slice(&output.stderr).unwrap_or_else(|error| {
                panic!("exit 10 must carry the error envelope: {error}; stderr: {stderr_text}")
            });
            assert_eq!(
                body["error"]["code"], "CREDENTIAL_STORE_ERROR",
                "keyring-less hosts must fail closed with the documented code: {stderr_text}"
            );
        }
        other => panic!("unexpected exit {other:?}; stdout: {stdout_text}; stderr: {stderr_text}"),
    }
    for stream in [output.stdout.as_slice(), output.stderr.as_slice()] {
        assert!(
            !String::from_utf8_lossy(stream).contains("stdin-pat-value-4"),
            "the token must never be echoed back"
        );
    }
    // The stored profile carries the identity; the config file holds no token.
    // (On keyring-less hosts no login completed, so no profile state exists
    // and only the no-token rule applies.)
    let config_text = std::fs::read_to_string(dir.path().join("config.toml")).unwrap_or_default();
    assert!(
        !config_text.contains("stdin-pat-value-4"),
        "tokens must never be written to the config file"
    );

    // HAMSTIK_TOKEN present is never stored: explicit refusal.
    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["--no-input", "--json", "auth", "login"])
        .write_stdin("whatever\n")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("ephemeral and is never stored"),
        "environment-token persistence guidance drifted"
    );
}

/// `me` and `auth status` human output surface expiry and scope inventory
/// without leaking credential values.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn me_and_status_human_output_surface_readiness() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(me_json(&[], "2099-01-01T00:00:00Z")),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir).args(["me"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("valid for"),
        "me must surface the expiry summary: {text}"
    );
    assert!(
        text.contains("(none granted"),
        "empty scope inventory must be visible: {text}"
    );
    assert!(!text.contains("secret-token"));

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["auth", "status"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("source:    environment"), "{text}");
    assert!(text.contains("scopes:    (none granted"), "{text}");
}

/// Doctor reuses the same thresholds: near-expiry warns, expired fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_expiry_uses_shared_thresholds() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/openapi.json"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::from_str::<Value>(include_str!("../../../openapi/hamstik-v1.json"))
                    .unwrap(),
            ),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(
            ResponseTemplate::new(200)
                // One hour of slack keeps `expires in 3 day(s)` stable across
                // the CLI's later clock read (see the near-expiry note in
                // auth_status_reports_readiness_without_secrets).
                .set_body_json(me_json(&["read"], &timestamp(now + 3 * 86_400 + 3_600))),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/organizations/acme"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "33333333-3333-4333-8333-333333333333",
            "slug": "acme",
            "name": "Acme",
            "description": "Test org",
            "plan": "free",
            "role": "owner",
            "isDefault": false,
            "suspended": false,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args(["--no-input", "--json", "--org", "acme", "doctor"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    let expiry = body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "credential.expiry")
        .unwrap();
    assert_eq!(expiry["status"], "warn", "shared 14-day threshold drifted");
    assert!(
        expiry["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("expires in 3 day(s)"),
        "{expiry}"
    );
}

/// Login's stdin path works under `--no-input` for CI and the rollback
/// removes an orphaned credential when the profile save fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_stdin_path_is_deterministic_and_redacted() {
    // Server rejects the token: login must not persist anything and must
    // surface the structured 401 with remediation.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "error": {"code": "AUTH_REQUIRED", "message": "invalid token", "status": 401},
            "requestId": "login-401"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let mut cmd = base(&server, &dir);
    cmd.env_remove("HAMSTIK_TOKEN");
    cmd.args(["--no-input", "--json", "auth", "login", "--with-token"]);
    cmd.write_stdin("bad-token\n");
    let output = cmd.output().unwrap();
    assert_eq!(output.status.code(), Some(3));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(body["error"]["code"], "AUTH_REQUIRED");
    assert_eq!(body["error"]["requestId"], "login-401");
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("bad-token"),
        "failed-login diagnostics must not echo the token"
    );
}
