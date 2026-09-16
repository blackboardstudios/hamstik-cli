// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Canonical error types shared by the Hamstik API client.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::Value;
use thiserror::Error;

/// Errors produced while validating or normalizing a configured host.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum HostError {
    /// The configured host string is empty or only whitespace.
    #[error("host must not be empty")]
    Empty,
    /// The host is not a parseable URL.
    #[error("invalid host URL: {0}")]
    InvalidUrl(String),
    /// The scheme is neither `https` nor `http`.
    #[error("unsupported URL scheme {0:?}; only https (or http for loopback) is allowed")]
    UnsupportedScheme(String),
    /// Plain `http` was used for a host that is not loopback.
    #[error("insecure http is only permitted for loopback hosts")]
    InsecureScheme,
    /// The URL embeds a username or password.
    #[error("embedded credentials are not allowed in the host URL")]
    CredentialsForbidden,
    /// The URL carries a query string or fragment.
    #[error("query and fragment components are not allowed in the host URL")]
    QueryFragmentForbidden,
    /// The URL carries a path; the client builds `/api/v1` paths itself.
    #[error("host URL must not include a path; the /api/v1 prefix is added automatically")]
    PathForbidden,
    /// A request path segment would navigate the path (`""`, `"."`, `".."`).
    #[error("path segment {0:?} is not allowed (empty, \".\", and \"..\" would navigate the path)")]
    ForbiddenPathSegment(String),
}

/// The transport stage at which a network-level failure occurred.
///
/// Derived from the underlying request error so diagnostics can name the
/// earliest failing layer (DNS, TCP, proxy, timeout, TLS) without exposing
/// credentials or raw header dumps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkStage {
    /// The hostname could not be resolved.
    Dns,
    /// The TCP connection could not be established or was refused.
    Connection,
    /// A configured proxy rejected or failed the request.
    Proxy,
    /// The request exceeded its time budget.
    Timeout,
    /// The TLS handshake or certificate/hostname verification failed.
    Tls,
    /// The failure could not be attributed to a specific stage.
    Unknown,
}

impl NetworkStage {
    /// The stable machine-readable stage name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dns => "dns",
            Self::Connection => "connection",
            Self::Proxy => "proxy",
            Self::Timeout => "timeout",
            Self::Tls => "tls",
            Self::Unknown => "unknown",
        }
    }
}

/// A structured error returned by the Public API.
///
/// The server's stable error `code` is always preserved verbatim so callers can
/// branch on values such as `REVISION_CONFLICT` rather than parsing prose.
#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct ApiError {
    /// The HTTP status the server returned.
    pub status: u16,
    /// The stable server error code (e.g. `NOT_FOUND`).
    pub code: String,
    /// The server's human-readable message (control characters stripped).
    pub message: String,
    /// Correlation id from the envelope or `X-Request-Id` header.
    pub request_id: Option<String>,
    /// Per-field validation messages, keyed by field name.
    pub field_errors: BTreeMap<String, Vec<String>>,
    /// Operation-specific structured context supplied by the server.
    pub details: Option<BTreeMap<String, Value>>,
    /// The parsed `Retry-After` header, when the server sent one.
    pub retry_after: Option<Duration>,
    /// The server's rate-limit snapshot from `RateLimit-*` headers, when the
    /// rejection carried them.
    pub rate_limit: Option<crate::client::RateLimitSnapshot>,
}

impl ApiError {
    /// Returns true when the error carries the given stable server code.
    pub fn is_code(&self, code: &str) -> bool {
        self.code == code
    }
}

/// Classifies a rendered transport-error message into the earliest failing
/// network stage.
///
/// This is the *fallback* classification: the client crate computes the stage
/// from the concrete `reqwest` source chain first and only falls back to
/// marker matching on the rendered text when no source in the chain was
/// recognizable. The markers are ordered so that more specific failures (TLS
/// surfaces as a connect error) are checked first.
#[must_use]
pub fn network_stage_from_message(message: &str) -> NetworkStage {
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("certificate") || lowered.contains("tls") {
        NetworkStage::Tls
    } else if lowered.contains("proxy") {
        NetworkStage::Proxy
    } else if lowered.contains("timed out") || lowered.contains("timeout") {
        NetworkStage::Timeout
    } else if lowered.contains("dns")
        || lowered.contains("name or service not known")
        || lowered.contains("resolve")
    {
        NetworkStage::Dns
    } else if lowered.contains("connect")
        || lowered.contains("refused")
        || lowered.contains("unreachable")
    {
        NetworkStage::Connection
    } else {
        NetworkStage::Unknown
    }
}

/// The single canonical client error type.
#[derive(Debug, Error)]
pub enum ClientError {
    /// The server returned a structured (or parseable) error envelope.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// A transport-level failure occurred before a usable response arrived.
    ///
    /// The stage is classified once, at the point where the concrete
    /// transport error is in hand (see
    /// [`crate::client::classify_network_error`]), rather than being
    /// re-derived from the rendered message.
    #[error("network error: {message}")]
    Network {
        /// The rendered transport error (no credentials, no raw headers).
        message: String,
        /// The classified transport stage.
        stage: NetworkStage,
    },
    /// The server response violated the expected contract.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// A configured host was invalid.
    #[error(transparent)]
    Host(#[from] HostError),
}

impl ClientError {
    /// Extracts the [`ApiError`] if this is an API error.
    pub fn as_api(&self) -> Option<&ApiError> {
        match self {
            ClientError::Api(api) => Some(api),
            _ => None,
        }
    }

    /// Returns the transport stage this failure was classified into at
    /// construction time.
    ///
    /// Always [`NetworkStage::Unknown`] for non-network variants.
    #[must_use]
    pub fn network_stage(&self) -> NetworkStage {
        match self {
            ClientError::Network { stage, .. } => *stage,
            _ => NetworkStage::Unknown,
        }
    }
}
