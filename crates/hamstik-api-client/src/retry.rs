// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Retry policy, transient-failure classification, backoff, and `Retry-After`
//! handling.
//!
//! Time is injectable via [`Sleeper`] so tests run deterministically without
//! real delays.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use uuid::Uuid;

/// The longest `Retry-After` we are willing to wait before giving up.
///
/// A server-directed wait beyond this returns a rate-limit error immediately
/// instead of blocking the caller.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

/// Sleep abstraction so tests can substitute a zero-duration sleeper.
#[async_trait]
pub trait Sleeper: Send + Sync {
    /// Waits for the given duration (async, non-blocking).
    async fn sleep(&self, duration: Duration);
}

/// Real sleeper backed by the Tokio timer.
pub struct TokioSleeper;

#[async_trait]
impl Sleeper for TokioSleeper {
    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}

/// Test sleeper that returns immediately.
pub struct NoopSleeper;

#[async_trait]
impl Sleeper for NoopSleeper {
    async fn sleep(&self, _duration: Duration) {}
}

/// Bounded exponential backoff policy with optional jitter.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Total attempts, including the first (e.g. `3` means up to 2 retries).
    pub attempts: u32,
    /// Base delay used for exponential growth.
    pub base: Duration,
    /// Upper bound for any single computed backoff delay.
    pub max_delay: Duration,
    /// Whether to apply +/-20% jitter to computed delays.
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base: Duration::from_millis(250),
            max_delay: Duration::from_secs(5),
            jitter: true,
        }
    }
}

impl RetryPolicy {
    /// A policy that never retries (used for `--no-retry`).
    pub fn none() -> Self {
        Self {
            attempts: 1,
            ..Self::default()
        }
    }
}

/// Returns true when an HTTP status is a transient failure eligible for retry.
#[must_use]
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 502 | 503 | 504)
}

/// Computes the backoff delay for a zero-based retry `attempt` index.
///
/// When `jitter` is enabled the result is scaled by a factor in `[0.8, 1.2)`,
/// derived from fresh randomness. The result is always capped at
/// [`RetryPolicy::max_delay`].
#[must_use]
pub fn backoff_delay(policy: &RetryPolicy, attempt: u32) -> Duration {
    let factor = 1u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX);
    let exp = policy.base.saturating_mul(factor).min(policy.max_delay);

    if !policy.jitter {
        return exp;
    }

    // Derive a jitter fraction in [0, 1) from the low bits of a fresh UUID.
    let id = Uuid::new_v4();
    let bytes = id.as_bytes();
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let fraction = (raw as f64) / (u32::MAX as f64); // [0.0, 1.0]
    // Map to [0.8, 1.2).
    let scale = 0.8 + (fraction * 0.4);
    let scaled = (exp.as_secs_f64() * scale).max(0.0);
    Duration::from_secs_f64(scaled).min(policy.max_delay)
}

/// Parses a `Retry-After` header value (delta-seconds or HTTP-date) into a
/// non-negative duration relative to `now`.
#[must_use]
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let when = httpdate::parse_http_date(value).ok()?;
    Some(when.duration_since(now).unwrap_or(Duration::ZERO))
}

/// Shared, thread-safe sleeper handle used by clients.
pub type SharedSleeper = Arc<dyn Sleeper>;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn retryable_status_set() {
        for status in [429u16, 502, 503, 504] {
            assert!(is_retryable_status(status), "{status} should retry");
        }
        for status in [400u16, 401, 403, 404, 409, 412, 500, 501] {
            assert!(!is_retryable_status(status), "{status} should not retry");
        }
    }

    #[test]
    fn backoff_grows_and_caps_without_jitter() {
        let policy = RetryPolicy {
            jitter: false,
            ..RetryPolicy::default()
        };
        assert_eq!(backoff_delay(&policy, 0), Duration::from_millis(250));
        assert_eq!(backoff_delay(&policy, 1), Duration::from_millis(500));
        assert_eq!(backoff_delay(&policy, 2), Duration::from_millis(1000));
        assert_eq!(backoff_delay(&policy, 40), Duration::from_secs(5));
    }

    #[test]
    fn jitter_stays_within_bounds() {
        let policy = RetryPolicy::default();
        for _ in 0..200 {
            let d = backoff_delay(&policy, 1);
            // 500ms +/-20% => [400ms, 600ms]
            assert!(d >= Duration::from_millis(400));
            assert!(d <= Duration::from_millis(600));
        }
    }

    #[test]
    fn parses_retry_after_seconds() {
        let now = SystemTime::now();
        assert_eq!(parse_retry_after("5", now), Some(Duration::from_secs(5)));
    }

    #[test]
    fn parses_retry_after_http_date_in_future() {
        let now = SystemTime::now();
        let future = now + Duration::from_secs(30);
        let header = httpdate::fmt_http_date(future);
        let parsed = parse_retry_after(&header, now).unwrap();
        assert!(parsed <= Duration::from_secs(31));
        assert!(parsed >= Duration::from_secs(28));
    }

    #[test]
    fn parses_retry_after_past_date_as_zero() {
        let now = SystemTime::now();
        let past = now - Duration::from_secs(120);
        let header = httpdate::fmt_http_date(past);
        assert_eq!(parse_retry_after(&header, now), Some(Duration::ZERO));
    }

    #[test]
    fn empty_or_invalid_retry_after_is_none() {
        let now = SystemTime::now();
        assert_eq!(parse_retry_after("", now), None);
        assert_eq!(parse_retry_after("not-a-date", now), None);
    }

    #[test]
    fn none_policy_has_single_attempt() {
        assert_eq!(RetryPolicy::none().attempts, 1);
    }
}
