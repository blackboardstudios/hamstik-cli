// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Typed client for the Hamstik Public API v1.
//!
//! This crate is the only place that talks to `/api/v1`. It exposes a small
//! [`HamstikApi`] trait (implemented by [`HamstikClient`]) with one method per
//! supported operation, plus the request/response models, pagination helpers,
//! idempotency utilities, retry/backoff policy, and a canonical error type.
//!
//! The CLI depends exclusively on this crate — never on `reqwest` or raw HTTP.

pub mod client;
pub mod error;
pub mod host;
pub mod idempotency;
pub mod models;
pub mod pagination;
pub mod retry;

/// The `/api/v1` prefix every request is built under.
pub const API_PREFIX: &str = "/api/v1";

pub use client::{ApiResponse, ClientConfig, HamstikApi, HamstikClient, sanitize_server_text};
pub use error::{ApiError, ClientError, HostError};
pub use host::Host;
pub use idempotency::{IdempotencyKeyError, generate_key, validate_key};
pub use models::*;
pub use pagination::{MAX_FOLLOW_ITEMS, MAX_FOLLOW_PAGES, Page, PageItems, follow_all};
pub use retry::{RetryPolicy, Sleeper, TokioSleeper};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn api_prefix_matches_contract() {
        assert_eq!(API_PREFIX, "/api/v1");
    }
}
