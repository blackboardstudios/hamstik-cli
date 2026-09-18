// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Canonical CLI error type.
//!
//! Every failure — whether local (usage, configuration, credential store) or
//! server-originated — becomes a single [`CliError`]. The error carries enough
//! structure to (a) map to a stable exit code and (b) render either a human
//! message or the stable JSON failure envelope (SPEC §54).

use std::collections::BTreeMap;

use serde_json::{Value, json};
use thiserror::Error;

use hamstik_api_client::ClientError;
use hamstik_api_client::error::ApiError;

use crate::exit;

/// The stable `kind` discriminator in the JSON error envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// A structured error returned by the Public API.
    Api,
    /// Usage / invalid local command input.
    Usage,
    /// Local configuration problem (config file, host URL, context).
    Configuration,
    /// Credential-store (OS keyring) failure distinct from auth.
    CredentialStore,
    /// Network / transport failure.
    Network,
    /// Server contract/compatibility problem.
    Protocol,
    /// Local operation requires an authenticated identity.
    Auth,
    /// Unexpected internal/IO failure.
    Internal,
}

impl ErrorKind {
    /// The stable `kind` string used in JSON envelopes.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Api => "api",
            ErrorKind::Usage => "usage",
            ErrorKind::Configuration => "configuration",
            ErrorKind::CredentialStore => "credential_store",
            ErrorKind::Network => "network",
            ErrorKind::Protocol => "protocol",
            ErrorKind::Auth => "authentication",
            ErrorKind::Internal => "internal",
        }
    }
}

/// A CLI error with enough metadata to map to exit codes and JSON output.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct CliError {
    /// Broad failure category driving the exit code.
    pub kind: ErrorKind,
    /// Stable machine-readable error code.
    pub code: String,
    /// Human-readable message (rendered to stderr).
    pub message: String,
    /// Server correlation id, when the failure came from an API response.
    pub request_id: Option<String>,
    /// HTTP status, when the failure came from an API response.
    pub status: Option<u16>,
    /// Per-field validation errors returned by the Public API.
    pub field_errors: Box<BTreeMap<String, Vec<String>>>,
    /// Operation-specific structured error details returned by the API.
    pub details: Option<Box<BTreeMap<String, Value>>>,
    /// Underlying error, when one exists.
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Clone for CliError {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind,
            code: self.code.clone(),
            message: self.message.clone(),
            request_id: self.request_id.clone(),
            status: self.status,
            field_errors: self.field_errors.clone(),
            details: self.details.clone(),
            source: None,
        }
    }
}

impl CliError {
    fn new(kind: ErrorKind, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind,
            code: code.into(),
            message: message.into(),
            request_id: None,
            status: None,
            field_errors: Box::default(),
            details: None,
            source: None,
        }
    }

    /// Builds a usage error (exit 2).
    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Usage, "INVALID_INPUT", message)
    }

    /// Builds a local configuration error (exit 10).
    pub fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Configuration, "CONFIGURATION_ERROR", message)
    }

    /// Builds a credential-store error (exit 10).
    pub fn credential(message: impl Into<String>) -> Self {
        Self::new(
            ErrorKind::CredentialStore,
            "CREDENTIAL_STORE_ERROR",
            message,
        )
    }

    /// Builds a network/transport error (exit 8).
    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, "NETWORK_ERROR", message)
    }

    /// Builds a server-contract error (exit 9).
    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Protocol, "PROTOCOL_ERROR", message)
    }

    /// Builds an internal/IO error (exit 1).
    pub fn general(message: impl std::fmt::Display) -> Self {
        Self::new(ErrorKind::Internal, "INTERNAL_ERROR", message.to_string())
    }

    /// Builds an authentication-required error (exit 3).
    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Auth, "AUTH_REQUIRED", message)
    }

    /// Builds an authorization/insufficient-scope error (exit 4).
    pub fn authorization(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Api, "FORBIDDEN", message)
    }

    /// Builds a client-side stand-in for a server `NOT_FOUND` API error
    /// (exit 5); used when a local lookup cannot produce the id the request
    /// needs.
    pub fn not_found(message: impl Into<String>) -> Self {
        let mut error = Self::new(ErrorKind::Api, "NOT_FOUND", message);
        error.status = Some(404);
        error
    }

    /// Maps a client (transport/API) error into a CLI error.
    pub fn from_client(err: ClientError) -> Self {
        match err {
            ClientError::Api(api) => Self::from_api(api),
            ClientError::Network { message, .. } => Self::network(message),
            ClientError::Protocol(message) => Self::protocol(message),
            ClientError::Host(host) => Self::config(format!("invalid host: {host}")),
        }
    }

    /// Maps a structured API error, preserving the server code and request id.
    pub fn from_api(api: ApiError) -> Self {
        Self {
            kind: ErrorKind::Api,
            code: api.code.clone(),
            message: api.message.clone(),
            request_id: api.request_id.clone(),
            status: Some(api.status),
            field_errors: Box::new(api.field_errors.clone()),
            details: api.details.clone().map(Box::new),
            source: None,
        }
    }

    /// Returns the same structured error with an enriched human message.
    ///
    /// Machine-readable fields (kind, code, request id, status, field
    /// errors, details) are preserved verbatim so automation sees identical
    /// contract data; only the displayed message gains CLI-added remediation.
    #[must_use]
    pub fn with_reminded_remediation(mut self, suffix: impl Into<String>) -> Self {
        self.message = format!("{}{}", self.message, suffix.into());
        self
    }

    /// The stable process exit code for this error.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        if self.kind == ErrorKind::Api {
            exit_code_for_api_code(&self.code, self.status.unwrap_or(0))
        } else {
            match self.kind {
                ErrorKind::Usage => exit::USAGE,
                ErrorKind::Configuration => exit::CONFIGURATION,
                ErrorKind::CredentialStore => exit::CONFIGURATION,
                ErrorKind::Network => exit::NETWORK,
                ErrorKind::Protocol => exit::SERVER,
                ErrorKind::Api => exit::GENERAL,
                ErrorKind::Auth => exit::AUTHENTICATION,
                ErrorKind::Internal => exit::GENERAL,
            }
        }
    }

    /// Builds the stable JSON failure envelope (SPEC §54).
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut error = json!({
            "kind": self.kind.as_str(),
            "code": self.code,
            "message": self.message,
        });
        if let Some(request_id) = &self.request_id {
            error["requestId"] = Value::String(request_id.clone());
        }
        if let Some(status) = self.status {
            error["status"] = Value::from(status);
        }
        if !self.field_errors.is_empty() {
            error["fieldErrors"] = json!(self.field_errors);
        }
        if let Some(details) = &self.details {
            error["details"] = json!(details);
        }
        json!({ "error": error })
    }
}

/// Maps a stable server error code (and HTTP status) to an exit code.
fn exit_code_for_api_code(code: &str, status: u16) -> i32 {
    match code {
        "AUTH_REQUIRED" | "INVALID_TOKEN" => exit::AUTHENTICATION,
        "INSUFFICIENT_SCOPE" | "FORBIDDEN" | "ORGANIZATION_SUSPENDED" => exit::AUTHORIZATION,
        "NOT_FOUND" => exit::NOT_FOUND,
        "CONFLICT"
        | "REVISION_CONFLICT"
        | "IDEMPOTENCY_KEY_REUSED"
        | "IDEMPOTENCY_REQUEST_IN_PROGRESS"
        | "INVALID_STATUS_TRANSITION"
        | "WORK_ITEM_STATUS_REQUIRES_TRANSITION"
        | "INVALID_SPRINT_TRANSITION"
        | "ACTIVE_SPRINT_EXISTS"
        | "SPRINT_HAS_UNFINISHED_WORK_ITEMS"
        | "INVALID_COMPLETION_TARGET"
        | "LINK_DUPLICATE"
        | "LINK_CONTRADICTION"
        | "WORK_ITEM_ARCHIVED"
        | "WORK_ITEM_HAS_CHILDREN"
        | "PROJECT_ARCHIVED" => exit::CONFLICT,
        "RATE_LIMITED" => exit::RATE_LIMITED,
        "VALIDATION_ERROR"
        | "INVALID_CURSOR"
        | "PAYLOAD_TOO_LARGE"
        | "PRECONDITION_REQUIRED"
        | "IDEMPOTENCY_KEY_REQUIRED" => exit::USAGE,
        // Anything else: fall back to HTTP-status semantics.
        _ if (500..600).contains(&status) => exit::SERVER,
        _ => exit::GENERAL,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn api(code: &str, status: u16) -> CliError {
        CliError::from_api(ApiError {
            status,
            code: code.to_string(),
            message: "msg".to_string(),
            request_id: Some("req".to_string()),
            field_errors: Default::default(),
            details: None,
            retry_after: None,
            rate_limit: None,
        })
    }

    #[test]
    fn maps_server_codes_to_exit_codes() {
        assert_eq!(api("AUTH_REQUIRED", 401).exit_code(), exit::AUTHENTICATION);
        assert_eq!(api("FORBIDDEN", 403).exit_code(), exit::AUTHORIZATION);
        assert_eq!(api("NOT_FOUND", 404).exit_code(), exit::NOT_FOUND);
        assert_eq!(api("REVISION_CONFLICT", 412).exit_code(), exit::CONFLICT);
        assert_eq!(api("RATE_LIMITED", 429).exit_code(), exit::RATE_LIMITED);
        assert_eq!(api("VALIDATION_ERROR", 400).exit_code(), exit::USAGE);
        assert_eq!(api("INTERNAL_ERROR", 500).exit_code(), exit::SERVER);
        assert_eq!(api("LINK_DUPLICATE", 409).exit_code(), exit::CONFLICT);
        assert_eq!(api("LINK_CONTRADICTION", 409).exit_code(), exit::CONFLICT);
        assert_eq!(api("WORK_ITEM_ARCHIVED", 409).exit_code(), exit::CONFLICT);
        assert_eq!(
            api("WORK_ITEM_HAS_CHILDREN", 409).exit_code(),
            exit::CONFLICT
        );
        assert_eq!(api("PROJECT_ARCHIVED", 409).exit_code(), exit::CONFLICT);
    }

    #[test]
    fn local_kinds_map_to_exit_codes() {
        assert_eq!(CliError::usage("x").exit_code(), exit::USAGE);
        assert_eq!(CliError::config("x").exit_code(), exit::CONFIGURATION);
        assert_eq!(CliError::credential("x").exit_code(), exit::CONFIGURATION);
        assert_eq!(CliError::network("x").exit_code(), exit::NETWORK);
        assert_eq!(CliError::protocol("x").exit_code(), exit::SERVER);
    }

    #[test]
    fn json_envelope_shape() {
        let value = api("REVISION_CONFLICT", 412).to_json();
        assert_eq!(value["error"]["kind"], "api");
        assert_eq!(value["error"]["code"], "REVISION_CONFLICT");
        assert_eq!(value["error"]["status"], 412);
        assert_eq!(value["error"]["requestId"], "req");
    }
}
