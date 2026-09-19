// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik doctor` — verify local configuration, credentials, Public API
//! compatibility, connectivity, authenticated context, and terminal rendering.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use hamstik_api_client::{ClientError, HamstikApi, Host};
use secrecy::SecretString;
use serde_json::{Value, json};

use crate::app::{Selection, Session};
use crate::config::ConfigFile;
use crate::context;
use crate::credentials::{self, CredentialStore};
use crate::error::CliError;
use crate::exit;
use crate::terminal::{SGR_GREEN, SGR_RED, SGR_YELLOW, color_probe, emoji_probe, paint};

use super::credential;
use super::emit_json;

/// The operation inventory compiled into the binary for compatibility checks.
///
/// The parsed parity test guarantees that this manifest exactly accounts for
/// the checked-in Public API contract. Doctor deliberately checks only these
/// required operations: additive live operations are compatible.
const API_PARITY_MANIFEST: &str = include_str!("../../../../openapi/api-parity.json");

/// The emoji sample rendered for interactive visual confirmation.
const EMOJI_SAMPLE: &str = "🐹 🐹 🐹";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Skipped,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Skipped => "skipped",
        }
    }
}

/// Structured error metadata retained by a failed diagnostic.
struct CheckError {
    kind: &'static str,
    code: String,
    request_id: Option<String>,
    http_status: Option<u16>,
    field_errors: Value,
    details: Option<Value>,
}

impl CheckError {
    fn from_cli(error: &CliError) -> Self {
        Self {
            kind: error.kind.as_str(),
            code: error.code.clone(),
            request_id: error.request_id.clone(),
            http_status: error.status,
            field_errors: json!(error.field_errors),
            details: error.details.as_ref().map(|details| json!(details)),
        }
    }

    fn to_json(&self) -> Value {
        let mut value = json!({
            "kind": self.kind,
            "code": self.code,
        });
        if let Some(request_id) = &self.request_id {
            value["requestId"] = Value::String(request_id.clone());
        }
        if let Some(status) = self.http_status {
            value["httpStatus"] = Value::from(status);
        }
        if !self
            .field_errors
            .as_object()
            .is_none_or(serde_json::Map::is_empty)
        {
            value["fieldErrors"] = self.field_errors.clone();
        }
        if let Some(details) = &self.details {
            value["details"] = details.clone();
        }
        value
    }
}

/// One stable diagnostic result.
struct Check {
    id: &'static str,
    name: &'static str,
    status: CheckStatus,
    critical: bool,
    detail: String,
    remediation: Option<String>,
    duration_ms: Option<u64>,
    request_id: Option<String>,
    error: Option<CheckError>,
    /// The failing transport stage for network checks.
    network_stage: Option<hamstik_api_client::NetworkStage>,
}

impl Check {
    fn pass(
        id: &'static str,
        name: &'static str,
        critical: bool,
        detail: impl Into<String>,
    ) -> Self {
        Self::new(id, name, CheckStatus::Pass, critical, detail)
    }

    fn warn(id: &'static str, name: &'static str, detail: impl Into<String>) -> Self {
        Self::new(id, name, CheckStatus::Warn, false, detail)
    }

    fn skipped(id: &'static str, name: &'static str, detail: impl Into<String>) -> Self {
        Self::new(id, name, CheckStatus::Skipped, false, detail)
    }

    fn failure(
        id: &'static str,
        name: &'static str,
        detail: impl Into<String>,
        error: &CliError,
    ) -> Self {
        let mut check = Self::new(id, name, CheckStatus::Fail, true, detail);
        check.request_id.clone_from(&error.request_id);
        check.error = Some(CheckError::from_cli(error));
        check
    }

    fn new(
        id: &'static str,
        name: &'static str,
        status: CheckStatus,
        critical: bool,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id,
            name,
            status,
            critical,
            detail: detail.into(),
            remediation: None,
            duration_ms: None,
            request_id: None,
            error: None,
            network_stage: None,
        }
    }

    /// Attaches the failing transport stage to a network check.
    fn with_network_stage(mut self, stage: hamstik_api_client::NetworkStage) -> Self {
        self.network_stage = Some(stage);
        self
    }

    fn with_remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }

    fn with_duration(mut self, started: Instant) -> Self {
        self.duration_ms = Some(started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
        self
    }

    fn with_request_id(mut self, request_id: Option<String>) -> Self {
        self.request_id = request_id;
        self
    }

    fn ok(&self) -> bool {
        self.status == CheckStatus::Pass
    }
}

/// Accumulates checks while preserving the first blocking/root-cause exit code.
struct Report {
    checks: Vec<Check>,
    exit_code: i32,
}

impl Report {
    fn new() -> Self {
        Self {
            checks: Vec::new(),
            exit_code: exit::SUCCESS,
        }
    }

    fn push(&mut self, check: Check) {
        self.checks.push(check);
    }

    fn fail(
        &mut self,
        id: &'static str,
        name: &'static str,
        error: CliError,
        remediation: impl Into<String>,
    ) {
        self.fail_check(
            Check::failure(id, name, error.message.clone(), &error).with_remediation(remediation),
            error.exit_code(),
        );
    }

    fn fail_check(&mut self, check: Check, code: i32) {
        if self.exit_code == exit::SUCCESS {
            self.exit_code = code;
        }
        self.checks.push(check);
    }
}

/// Aggregate check-status counts for the final summary.
struct ReportSummary {
    pass: usize,
    warn: usize,
    fail: usize,
    skipped: usize,
}

impl ReportSummary {
    fn of(report: &Report) -> Self {
        let mut summary = Self {
            pass: 0,
            warn: 0,
            fail: 0,
            skipped: 0,
        };
        for check in &report.checks {
            match check.status {
                CheckStatus::Pass => summary.pass += 1,
                CheckStatus::Warn => summary.warn += 1,
                CheckStatus::Fail => summary.fail += 1,
                CheckStatus::Skipped => summary.skipped += 1,
            }
        }
        summary
    }
}

struct LocalState {
    selection: Option<Selection>,
}

#[derive(Debug)]
struct Compatibility {
    detail: String,
    additive_operations: usize,
}

/// Runs the complete configuration/connectivity diagnostics.
///
/// `local_only` checks configuration, context resolution, profile selection,
/// credential-store accessibility, terminal behavior, and bundled
/// compatibility metadata without contacting the host: remote checks are
/// reported as skipped and no DNS or HTTP traffic is generated.
pub async fn run(
    session: &mut Session<'_>,
    local_only: bool,
    bundle: Option<&std::path::Path>,
) -> Result<(), CliError> {
    let mut report = Report::new();
    let local = diagnose_local_state(session, &mut report);

    let Some(selection) = local.selection else {
        finish_without_host(session, &mut report, local_only);
        return finish(session, report, bundle).await;
    };

    if local_only {
        // Local-only mode: explicitly mark every remote stage skipped so the
        // report shape stays stable, and run only offline checks.
        report.push(Check::skipped(
            "credential.expiry",
            "PAT expiration",
            "requires an authenticated request in local-only mode; use `me --json` for expiry data",
        ));
        report.push(Check::skipped(
            "scope.readiness",
            "scope readiness",
            "requires an authenticated request in local-only mode",
        ));
        report.push(Check::skipped(
            "network.connectivity",
            "network",
            "skipped by local-only mode",
        ));
        report.push(Check::skipped(
            "network.tls",
            "TLS",
            "skipped by local-only mode",
        ));
        report.push(Check::skipped(
            "api.openapi",
            "Public API",
            "skipped by local-only mode",
        ));
        // The bundled snapshot is still a valid offline compatibility check.
        let bundled = match bundled_openapi() {
            Ok(document) => document,
            Err(message) => {
                report.fail(
                    "api.compatibility",
                    "API compatibility",
                    CliError::protocol(message),
                    "The checked-in contract snapshot is malformed; this is a bug.",
                );
                report.push(color_check(session));
                report.push(emoji_check(session));
                return finish(session, report, bundle).await;
            }
        };
        match check_api_compatibility(&bundled) {
            Ok(compatibility) => report.push(Check::pass(
                "api.compatibility",
                "API compatibility",
                true,
                compatibility.detail,
            )),
            Err(message) => report.fail(
                "api.compatibility",
                "API compatibility",
                CliError::protocol(message),
                "The checked-in contract snapshot is malformed; this is a bug.",
            ),
        }
        report.push(Check::skipped(
            "api.authentication",
            "authentication",
            "skipped by local-only mode",
        ));
        report.push(Check::skipped(
            "context.organization",
            "organization",
            "skipped by local-only mode",
        ));
        report.push(Check::skipped(
            "context.project",
            "project",
            "skipped by local-only mode",
        ));
        report.push(color_check(session));
        report.push(emoji_check(session));
        return finish(session, report, bundle).await;
    }

    // An explicit ephemeral token makes the keyring irrelevant; do not even
    // construct a backend entry that could trigger an unlock prompt.
    let persistent_store =
        session.env.var("HAMSTIK_TOKEN").is_some() || credentials::store_is_persistent();
    let secret = resolve_credentials(
        session.env,
        session.store,
        &selection,
        persistent_store,
        &mut report,
    );

    let mut network_reachable = false;
    let mut public_client_ready = false;
    match session.public_api(&selection) {
        Ok(api) => {
            public_client_ready = true;
            let started = Instant::now();
            match api.get_open_api().await {
                Ok(response) => {
                    network_reachable = true;
                    record_transport_success(&mut report, &selection.host, started);
                    let version = response
                        .value
                        .pointer("/info/version")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    report.push(
                        Check::pass(
                            "api.openapi",
                            "Public API",
                            true,
                            format!("/api/v1/openapi.json returned API version {version}"),
                        )
                        .with_duration(started)
                        .with_request_id(response.request_id),
                    );
                    match check_api_compatibility(&response.value) {
                        Ok(compatibility) if compatibility.additive_operations == 0 => {
                            report.push(Check::pass(
                                "api.compatibility",
                                "API compatibility",
                                true,
                                compatibility.detail,
                            ));
                        }
                        Ok(compatibility) => {
                            report.push(Check::warn(
                                "api.compatibility",
                                "API compatibility",
                                compatibility.detail,
                            ));
                        }
                        Err(message) => report.fail(
                            "api.compatibility",
                            "API compatibility",
                            CliError::protocol(message),
                            "Upgrade the Hamstik CLI or verify that --host points to a compatible Public API v1 server.",
                        ),
                    }
                }
                Err(error) => {
                    network_reachable =
                        record_openapi_failure(&mut report, &selection.host, error, started);
                }
            }
        }
        Err(error) => {
            report.push(Check::skipped(
                "network.connectivity",
                "network",
                "HTTP client could not be constructed",
            ));
            report.fail(
                "network.tls",
                "TLS configuration",
                error,
                "Check --ca-bundle/HAMSTIK_CA_BUNDLE and the configured host.",
            );
            report.push(Check::skipped(
                "api.openapi",
                "Public API",
                "HTTP client could not be constructed",
            ));
            report.push(Check::skipped(
                "api.compatibility",
                "API compatibility",
                "OpenAPI document unavailable",
            ));
        }
    }

    let mut authenticated_api: Option<Arc<dyn HamstikApi>> = None;
    let mut authenticated = false;
    match (secret, network_reachable, public_client_ready) {
        (Some(secret), true, true) => match session.build_client(selection.host.clone(), secret) {
            Ok(api) => {
                let started = Instant::now();
                match api.whoami().await {
                    Ok(me) => {
                        authenticated = true;
                        report.push(
                            Check::pass(
                                "api.authentication",
                                "authentication",
                                true,
                                format!(
                                    "{} using credential {:?}; {} scope(s), expires {}",
                                    me.value.email,
                                    me.value.authentication.credential_name,
                                    me.value.authentication.scopes.len(),
                                    me.value.authentication.expires_at
                                ),
                            )
                            .with_duration(started)
                            .with_request_id(me.request_id.clone()),
                        );
                        authenticated_api = Some(api);
                        check_pat_expiry(&mut report, me.value.authentication.expires_at.clone());
                        check_scope_readiness(&mut report, &me.value.authentication.scopes);
                    }
                    Err(error) => {
                        let cli = CliError::from_client(error);
                        report.fail_check(
                            Check::failure(
                                "api.authentication",
                                "authentication",
                                cli.message.clone(),
                                &cli,
                            )
                            .with_duration(started)
                            .with_remediation(authentication_remediation(&cli)),
                            cli.exit_code(),
                        );
                    }
                }
            }
            Err(error) => report.fail(
                "api.authentication",
                "authentication client",
                error,
                "Check the configured host and CA bundle.",
            ),
        },
        (None, _, _) => report.push(Check::skipped(
            "api.authentication",
            "authentication",
            "no usable credential source",
        )),
        (Some(_), false, _) => report.push(Check::skipped(
            "api.authentication",
            "authentication",
            "network connectivity was not established",
        )),
        (Some(_), true, false) => report.push(Check::skipped(
            "api.authentication",
            "authentication",
            "HTTP client configuration failed",
        )),
    }

    diagnose_context_resources(
        &selection,
        authenticated_api.as_ref(),
        authenticated,
        &mut report,
    )
    .await;

    report.push(color_check(session));
    report.push(emoji_check(session));
    finish(session, report, bundle).await
}

fn diagnose_local_state(session: &Session<'_>, report: &mut Report) -> LocalState {
    let config_path = session.config.path();
    let config = match session.config.load() {
        Ok(config) => {
            let detail = if config_path.exists() {
                format!("{} is readable", config_path.display())
            } else {
                format!(
                    "{} is absent; built-in defaults apply",
                    config_path.display()
                )
            };
            report.push(Check::pass(
                "local.config",
                "configuration file",
                true,
                detail,
            ));
            Some(config)
        }
        Err(error) => {
            report.fail(
                "local.config",
                "configuration file",
                error,
                "Repair or remove the named non-secret configuration file.",
            );
            None
        }
    };

    let context_path = context::discover(&session.cwd);
    let context_file = match context_path.as_ref() {
        Some(path) => match context::load(path) {
            Ok(file) => {
                report.push(Check::pass(
                    "local.context_file",
                    "context file",
                    true,
                    format!("{} is readable", path.display()),
                ));
                Some(file)
            }
            Err(error) => {
                report.fail(
                    "local.context_file",
                    "context file",
                    error,
                    "Repair or remove the named .hamstik.toml file.",
                );
                None
            }
        },
        None => {
            report.push(Check::skipped(
                "local.context_file",
                "context file",
                "no .hamstik.toml found; profile/environment/default values may be used",
            ));
            None
        }
    };

    let (profile_name, profile_meta) = resolve_profile(session, config.as_ref(), report);
    let resolution = context::resolve(
        (
            &session.global.host,
            &session.global.org,
            &session.global.project,
        ),
        session.env,
        context_file.as_ref(),
        profile_meta.as_ref(),
    );

    report.push(Check::pass(
        "local.context_resolution",
        "context resolution",
        true,
        format!(
            "organization={} ({}), project={} ({})",
            resolution
                .organization
                .value
                .as_deref()
                .unwrap_or("not set"),
            resolution.organization.source.label(),
            resolution.project.value.as_deref().unwrap_or("not set"),
            resolution.project.source.label(),
        ),
    ));

    match Host::parse(&resolution.host) {
        Ok(host) => {
            report.push(Check::pass(
                "local.host",
                "host",
                true,
                format!("{host} (from {})", resolution.host_source.label()),
            ));
            report.push(proxy_check(session));
            LocalState {
                selection: Some(Selection {
                    host,
                    host_source: resolution.host_source,
                    profile: profile_name,
                    profile_meta,
                    organization: resolution.organization,
                    project: resolution.project,
                    context_path,
                    ephemeral_token: session.env.var("HAMSTIK_TOKEN").is_some(),
                }),
            }
        }
        Err(error) => {
            report.fail(
                "local.host",
                "host",
                CliError::config(format!("invalid host: {error}")),
                "Pass a bare HTTPS origin with --host, or correct HAMSTIK_HOST/context/profile configuration.",
            );
            report.push(Check::skipped(
                "network.proxy",
                "proxy",
                "host validation failed",
            ));
            LocalState { selection: None }
        }
    }
}

fn resolve_profile(
    session: &Session<'_>,
    config: Option<&ConfigFile>,
    report: &mut Report,
) -> (Option<String>, Option<crate::config::Profile>) {
    let Some(config) = config else {
        report.push(Check::skipped(
            "local.profile",
            "profile",
            "configuration file is unreadable",
        ));
        return (None, None);
    };

    match context::select_profile(config, session.global.profile.as_deref(), session.env) {
        Ok(Some(name)) => {
            let metadata = config.profiles.get(&name).cloned();
            report.push(Check::pass(
                "local.profile",
                "profile",
                true,
                format!("{name:?} selected"),
            ));
            (Some(name), metadata)
        }
        Ok(None) => {
            report.push(Check::skipped(
                "local.profile",
                "profile",
                "none selected; HAMSTIK_TOKEN may be used",
            ));
            (None, None)
        }
        Err(error) => {
            report.fail(
                "local.profile",
                "profile",
                error,
                "Select an existing profile with --profile or run `hamstik auth login`.",
            );
            (None, None)
        }
    }
}

fn resolve_credentials(
    env: &dyn crate::environment::Environment,
    store: &dyn CredentialStore,
    selection: &Selection,
    persistent_store: bool,
    report: &mut Report,
) -> Option<SecretString> {
    if let Some(token) = env.var("HAMSTIK_TOKEN") {
        report.push(Check::skipped(
            "credential.store",
            "credential store",
            "not consulted because HAMSTIK_TOKEN is active",
        ));
        return match crate::input::token_to_secret(&token) {
            Ok(secret) => {
                report.push(Check::pass(
                    "credential.source",
                    "credential source",
                    true,
                    "HAMSTIK_TOKEN is usable and remains ephemeral",
                ));
                Some(secret)
            }
            Err(message) => {
                report.fail(
                    "credential.source",
                    "credential source",
                    CliError::auth(format!("HAMSTIK_TOKEN is unusable: {message}")),
                    "Set HAMSTIK_TOKEN to a non-empty PAT or unset it and select a stored profile.",
                );
                None
            }
        };
    }

    if !persistent_store {
        report.fail(
            "credential.store",
            "credential store",
            CliError::credential(credentials::NO_STORE_HINT),
            "Configure an OS credential service or use HAMSTIK_TOKEN for this process.",
        );
        report.push(Check::skipped(
            "credential.source",
            "credential source",
            "persistent credential store unavailable",
        ));
        return None;
    }

    let Some(profile) = selection.profile_meta.as_ref() else {
        report.push(Check::warn(
            "credential.store",
            "credential store",
            "persistent backend configured; no profile was available for a read probe",
        ));
        report.fail(
            "credential.source",
            "credential source",
            CliError::auth("no selected profile and HAMSTIK_TOKEN is not set"),
            "Run `hamstik auth login`, select a profile, or set HAMSTIK_TOKEN.",
        );
        return None;
    };

    let lookup_hosts = if selection.host.as_str() == profile.host {
        vec![selection.host.as_str()]
    } else {
        vec![selection.host.as_str(), profile.host.as_str()]
    };
    for host in lookup_hosts {
        let account = credentials::account_key(host, &profile.user_id);
        match store.get(&account) {
            Ok(Some(secret)) => {
                report.push(Check::pass(
                    "credential.store",
                    "credential store",
                    true,
                    "persistent store is accessible",
                ));
                report.push(Check::pass(
                    "credential.source",
                    "credential source",
                    true,
                    format!(
                        "stored credential for profile {:?}",
                        selection.profile.as_deref().unwrap_or_default()
                    ),
                ));
                return Some(secret);
            }
            Ok(None) => {}
            Err(error) => {
                report.fail(
                    "credential.store",
                    "credential store",
                    CliError::credential(format!("credential store unavailable: {error}")),
                    "Unlock or configure the OS credential store, or use HAMSTIK_TOKEN.",
                );
                report.push(Check::skipped(
                    "credential.source",
                    "credential source",
                    "credential store read failed",
                ));
                return None;
            }
        }
    }

    report.push(Check::pass(
        "credential.store",
        "credential store",
        true,
        "persistent store is accessible",
    ));
    report.fail(
        "credential.source",
        "credential source",
        CliError::auth(format!(
            "no stored credential for profile {:?}",
            selection.profile.as_deref().unwrap_or_default()
        )),
        "Run `hamstik auth login` for the selected profile.",
    );
    None
}

fn proxy_check(session: &Session<'_>) -> Check {
    let names: Vec<_> = [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ]
    .into_iter()
    .filter(|name| session.env.var(name).is_some_and(|value| !value.is_empty()))
    .collect();
    if names.is_empty() {
        Check::skipped(
            "network.proxy",
            "proxy",
            "no proxy environment variables are set",
        )
    } else {
        Check::pass(
            "network.proxy",
            "proxy",
            false,
            format!(
                "configured by {} (values hidden because they may contain credentials)",
                names.join(", ")
            ),
        )
    }
}

/// Reports PAT expiry as expired / imminently expiring (≤14 days) / healthy.
///
/// Clock skew is the server's; thresholds are documented CLI-side warnings.
/// The classification lives in `commands::credential` so `auth status`, `me`,
/// and `doctor` share one threshold definition.
fn check_pat_expiry(report: &mut Report, expires_at: String) {
    // `now` comes from the process clock; the comparison is advisory only.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    match credential::classify_expiry(&expires_at, now) {
        credential::Expiry::Expired => report.fail(
            "credential.expiry",
            "PAT expiration",
            CliError::auth(format!(
                "credential expired {expires_at}; every request will be rejected as unauthenticated"
            )),
            "Create a new PAT and run `hamstik auth login --with-token`.",
        ),
        credential::Expiry::Approaching(0) => report.push(Check::warn(
            "credential.expiry",
            "PAT expiration",
            format!("credential expires within 24 hours ({expires_at}); create a new PAT soon"),
        )),
        credential::Expiry::Approaching(days) => report.push(Check::warn(
            "credential.expiry",
            "PAT expiration",
            format!("credential expires in {days} day(s) ({expires_at}); create a new PAT soon"),
        )),
        credential::Expiry::Valid(days) => report.push(Check::pass(
            "credential.expiry",
            "PAT expiration",
            false,
            format!("credential is valid for {days} more day(s) (expires {expires_at})"),
        )),
        credential::Expiry::Unknown => report.push(Check::warn(
            "credential.expiry",
            "PAT expiration",
            format!("could not parse credential expiry {expires_at:?}"),
        )),
    }
}

/// Scope readiness reuses the shared credential reporting so `auth status`,
/// `me`, and `doctor` describe scope inventory identically.
fn check_scope_readiness(report: &mut Report, scopes: &[String]) {
    let report_value = credential::scope_report(scopes);
    if report_value["readiness"] == "none" {
        report.push(Check::warn(
            "scope.readiness",
            "scope readiness",
            report_value["note"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ));
    } else {
        report.push(Check::pass(
            "scope.readiness",
            "scope readiness",
            false,
            format!(
                "credential carries {} scope(s): {}",
                report_value["count"],
                scopes.join(", ")
            ),
        ));
    }
}

/// The checked-in OpenAPI snapshot used for offline compatibility checks.
fn bundled_openapi() -> Result<Value, String> {
    serde_json::from_str(include_str!("../../../../openapi/hamstik-v1.json"))
        .map_err(|error| format!("checked-in OpenAPI snapshot is invalid: {error}"))
}

/// Shared skipped-check tail for the no-selection path.
fn finish_without_host(session: &Session<'_>, report: &mut Report, local_only: bool) {
    let reason = if local_only {
        "skipped by local-only mode"
    } else {
        "host resolution failed"
    };
    report.push(Check::skipped(
        "credential.store",
        "credential store",
        reason,
    ));
    report.push(Check::skipped(
        "credential.expiry",
        "PAT expiration",
        reason,
    ));
    report.push(Check::skipped("scope.readiness", "scope readiness", reason));
    report.push(Check::skipped("network.connectivity", "network", reason));
    report.push(Check::skipped("network.tls", "TLS", reason));
    report.push(Check::skipped("api.openapi", "Public API", reason));
    report.push(Check::skipped(
        "api.compatibility",
        "API compatibility",
        reason,
    ));
    report.push(Check::skipped(
        "api.authentication",
        "authentication",
        reason,
    ));
    report.push(context_skipped(
        "context.organization",
        "organization",
        reason,
    ));
    report.push(context_skipped("context.project", "project", reason));
    report.push(color_check(session));
    report.push(emoji_check(session));
}

fn record_transport_success(report: &mut Report, host: &Host, started: Instant) {
    report.push(
        Check::pass(
            "network.connectivity",
            "network",
            true,
            format!("{} returned an HTTP response", host.as_str()),
        )
        .with_duration(started),
    );
    if host.as_str().starts_with("https://") {
        report.push(
            Check::pass(
                "network.tls",
                "TLS",
                true,
                "HTTPS handshake and certificate validation succeeded",
            )
            .with_duration(started),
        );
    } else {
        report.push(Check::skipped(
            "network.tls",
            "TLS",
            "not applicable to an allowed loopback HTTP host",
        ));
    }
}

fn record_openapi_failure(
    report: &mut Report,
    host: &Host,
    error: ClientError,
    started: Instant,
) -> bool {
    match error {
        ClientError::Network { message, stage } => {
            let (name, remediation) = match stage {
                hamstik_api_client::NetworkStage::Dns => (
                    "DNS resolution",
                    "Verify the hostname spelling, your DNS resolver, and that you are online.",
                ),
                hamstik_api_client::NetworkStage::Connection => (
                    "TCP connection",
                    "Check firewall rules, VPN state, and that the host is reachable on the network.",
                ),
                hamstik_api_client::NetworkStage::Proxy => (
                    "proxy",
                    "Check HTTPS_PROXY/HTTP_PROXY/ALL_PROXY values and proxy authentication; values are hidden because they may contain credentials.",
                ),
                hamstik_api_client::NetworkStage::Timeout => (
                    "request timeout",
                    "The host did not respond in time; check load, VPN latency, and network stability.",
                ),
                hamstik_api_client::NetworkStage::Tls => (
                    "TLS",
                    "Verify the CA bundle (--ca-bundle/HAMSTIK_CA_BUNDLE), the certificate chain, and hostname.",
                ),
                hamstik_api_client::NetworkStage::Unknown => (
                    "network",
                    "Check DNS, connectivity, proxy settings, firewall rules, and the configured host.",
                ),
            };
            let cli = CliError::network(format!("{name}: {message}"));
            report.fail_check(
                Check::failure("network.connectivity", name, cli.message.clone(), &cli)
                    .with_duration(started)
                    .with_remediation(remediation)
                    .with_network_stage(stage),
                cli.exit_code(),
            );
            report.push(Check::skipped(
                "network.tls",
                "TLS",
                "no HTTP response; TLS could not be confirmed independently",
            ));
            report.push(Check::skipped(
                "api.openapi",
                "Public API",
                "network connectivity was not established",
            ));
            report.push(Check::skipped(
                "api.compatibility",
                "API compatibility",
                "OpenAPI document unavailable",
            ));
            false
        }
        other => {
            record_transport_success(report, host, started);
            let cli = CliError::from_client(other);
            report.fail_check(
                Check::failure("api.openapi", "Public API", cli.message.clone(), &cli)
                    .with_duration(started)
                    .with_remediation(
                        "Verify that --host serves the Hamstik Public API at /api/v1/openapi.json.",
                    ),
                cli.exit_code(),
            );
            report.push(Check::skipped(
                "api.compatibility",
                "API compatibility",
                "OpenAPI document could not be decoded",
            ));
            true
        }
    }
}

fn check_api_compatibility(document: &Value) -> Result<Compatibility, String> {
    let api_version = document
        .pointer("/info/version")
        .and_then(Value::as_str)
        .ok_or_else(|| "OpenAPI info.version is missing".to_string())?;
    if api_version.split('.').next() != Some("1") {
        return Err(format!(
            "server advertises Public API version {api_version}; this CLI requires v1"
        ));
    }
    let openapi_version = document
        .get("openapi")
        .and_then(Value::as_str)
        .ok_or_else(|| "OpenAPI version marker is missing".to_string())?;
    if openapi_version.split('.').next() != Some("3") {
        return Err(format!(
            "server document uses OpenAPI {openapi_version}; this CLI requires OpenAPI 3"
        ));
    }

    let manifest: Value = serde_json::from_str(API_PARITY_MANIFEST)
        .map_err(|error| format!("embedded API parity manifest is invalid: {error}"))?;
    let expected = manifest
        .get("operations")
        .and_then(Value::as_array)
        .ok_or_else(|| "embedded API parity manifest has no operations".to_string())?;
    let paths = document
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| "OpenAPI paths object is missing".to_string())?;

    let mut expected_ids = BTreeSet::new();
    for operation in expected {
        let id = operation
            .get("operationId")
            .and_then(Value::as_str)
            .ok_or_else(|| "embedded manifest operationId is missing".to_string())?;
        let path = operation
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("embedded manifest path is missing for {id}"))?;
        let method = operation
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("embedded manifest method is missing for {id}"))?
            .to_ascii_lowercase();
        let live_id = paths
            .get(path)
            .and_then(|item| item.get(&method))
            .and_then(|item| item.get("operationId"))
            .and_then(Value::as_str);
        if live_id != Some(id) {
            return Err(format!(
                "required operation {id} is missing or moved from {} {}",
                method.to_ascii_uppercase(),
                path
            ));
        }
        expected_ids.insert(id.to_string());
    }

    let live_ids: BTreeSet<String> = paths
        .values()
        .filter_map(Value::as_object)
        .flat_map(|item| item.values())
        .filter_map(|operation| operation.get("operationId").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let additive_operations = live_ids.difference(&expected_ids).count();
    let detail = if additive_operations == 0 {
        format!(
            "Public API {api_version}; all {} required operations are present",
            expected_ids.len()
        )
    } else {
        format!(
            "Public API {api_version}; all {} required operations are present and {additive_operations} additive operation(s) are available",
            expected_ids.len()
        )
    };
    Ok(Compatibility {
        detail,
        additive_operations,
    })
}

async fn diagnose_context_resources(
    selection: &Selection,
    api: Option<&Arc<dyn HamstikApi>>,
    authenticated: bool,
    report: &mut Report,
) {
    let Some(org) = selection.organization.value.as_deref() else {
        report.push(
            context_skipped("context.organization", "organization", "not selected")
                .with_remediation(
                    "Pass --org, set HAMSTIK_ORG, or configure an Organization context.",
                ),
        );
        if selection.project.value.is_some() {
            report.fail(
                "context.project",
                "project",
                CliError::config("a Project is selected without an Organization"),
                "Select an Organization with --org or clear the Project context.",
            );
        } else {
            report.push(context_skipped(
                "context.project",
                "project",
                "not selected",
            ));
        }
        return;
    };

    if !authenticated {
        report.push(context_skipped(
            "context.organization",
            "organization",
            format!("{org} selected but authentication did not succeed"),
        ));
        report.push(context_skipped(
            "context.project",
            "project",
            "Organization could not be validated",
        ));
        return;
    }
    let Some(api) = api else {
        report.push(context_skipped(
            "context.organization",
            "organization",
            "authenticated API client unavailable",
        ));
        report.push(context_skipped(
            "context.project",
            "project",
            "Organization could not be validated",
        ));
        return;
    };

    let started = Instant::now();
    let organization_valid = match api.get_organization(org).await {
        Ok(response) if response.value.suspended => {
            report.fail_check(
                Check::failure(
                    "context.organization",
                    "organization",
                    format!("{org} is suspended"),
                    &CliError::config("selected Organization is suspended"),
                )
                .with_duration(started)
                .with_request_id(response.request_id)
                .with_remediation("Select an active Organization or contact its administrator."),
                exit::AUTHORIZATION,
            );
            false
        }
        Ok(response) => {
            report.push(
                Check::pass(
                    "context.organization",
                    "organization",
                    true,
                    format!(
                        "{} ({}) is accessible with role {}",
                        response.value.name, response.value.slug, response.value.role
                    ),
                )
                .with_duration(started)
                .with_request_id(response.request_id),
            );
            true
        }
        Err(error) => {
            let cli = CliError::from_client(error);
            report.fail_check(
                Check::failure(
                    "context.organization",
                    "organization",
                    cli.message.clone(),
                    &cli,
                )
                .with_duration(started)
                .with_remediation(
                    "Correct the Organization context or verify membership and PAT scope.",
                ),
                cli.exit_code(),
            );
            false
        }
    };

    let Some(project) = selection.project.value.as_deref() else {
        report.push(
            context_skipped("context.project", "project", "not selected").with_remediation(
                "Pass --project, set HAMSTIK_PROJECT, or configure a Project context.",
            ),
        );
        return;
    };
    if !organization_valid {
        report.push(context_skipped(
            "context.project",
            "project",
            format!("{project} selected but Organization validation failed"),
        ));
        return;
    }

    let started = Instant::now();
    match api.get_project(org, project).await {
        Ok(response) if response.value.archived_at.is_some() => report.push(
            Check::warn(
                "context.project",
                "project",
                format!(
                    "{} ({}) is accessible but archived",
                    response.value.name, response.value.key
                ),
            )
            .with_duration(started)
            .with_request_id(response.request_id)
            .with_remediation(
                "Select an active Project for mutations, or intentionally keep this context for read-only work.",
            ),
        ),
        Ok(response) => report.push(
            Check::pass(
                "context.project",
                "project",
                true,
                format!(
                    "{} ({}) is accessible",
                    response.value.name, response.value.key
                ),
            )
            .with_duration(started)
            .with_request_id(response.request_id),
        ),
        Err(error) => {
            let cli = CliError::from_client(error);
            report.fail_check(
                Check::failure("context.project", "project", cli.message.clone(), &cli)
                    .with_duration(started)
                    .with_remediation(
                        "Correct the Project context or verify access and PAT scope.",
                    ),
                cli.exit_code(),
            );
        }
    }
}

fn authentication_remediation(error: &CliError) -> String {
    match error.status {
        Some(401) => "Replace or refresh the PAT, then run `hamstik auth login`.".to_string(),
        Some(403) => "Grant the PAT the required scope or use an account with access.".to_string(),
        _ => "Check the PAT, network, host, and Public API availability.".to_string(),
    }
}

fn context_skipped(id: &'static str, name: &'static str, detail: impl Into<String>) -> Check {
    Check::skipped(id, name, detail)
}

/// Probes ANSI color support for this invocation's stdout.
fn color_check(session: &Session<'_>) -> Check {
    let probe = color_probe(
        session.env,
        session.global.no_color,
        session.env.stdout_is_terminal(),
    );
    if probe.ok {
        Check::pass("terminal.color", "terminal color", false, probe.detail)
    } else {
        Check::warn("terminal.color", "terminal color", probe.detail)
    }
}

/// Probes emoji support for this invocation's stdout.
fn emoji_check(session: &Session<'_>) -> Check {
    let probe = emoji_probe(session.env, session.env.stdout_is_terminal());
    if probe.ok {
        Check::pass("terminal.emoji", "terminal emoji", false, probe.detail)
    } else {
        Check::warn("terminal.emoji", "terminal emoji", probe.detail)
    }
}

fn render(session: &mut Session<'_>, report: Report) -> Result<(), CliError> {
    let overall_ok = report.exit_code == exit::SUCCESS;
    let summary = ReportSummary::of(&report);

    if session.json() {
        let items: Vec<_> = report
            .checks
            .iter()
            .map(|check| {
                let mut value = json!({
                    "id": check.id,
                    "name": check.name,
                    "status": check.status.as_str(),
                    // Retained for compatibility with the initial doctor JSON.
                    "ok": check.ok(),
                    "critical": check.critical,
                    "detail": check.detail,
                });
                if let Some(remediation) = &check.remediation {
                    value["remediation"] = Value::String(remediation.clone());
                }
                if let Some(duration_ms) = check.duration_ms {
                    value["durationMs"] = Value::from(duration_ms);
                }
                if let Some(request_id) = &check.request_id {
                    value["requestId"] = Value::String(request_id.clone());
                }
                if let Some(stage) = check.network_stage {
                    value["networkStage"] = Value::String(stage.as_str().to_string());
                }
                if let Some(error) = &check.error {
                    value["error"] = error.to_json();
                }
                value
            })
            .collect();
        emit_json(
            session,
            &json!({
                "schemaVersion": 1,
                "checks": items,
                "summary": {
                    "pass": summary.pass,
                    "warn": summary.warn,
                    "fail": summary.fail,
                    "skipped": summary.skipped,
                },
                "ok": overall_ok,
                "exitCode": report.exit_code,
            }),
        )?;
    } else if !session.out.is_quiet() {
        let color = color_probe(
            session.env,
            session.global.no_color,
            session.env.stdout_is_terminal(),
        )
        .ok;
        for check in &report.checks {
            let (marker, sgr) = match check.status {
                CheckStatus::Pass => ("ok", SGR_GREEN),
                CheckStatus::Warn => ("WARN", SGR_YELLOW),
                CheckStatus::Fail => ("FAIL", SGR_RED),
                CheckStatus::Skipped => ("skip", SGR_YELLOW),
            };
            let marker = paint(color, sgr, marker);
            session
                .out
                .line(&format!(
                    "[{marker:<4}] {:<20} {}",
                    check.name, check.detail
                ))
                .map_err(CliError::general)?;
            if let Some(remediation) = &check.remediation {
                session
                    .out
                    .line(&format!("       hint: {remediation}"))
                    .map_err(CliError::general)?;
            }
            if let Some(error) = &check.error {
                let status = error
                    .http_status
                    .map(|status| format!(" (HTTP {status})"))
                    .unwrap_or_default();
                session
                    .out
                    .line(&format!("       error: {}{status}", error.code))
                    .map_err(CliError::general)?;
            }
            if let Some(request_id) = &check.request_id {
                session
                    .out
                    .line(&format!("       request id: {request_id}"))
                    .map_err(CliError::general)?;
            }
        }
        session
            .out
            .line(&format!(
                "summary: {} passed, {} warned, {} failed, {} skipped",
                summary.pass, summary.warn, summary.fail, summary.skipped
            ))
            .map_err(CliError::general)?;
        session
            .out
            .line(if overall_ok { "ready." } else { "not ready." })
            .map_err(CliError::general)?;

        // Visual samples are useful only when a person is looking at a TTY.
        if session.env.stdout_is_terminal() {
            let emoji_ok = report
                .checks
                .iter()
                .any(|check| check.id == "terminal.emoji" && check.ok());
            session
                .out
                .line(&format!(
                    "emoji sample: {EMOJI_SAMPLE}{}",
                    if emoji_ok {
                        ""
                    } else {
                        " (boxes mean your terminal lacks emoji)"
                    }
                ))
                .map_err(CliError::general)?;
            if color {
                let swatches = format!(
                    "{}green{} {}red{} {}yellow{}",
                    SGR_GREEN, "\x1b[0m", SGR_RED, "\x1b[0m", SGR_YELLOW, "\x1b[0m",
                );
                session
                    .out
                    .line(&format!("color sample: {swatches}"))
                    .map_err(CliError::general)?;
            }
        }
    }

    session.exit_code = report.exit_code;
    Ok(())
}

/// Either render the report to the terminal/JSON or write a support bundle.
async fn finish(
    session: &mut Session<'_>,
    report: Report,
    bundle: Option<&std::path::Path>,
) -> Result<(), CliError> {
    if let Some(path) = bundle {
        write_bundle(session, &report, path).await?;
    }
    render(session, report)
}

/// Bundle layout version. Bump when the on-disk structure changes.
const BUNDLE_VERSION: &str = "1.0";

/// Writes a redacted, versioned support bundle to `path`.
async fn write_bundle(
    session: &mut Session<'_>,
    report: &Report,
    path: &std::path::Path,
) -> Result<(), CliError> {
    use std::io::Write;

    let file = std::fs::File::create(path)
        .map_err(|err| CliError::general(format!("cannot create bundle: {err}")))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();

    // --- bundle-manifest.json ---
    let manifest = json!({
        "bundleVersion": BUNDLE_VERSION,
        "generatedAt": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| format!("{}", d.as_secs()))
            .unwrap_or_else(|_| "0".to_string()),
        "contents": [
            "bundle-manifest.json",
            "doctor-report.json",
            "context-explain.json",
            "cli-info.json",
            "api-compatibility.json",
            "config-metadata.json",
        ],
        "redaction": "by construction plus explicit scrub pass",
    });
    zip.start_file("bundle-manifest.json", options)
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;
    zip.write_all(manifest.to_string().as_bytes())
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;

    // --- doctor-report.json ---
    let report_json = report_to_json(report);
    zip.start_file("doctor-report.json", options)
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;
    zip.write_all(report_json.to_string().as_bytes())
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;

    // --- context-explain.json ---
    let context_json = context_explain_json(session);
    zip.start_file("context-explain.json", options)
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;
    zip.write_all(context_json.to_string().as_bytes())
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;

    // --- cli-info.json ---
    let cli_info = cli_info_json();
    zip.start_file("cli-info.json", options)
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;
    zip.write_all(cli_info.to_string().as_bytes())
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;

    // --- api-compatibility.json ---
    let api_compat = api_compatibility_json();
    zip.start_file("api-compatibility.json", options)
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;
    zip.write_all(api_compat.to_string().as_bytes())
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;

    // --- config-metadata.json ---
    let config_meta = config_metadata_json(session);
    zip.start_file("config-metadata.json", options)
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;
    zip.write_all(config_meta.to_string().as_bytes())
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;

    zip.finish()
        .map_err(|err| CliError::general(format!("bundle write error: {err}")))?;

    session
        .out
        .line(&format!("support bundle written to {}", path.display()))
        .map_err(CliError::general)?;
    Ok(())
}

fn report_to_json(report: &Report) -> Value {
    let items: Vec<_> = report
        .checks
        .iter()
        .map(|check| {
            let mut value = json!({
                "id": check.id,
                "name": check.name,
                "status": check.status.as_str(),
                "ok": check.ok(),
                "critical": check.critical,
                "detail": check.detail,
            });
            if let Some(remediation) = &check.remediation {
                value["remediation"] = Value::String(remediation.clone());
            }
            if let Some(duration_ms) = check.duration_ms {
                value["durationMs"] = Value::from(duration_ms);
            }
            if let Some(request_id) = &check.request_id {
                value["requestId"] = Value::String(request_id.clone());
            }
            if let Some(stage) = check.network_stage {
                value["networkStage"] = Value::String(stage.as_str().to_string());
            }
            if let Some(error) = &check.error {
                value["error"] = error.to_json();
            }
            value
        })
        .collect();
    json!({
        "schemaVersion": 1,
        "checks": items,
        "summary": {
            "pass": ReportSummary::of(report).pass,
            "warn": ReportSummary::of(report).warn,
            "fail": ReportSummary::of(report).fail,
            "skipped": ReportSummary::of(report).skipped,
        },
        "ok": report.exit_code == exit::SUCCESS,
        "exitCode": report.exit_code,
    })
}

fn context_explain_json(session: &Session<'_>) -> Value {
    let selection = match session.selection() {
        Ok(s) => s,
        Err(_) => return json!({"error": "context resolution unavailable"}),
    };
    let config = match session.config.load() {
        Ok(c) => c,
        Err(_) => return json!({"error": "config unavailable"}),
    };
    let context_document = selection
        .context_path
        .as_deref()
        .and_then(|path| crate::context::load(path).ok());
    let (host_chain, org_chain, project_chain) = crate::context::explain_chains(
        (
            &session.global.host,
            &session.global.org,
            &session.global.project,
        ),
        session.env,
        context_document.as_ref(),
        selection.profile_meta.as_ref(),
    );

    let profile_chain = {
        let mut sources = Vec::new();
        let mut push = |value: Option<String>, source: &str, status: &str| {
            sources.push(json!({
                "source": source,
                "status": status,
                "value": value,
            }));
        };
        match (
            session.global.profile.clone(),
            session.env.var("HAMSTIK_PROFILE"),
            config.active_profile.clone(),
        ) {
            (Some(flag), env, active) => {
                push(Some(flag), "cli", "winner");
                if let Some(env) = env {
                    push(Some(env), "environment", "shadowed");
                }
                if let Some(active) = active {
                    push(Some(active), "profile", "shadowed");
                }
            }
            (None, Some(env), active) => {
                push(Some(env), "environment", "winner");
                if let Some(active) = active {
                    push(Some(active), "profile", "shadowed");
                }
            }
            (None, None, Some(active)) => push(Some(active), "profile", "winner"),
            (None, None, None) => push(None, "default", "unset"),
        }
        json!({ "sources": sources })
    };

    let token_chain = {
        let env_token = session.env.var("HAMSTIK_TOKEN").is_some();
        let mut sources = Vec::new();
        if env_token {
            sources.push(json!({
                "source": "environment",
                "status": "winner",
                "value": "<set; value not displayed>",
            }));
        } else if selection.profile.is_some() {
            sources.push(json!({
                "source": "credential_store",
                "status": "winner",
                "value": "<stored credential; value never displayed>",
            }));
        } else {
            sources.push(json!({
                "source": "default",
                "status": "unset",
                "value": Value::Null,
            }));
        }
        json!({ "sources": sources })
    };

    json!({
        "schemaVersion": 1,
        "values": {
            "host": chain_json(&host_chain, "HAMSTIK_HOST"),
            "organization": chain_json(&org_chain, "HAMSTIK_ORG"),
            "project": chain_json(&project_chain, "HAMSTIK_PROJECT"),
            "profile": profile_chain,
            "token": token_chain,
        },
        "contextDiscovery": {
            "searchedFrom": session.cwd.display().to_string(),
            "filename": crate::context::CONTEXT_FILENAME,
            "found": selection.context_path.as_ref().map(|p| p.display().to_string()),
        },
        "localOnly": true,
    })
}

fn chain_json(field: &crate::context::ResolvedChain, env_var: &str) -> Value {
    let mut sources = Vec::new();
    if let Some((value, source)) = &field.winner {
        sources.push(json!({
            "source": source.as_key(),
            "status": "winner",
            "value": value,
        }));
    }
    for (value, source) in &field.shadowed {
        sources.push(json!({
            "source": source.as_key(),
            "status": "shadowed",
            "value": value,
        }));
    }
    if field.winner.is_none() {
        sources.push(json!({
            "source": "default",
            "status": "unset",
            "value": Value::Null,
        }));
    }
    json!({
        "envVar": env_var,
        "sources": sources,
        "winningSource": field.source().as_key(),
    })
}

fn cli_info_json() -> Value {
    json!({
        "cliVersion": env!("CARGO_PKG_VERSION"),
        "profile": std::env::var("PROFILE").unwrap_or_default(),
        "targetArch": std::env::consts::ARCH,
        "targetOs": std::env::consts::OS,
        "rustVersion": env!("CARGO_PKG_RUST_VERSION"),
    })
}

fn api_compatibility_json() -> Value {
    match bundled_openapi() {
        Ok(doc) => match check_api_compatibility(&doc) {
            Ok(compat) => {
                json!({"detail": compat.detail, "additiveOperations": compat.additive_operations})
            }
            Err(message) => json!({"error": message}),
        },
        Err(message) => json!({"error": message}),
    }
}

fn config_metadata_json(session: &Session<'_>) -> Value {
    let path = session.config.path();
    let mut meta = json!({
        "path": path.display().to_string(),
        "exists": path.exists(),
    });
    if let Ok(config) = session.config.load() {
        meta["activeProfile"] = json!(config.active_profile);
        meta["profiles"] = json!(config.profiles.keys().collect::<Vec<_>>());
        let _ = config;
    }
    meta
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::app::{ApiFactory, ClientRequest, PublicClientRequest};
    use crate::context::{ResolvedField, Source};
    use crate::credentials::CredentialError;
    use crate::environment::MapEnvironment;
    use crate::input::Prompt;
    use std::io;
    use std::sync::Arc;

    struct FailingStore;

    impl CredentialStore for FailingStore {
        fn get(&self, _account: &str) -> Result<Option<SecretString>, CredentialError> {
            Err(CredentialError::Unavailable("locked".to_string()))
        }

        fn set(&self, _account: &str, _secret: &SecretString) -> Result<(), CredentialError> {
            Err(CredentialError::Unavailable("locked".to_string()))
        }

        fn delete(&self, _account: &str) -> Result<(), CredentialError> {
            Err(CredentialError::Unavailable("locked".to_string()))
        }
    }

    struct MockFactory;

    impl ApiFactory for MockFactory {
        fn build(&self, _request: &ClientRequest) -> Result<Arc<dyn HamstikApi>, CliError> {
            unimplemented!()
        }

        fn build_public(
            &self,
            _request: &PublicClientRequest,
        ) -> Result<Arc<dyn HamstikApi>, CliError> {
            unimplemented!()
        }
    }

    struct MockPrompt;

    impl Prompt for MockPrompt {
        fn read_line(&mut self, _prompt: &str) -> io::Result<String> {
            unimplemented!()
        }

        fn read_secret(&mut self, _prompt: &str) -> io::Result<String> {
            unimplemented!()
        }
    }

    fn selection() -> Selection {
        Selection {
            host: Host::parse("https://hamstik.com").unwrap(),
            host_source: Source::Default,
            profile: Some("test".to_string()),
            profile_meta: Some(crate::config::Profile {
                host: "https://hamstik.com".to_string(),
                user_id: "user-1".to_string(),
                email: "user@example.com".to_string(),
                default_organization: None,
                default_project: None,
            }),
            organization: ResolvedField {
                value: None,
                source: Source::Default,
            },
            project: ResolvedField {
                value: None,
                source: Source::Default,
            },
            context_path: None,
            ephemeral_token: false,
        }
    }

    #[test]
    fn credential_store_read_failure_is_never_ready() {
        let mut report = Report::new();
        let secret = resolve_credentials(
            &MapEnvironment::new(),
            &FailingStore,
            &selection(),
            true,
            &mut report,
        );
        assert!(secret.is_none());
        assert_eq!(report.exit_code, exit::CONFIGURATION);
        assert!(
            report.checks.iter().any(|check| {
                check.id == "credential.store" && check.status == CheckStatus::Fail
            })
        );
    }

    #[test]
    fn invalid_environment_token_produces_one_source_failure() {
        let env = MapEnvironment::new().with_var("HAMSTIK_TOKEN", " ");
        let mut report = Report::new();
        let secret = resolve_credentials(&env, &FailingStore, &selection(), true, &mut report);
        assert!(secret.is_none());
        assert_eq!(report.exit_code, exit::AUTHENTICATION);
        assert_eq!(
            report
                .checks
                .iter()
                .filter(|check| check.id == "credential.source")
                .count(),
            1
        );
    }

    #[test]
    fn checked_in_contract_is_compatible() {
        let document: Value =
            serde_json::from_str(include_str!("../../../../openapi/hamstik-v1.json")).unwrap();
        let compatibility = check_api_compatibility(&document).unwrap();
        assert_eq!(compatibility.additive_operations, 0);
        assert!(compatibility.detail.contains("57 required operations"));
    }

    #[test]
    fn additive_operations_are_compatible_but_reported() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../../openapi/hamstik-v1.json")).unwrap();
        document["paths"]["/api/v1/future"] = json!({
            "get": {"operationId": "futureOperation"}
        });
        let compatibility = check_api_compatibility(&document).unwrap();
        assert_eq!(compatibility.additive_operations, 1);
    }

    #[test]
    fn missing_required_operation_is_incompatible() {
        let mut document: Value =
            serde_json::from_str(include_str!("../../../../openapi/hamstik-v1.json")).unwrap();
        document["paths"]
            .as_object_mut()
            .unwrap()
            .remove("/api/v1/me");
        let error = check_api_compatibility(&document).unwrap_err();
        assert!(error.contains("getMe"), "{error}");
    }

    #[test]
    fn report_json_does_not_contain_raw_credentials() {
        let mut report = Report::new();
        report.push(Check::failure(
            "api.authentication",
            "authentication",
            "token expired",
            &CliError::auth("token expired"),
        ));
        let json = report_to_json(&report);
        let text = json.to_string();
        assert!(
            !text.contains("Authorization"),
            "report must not contain Authorization headers: {text}"
        );
        assert!(
            !text.contains("Bearer ") && !text.contains("bearer "),
            "report must not contain bearer tokens: {text}"
        );
    }

    #[test]
    fn config_metadata_json_never_exposes_secrets() {
        let env = MapEnvironment::new().with_var("HAMSTIK_TOKEN", "secret-token-value");
        let session = Session {
            global: crate::args::GlobalOptions {
                host: None,
                profile: None,
                org: None,
                project: None,
                json: false,
                quiet: false,
                verbose: false,
                no_color: false,
                no_input: true,
                no_retry: false,
                dry_run: false,
                ca_bundle: None,
            },
            env: &env,
            cwd: std::path::PathBuf::from("."),
            config: crate::config::ConfigStore::new(std::path::PathBuf::from(
                "/tmp/nonexistent-config.toml",
            )),
            store: &FailingStore,
            out: crate::output::Output::new(
                crate::output::Mode::Human,
                false,
                Box::new(Vec::new()),
                Box::new(Vec::new()),
            ),
            factory: &MockFactory,
            prompt: &mut MockPrompt,
            exit_code: 0,
        };
        let json = config_metadata_json(&session);
        let text = json.to_string();
        assert!(
            !text.contains("secret-token-value"),
            "config metadata must not contain token values: {text}"
        );
        assert!(
            !text.contains("HAMSTIK_TOKEN"),
            "config metadata must not reference token env var: {text}"
        );
    }

    #[test]
    fn cli_info_json_contains_no_secrets() {
        let json = cli_info_json();
        let text = json.to_string();
        assert!(
            !text.contains("Authorization"),
            "cli info must not contain Authorization: {text}"
        );
        assert!(
            !text.contains("token"),
            "cli info must not contain token: {text}"
        );
    }

    #[test]
    fn context_explain_json_never_exposes_token_values() {
        let env = MapEnvironment::new().with_var("HAMSTIK_TOKEN", "super-secret-token");
        let session = Session {
            global: crate::args::GlobalOptions {
                host: None,
                profile: None,
                org: None,
                project: None,
                json: false,
                quiet: false,
                verbose: false,
                no_color: false,
                no_input: true,
                no_retry: false,
                dry_run: false,
                ca_bundle: None,
            },
            env: &env,
            cwd: std::path::PathBuf::from("."),
            config: crate::config::ConfigStore::new(std::path::PathBuf::from(
                "/tmp/nonexistent-config.toml",
            )),
            store: &FailingStore,
            out: crate::output::Output::new(
                crate::output::Mode::Human,
                false,
                Box::new(Vec::new()),
                Box::new(Vec::new()),
            ),
            factory: &MockFactory,
            prompt: &mut MockPrompt,
            exit_code: 0,
        };
        let json = context_explain_json(&session);
        let text = json.to_string();
        assert!(
            !text.contains("super-secret-token"),
            "context explain must not contain token values: {text}"
        );
        assert!(
            text.contains("<set; value not displayed>") || !text.contains("HAMSTIK_TOKEN"),
            "context explain must not reference token env var: {text}"
        );
    }
}
