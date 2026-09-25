// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Doctor typed-network-failure and local-only mode tests.

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::MockServer;

fn me_json(scopes: &[&str], expires_at: &str) -> Value {
    json!({
        "id": "11111111-1111-4111-8111-111111111111",
        "publicId": "usr_cPbfeqnghA-RLpDVOMQhHg",
        "name": "Steven",
        "email": "steven@example.com",
        "authentication": {
            "type": "pat",
            "credentialId": "22222222-2222-4222-8222-222222222222",
            "credentialName": "cli-pat",
            "scopes": scopes,
            "expiresAt": expires_at
        },
        "defaultOrganization": null,
        "organizations": []
    })
}

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_AUDIT_LOG", dir.path().join("audit.log"));
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
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
    cmd.env(
        "HAMSTIK_REQUEST_JOURNAL",
        dir.path().join("request-journal.log"),
    );
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env("HAMSTIK_HOST", "http://127.0.0.1:1");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn check<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == id)
        .unwrap_or_else(|| panic!("missing check {id} in {:?}", body["checks"]))
}

/// Local-only mode emits no DNS or HTTP traffic (the host is unreachable)
/// and clearly marks every remote check skipped while local checks pass.
#[test]
fn local_only_mode_makes_no_traffic_and_skips_remote_checks() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "doctor",
            "--local-only",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "local-only checks pass locally: {:?}",
        output.stderr
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();

    for id in [
        "network.connectivity",
        "network.tls",
        "api.openapi",
        "api.authentication",
        "context.organization",
        "context.project",
        "credential.expiry",
        "scope.readiness",
    ] {
        assert_eq!(
            check(&body, id)["status"],
            "skipped",
            "{id} must be skipped in local-only mode"
        );
    }
    // Offline compatibility still validates against the bundled snapshot.
    assert_eq!(check(&body, "api.compatibility")["status"], "pass");
    assert_eq!(check(&body, "local.config")["status"], "pass");
    assert_eq!(
        body["summary"]["skipped"].as_u64().unwrap(),
        11,
        "summary must total the skipped remote checks: {body}"
    );
    assert_eq!(
        body["summary"]["pass"].as_u64().unwrap()
            + body["summary"]["warn"].as_u64().unwrap()
            + body["summary"]["fail"].as_u64().unwrap()
            + body["summary"]["skipped"].as_u64().unwrap(),
        body["checks"].as_array().unwrap().len() as u64,
        "summary must cover every check"
    );
}

/// An unreachable host reports the earliest failing stage; dependent checks
/// are skipped while local/terminal checks still run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_failure_classifies_stage_and_skips_dependents() {
    let dir = TempDir::new().unwrap();
    let output = local(&dir)
        .args(["--no-input", "--json", "doctor"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(8), "network exit code drifted");
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();

    let connectivity = check(&body, "network.connectivity");
    assert_eq!(connectivity["status"], "fail");
    let stage = connectivity["networkStage"].as_str().unwrap_or_default();
    assert!(
        ["connection", "dns", "unknown"].contains(&stage),
        "unreachable host should classify as connection/dns: {connectivity}"
    );
    assert!(
        connectivity["remediation"].is_string(),
        "stage-specific remediation is required"
    );
    assert_eq!(check(&body, "api.openapi")["status"], "skipped");
    assert_eq!(check(&body, "api.compatibility")["status"], "skipped");
    assert_eq!(check(&body, "api.authentication")["status"], "skipped");
    // Independent local checks still ran.
    assert_eq!(check(&body, "local.config")["status"], "pass");
    assert_eq!(check(&body, "terminal.color")["status"], "warn");
}

/// Healthy, imminently expiring, and expired credentials produce distinct
/// expiry classifications.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pat_expiry_thresholds_are_distinguished() {
    struct Case {
        name: &'static str,
        expires_at: String,
        expected: &'static str,
    }
    let far_future = "2099-01-01T00:00:00Z";
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    // 3 days out: inside the 14-day warn window.
    let imminent = humantime_timestamp(now_secs + 3 * 86_400);
    // 30 minutes ago: expired.
    let expired = humantime_timestamp(now_secs - 1_800);

    let cases = [
        Case {
            name: "healthy",
            expires_at: far_future.to_string(),
            expected: "pass",
        },
        Case {
            name: "imminent",
            expires_at: imminent,
            expected: "warn",
        },
        Case {
            name: "expired",
            expires_at: expired,
            expected: "fail",
        },
    ];
    for case in &cases {
        let server = MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/openapi.json"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(
                    serde_json::from_str::<Value>(include_str!("../../../openapi/hamstik-v1.json"))
                        .unwrap(),
                ),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/me"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(me_json(&["read", "write"], &case.expires_at)),
            )
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/api/v1/organizations/acme"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
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
        // An expired credential is an authentication-class failure (exit 3);
        // healthy and imminent-expiry runs stay successful.
        let expected_exit = if case.expected == "fail" { 3 } else { 0 };
        assert_eq!(
            output.status.code(),
            Some(expected_exit),
            "`{}` exit code drifted: {:?}",
            case.name,
            output.stderr
        );
        let body: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            check(&body, "credential.expiry")["status"],
            case.expected,
            "`{}` expiry classification drifted: {}",
            case.name,
            check(&body, "credential.expiry")
        );
    }
}

/// Scope readiness surfaces the scope inventory without inventing authority.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scope_readiness_reports_inventory() {
    let server = MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/v1/openapi.json"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::from_str::<Value>(include_str!("../../../openapi/hamstik-v1.json"))
                    .unwrap(),
            ),
        )
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/v1/me"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(me_json(&[], "2099-01-01T00:00:00Z")),
        )
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/v1/organizations/acme"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
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
    let scope_check = check(&body, "scope.readiness");
    assert_eq!(scope_check["status"], "warn");
    assert!(
        scope_check["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("no scopes"),
        "empty scope inventory must be flagged: {scope_check}"
    );
}

/// Redaction: no PAT, bearer header, or secret-bearing value reaches any
/// output stream, including diagnostics for failed checks.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_output_never_leaks_credentials() {
    // Wrong token → 401 responses; the token value must never be echoed.
    let server = MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/v1/openapi.json"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::from_str::<Value>(include_str!("../../../openapi/hamstik-v1.json"))
                    .unwrap(),
            ),
        )
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/v1/me"))
        .respond_with(wiremock::ResponseTemplate::new(401).set_body_json(json!({
            "error": {"code": "AUTH_REQUIRED", "message": "invalid token", "status": 401},
            "requestId": "doctor-401"
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let mut cmd = base(&server, &dir);
    // Replace the helper's generic token with a distinctive secret.
    cmd.env("HAMSTIK_TOKEN", "super-secret-pat-doctor-13");
    let output = cmd
        .args(["--no-input", "--json", "--org", "acme", "doctor"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "auth exit code drifted");
    for stream in [output.stdout.as_slice(), output.stderr.as_slice()] {
        let text = String::from_utf8_lossy(stream);
        assert!(
            !text.contains("super-secret-pat-doctor-13"),
            "credential leaked into output: {text}"
        );
        assert!(
            !text.contains("authorization:"),
            "authorization header must never be echoed: {text}"
        );
    }
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(check(&body, "api.authentication")["status"], "fail");
    assert_eq!(
        check(&body, "api.authentication")["error"]["requestId"],
        "doctor-401",
        "request ids must be preserved through doctor"
    );
}

/// `--diff` compares a freshly generated local-only bundle against a saved
/// one: a changed host is a named diff, and identical bundles report an
/// explicit "no differences" result in both JSON and human output.
#[test]
fn diff_reports_named_changes_and_identical_bundles_have_no_differences() {
    let dir = TempDir::new().unwrap();
    let baseline = dir.path().join("baseline.zip");

    // Baseline bundle captured at host A.
    let saved = local(&dir)
        .env("HAMSTIK_HOST", "http://127.0.0.1:1")
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "doctor",
            "--local-only",
            "--bundle",
        ])
        .arg(&baseline)
        .output()
        .unwrap();
    assert_eq!(saved.status.code(), Some(0), "{:?}", saved.stderr);
    assert!(baseline.exists(), "baseline bundle must be written");

    // Re-running in the identical state reports no differences.
    let identical = local(&dir)
        .env("HAMSTIK_HOST", "http://127.0.0.1:1")
        .args(["--no-input", "--json", "--org", "acme", "doctor", "--diff"])
        .arg(&baseline)
        .output()
        .unwrap();
    assert_eq!(identical.status.code(), Some(0), "{:?}", identical.stderr);
    let body: Value = serde_json::from_slice(&identical.stdout).unwrap();
    assert_eq!(body["noDifferences"], true, "{body}");
    assert_eq!(body["count"], 0, "{body}");
    assert!(body["differences"].as_array().unwrap().is_empty(), "{body}");

    let human = local(&dir)
        .env("HAMSTIK_HOST", "http://127.0.0.1:1")
        .args(["--no-input", "--org", "acme", "doctor", "--diff"])
        .arg(&baseline)
        .output()
        .unwrap();
    assert_eq!(human.status.code(), Some(0), "{:?}", human.stderr);
    assert!(
        String::from_utf8_lossy(&human.stdout).contains("no differences"),
        "human output must state no differences: {:?}",
        String::from_utf8_lossy(&human.stdout)
    );

    // A different host is reported as a named check diff.
    let changed = local(&dir)
        .env("HAMSTIK_HOST", "http://127.0.0.1:2")
        .args(["--no-input", "--json", "--org", "acme", "doctor", "--diff"])
        .arg(&baseline)
        .output()
        .unwrap();
    assert_eq!(changed.status.code(), Some(0), "{:?}", changed.stderr);
    let body: Value = serde_json::from_slice(&changed.stdout).unwrap();
    assert_eq!(body["noDifferences"], false, "{body}");
    assert!(
        body["differences"].as_array().unwrap().iter().any(|entry| {
            entry["name"] == "local.host"
                && entry["kind"] == "changed"
                && entry["path"] == "checks[local.host].detail"
        }),
        "host change must be a named diff: {body}"
    );
}

/// A changed Organization scope between bundles is reported by the context
/// resolution check and the context-explain chain.
#[test]
fn diff_reports_context_scope_changes() {
    let dir = TempDir::new().unwrap();
    let baseline = dir.path().join("baseline.zip");

    let saved = local(&dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "doctor",
            "--local-only",
            "--bundle",
        ])
        .arg(&baseline)
        .output()
        .unwrap();
    assert_eq!(saved.status.code(), Some(0), "{:?}", saved.stderr);

    let changed = local(&dir)
        .args(["--no-input", "--json", "--org", "other", "doctor", "--diff"])
        .arg(&baseline)
        .output()
        .unwrap();
    assert_eq!(changed.status.code(), Some(0), "{:?}", changed.stderr);
    let body: Value = serde_json::from_slice(&changed.stdout).unwrap();
    let differences = body["differences"].as_array().unwrap();
    assert!(
        differences.iter().any(|entry| {
            entry["name"] == "local.context_resolution" && entry["kind"] == "changed"
        }),
        "organization change must be a named diff: {body}"
    );
    assert!(
        differences
            .iter()
            .any(|entry| entry["file"] == "context-explain.json"),
        "organization change must also surface in context-explain: {body}"
    );
}

/// A saved bundle that is missing, corrupt, or not a ZIP fails clearly and
/// never silently reports "no differences".
#[test]
fn diff_rejects_unreadable_bundle() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("missing.zip");
    let output = local(&dir)
        .args(["--no-input", "--json", "doctor", "--diff"])
        .arg(&missing)
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(0), "missing bundle must fail");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        text.contains("missing.zip"),
        "error must name the bundle: {text}"
    );

    let corrupt = dir.path().join("corrupt.zip");
    std::fs::write(&corrupt, b"not a zip").unwrap();
    let output = local(&dir)
        .args(["--no-input", "--json", "doctor", "--diff"])
        .arg(&corrupt)
        .output()
        .unwrap();
    assert_ne!(output.status.code(), Some(0), "corrupt bundle must fail");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        text.contains("corrupt.zip"),
        "error must name the bundle: {text}"
    );
}

/// `--diff` and `--bundle` may target the same file: the saved baseline is
/// read before the fresh bundle overwrites it, so the comparison still runs
/// against the saved state.
#[test]
fn diff_and_bundle_same_path_compares_saved_baseline() {
    let dir = TempDir::new().unwrap();
    let bundle = dir.path().join("bundle.zip");

    let saved = local(&dir)
        .args([
            "--no-input",
            "--json",
            "--org",
            "acme",
            "doctor",
            "--local-only",
            "--bundle",
        ])
        .arg(&bundle)
        .output()
        .unwrap();
    assert_eq!(saved.status.code(), Some(0), "{:?}", saved.stderr);

    let output = local(&dir)
        .args(["--no-input", "--json", "--org", "acme", "doctor", "--diff"])
        .arg(&bundle)
        .args(["--bundle"])
        .arg(&bundle)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["noDifferences"], true, "{body}");
    assert_eq!(body["count"], 0, "{body}");
}

/// Builds an RFC 3339 timestamp `seconds` from now (UTC).
fn humantime_timestamp(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let secs_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// Howard Hinnant's `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
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
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}
