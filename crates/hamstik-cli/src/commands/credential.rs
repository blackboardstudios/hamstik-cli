// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! PAT credential readiness reporting.
//!
//! Shared by `auth status`, `me`, and `doctor`: expiry classification and the
//! granted-scope inventory. The Public API does not publish a scope
//! vocabulary, so "readiness" never claims authority beyond what the server
//! returned: unknown scope strings are reported verbatim and a missing scope
//! is described in terms of the command families the affected surface lists,
//! with the exact server error preserved as the authoritative signal.

use serde_json::{Value, json};

/// Days before expiry at which credentials are "approaching expiration".
pub(crate) const NEAR_EXPIRY_DAYS: i64 = 14;

/// Parses an RFC 3339 date-time into Unix seconds (UTC offsets supported).
///
/// Duplicated from `doctor`'s helper so both call sites share the algorithm;
/// the tests pin identical behavior on both surfaces.
pub(crate) fn parse_timestamp(value: &str) -> Option<i64> {
    let date_time = value.trim();
    let (date, rest) = date_time
        .split_once('T')
        .or_else(|| date_time.split_once(' '))?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;

    let (time, offset) =
        if let Some(stripped) = rest.strip_suffix('Z').or_else(|| rest.strip_suffix('z')) {
            (stripped, 0i64)
        } else if let Some(position) = rest.rfind(['+', '-']) {
            let (time, zone) = rest.split_at(position);
            let sign = if zone.starts_with('+') { 1 } else { -1 };
            let mut zone_parts = zone[1..].split(':');
            let hours: i64 = zone_parts.next().unwrap_or("0").parse().ok()?;
            let minutes: i64 = zone_parts.next().unwrap_or("0").parse().ok()?;
            (time, sign * (hours * 3600 + minutes * 60))
        } else {
            (rest, 0)
        };

    let mut time_parts = time.split(':');
    let hours: i64 = time_parts.next()?.parse().ok()?;
    let minutes: i64 = time_parts.next()?.parse().ok()?;
    let seconds_fragment = time_parts.next().unwrap_or("0");
    let seconds: i64 = seconds_fragment
        .split('.')
        .next()
        .unwrap_or(seconds_fragment)
        .parse()
        .ok()?;

    let (y, m) = if month <= 2 {
        (year - 1, month + 12)
    } else {
        (year, month)
    };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let year_of_era = y - era * 400;
    let day_of_era = (153 * (m - 3) + 2) / 5 + day - 1;
    let days = era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_era
        - 719_468;
    Some(days * 86_400 + hours * 3600 + minutes * 60 + seconds - offset)
}

/// Days from now until expiry (negative when expired).
pub(crate) fn days_until(expires_at: &str, now_seconds: i64) -> Option<i64> {
    let expiry = parse_timestamp(expires_at)?;
    Some((expiry - now_seconds).div_euclid(86_400))
}

/// Expiry classification for a credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Expiry {
    /// Already past its `expiresAt`.
    Expired,
    /// Within the warning window.
    Approaching(i64),
    /// Healthy.
    Valid(i64),
    /// `expiresAt` could not be parsed.
    Unknown,
}

/// Classifies credential expiry against the process clock.
pub(crate) fn classify_expiry(expires_at: &str, now_seconds: i64) -> Expiry {
    match days_until(expires_at, now_seconds) {
        Some(days) if days < 0 => Expiry::Expired,
        Some(days) if days <= NEAR_EXPIRY_DAYS => Expiry::Approaching(days),
        Some(days) => Expiry::Valid(days),
        None => Expiry::Unknown,
    }
}

/// A nonblocking expiry warning (or expiry failure detail) for human output.
pub(crate) fn expiry_summary(expires_at: &str, now_seconds: i64) -> String {
    match classify_expiry(expires_at, now_seconds) {
        Expiry::Expired => format!(
            "credential EXPIRED {expires_at}; every request will be rejected - create a new PAT and run `hamstik auth login --with-token`"
        ),
        Expiry::Approaching(days) => {
            format!("credential expires in {days} day(s) ({expires_at}); create a new PAT soon")
        }
        Expiry::Valid(days) => {
            format!("credential is valid for {days} more day(s) (expires {expires_at})")
        }
        Expiry::Unknown => format!("credential expiry {expires_at:?} could not be parsed"),
    }
}

/// Describes what the granted scopes cover for the CLI's command families.
///
/// The Public API does not publish a scope vocabulary, so this is a
/// transparency report over the server-returned scope list: the raw scopes,
/// their count, and an honest statement that per-command authorization is
/// decided by the server. When no scopes are granted, the affected families
/// are named so users know what to request from their administrator.
pub(crate) fn scope_report(scopes: &[String]) -> Value {
    if scopes.is_empty() {
        return json!({
            "granted": [],
            "count": 0,
            "readiness": "none",
            "note": "the credential returned no scopes; command families may be rejected at authorization time (exit 4). Ask your administrator to grant the scopes for the command families you use.",
            "affectedFamilies": [
                "read (work/project/sprint/label views)",
                "work mutations",
                "bulk operations",
                "comments/labels/links/attachments",
                "organization administration",
                "project administration",
                "profile/avatar/activity"
            ],
        });
    }
    json!({
        "granted": scopes,
        "count": scopes.len(),
        "readiness": "granted",
        "note": "the server decides per-command authorization from these scopes; an INSUFFICIENT_SCOPE or FORBIDDEN error (exit 4) names the missing authority exactly",
    })
}

/// One concise human line summarizing scopes for `auth status`/`me`.
pub(crate) fn scope_summary_line(scopes: &[String]) -> String {
    if scopes.is_empty() {
        "  scopes:    (none granted; mutations and most reads will be rejected)".to_string()
    } else {
        format!("  scopes:    {}", scopes.join(", "))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_timestamp_handles_utc_offsets_and_fractions() {
        assert_eq!(
            parse_timestamp("2026-01-01T00:00:00Z"),
            parse_timestamp("2026-01-01 00:00:00+00:00")
        );
        let base = parse_timestamp("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(parse_timestamp("2026-01-01T01:00:00+01:00"), Some(base));
        assert_eq!(parse_timestamp("2025-12-31T23:00:00-01:00"), Some(base));
        assert_eq!(
            parse_timestamp("2026-01-01T00:00:00.500Z"),
            Some(base), // fraction truncates toward the second
        );
        assert_eq!(parse_timestamp("garbage"), None);
    }

    #[test]
    fn expiry_classification_matches_documented_thresholds() {
        let now = parse_timestamp("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(
            classify_expiry("2025-12-31T00:00:00Z", now),
            Expiry::Expired
        );
        assert_eq!(
            classify_expiry("2026-01-10T00:00:00Z", now),
            Expiry::Approaching(9)
        );
        assert_eq!(
            classify_expiry("2027-01-01T00:00:00Z", now),
            Expiry::Valid(365)
        );
        assert_eq!(classify_expiry("garbage", now), Expiry::Unknown);
    }

    #[test]
    fn scope_report_never_invents_authority() {
        let empty = scope_report(&[]);
        assert_eq!(empty["readiness"], "none");
        assert!(empty["affectedFamilies"].as_array().unwrap().len() >= 6);

        let granted = scope_report(&["work:write".to_string(), "read".to_string()]);
        assert_eq!(granted["count"], 2);
        assert_eq!(granted["granted"][0], "work:write");
        assert!(
            granted["note"]
                .as_str()
                .unwrap()
                .contains("the server decides"),
            "the note must defer authority to the server"
        );
    }
}
