// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Human-friendly date/time command-line input (CLI-24).
//!
//! Date and time flags accept convenient expressions and normalize them to a
//! single absolute instant that is then sent to the Public API in the canonical
//! RFC 3339 UTC form the contract expects. Nothing here changes what is sent on
//! the wire beyond supplying that instant: server-side filters (including the
//! server-computed `--overdue` filter) stay server-computed, and `--json`
//! output keeps carrying canonical timestamps only.
//!
//! # Accepted input
//!
//! - Explicit RFC 3339 timestamps, unchanged from before this feature:
//!   `2026-09-01T14:30:00Z`, `2026-09-01T14:30:00.250+02:00`.
//! - A bare calendar date: `2026-09-01`.
//! - A civil (timezone-less) date and time, with `T` or a single space:
//!   `2026-09-01T14:30`, `2026-09-01 14:30`, `2026-09-01 14:30:15`,
//!   `2026-09-01 14:30:15.500`.
//! - The keywords `today`, `yesterday`, and `tomorrow`.
//! - A relative offset: an optional sign, a non-negative integer count, and a
//!   unit — `7d`, `2w`, `12h`, `45m`, `90s`, `1mo`, `1y`, `+3h`, `-2d`.
//!   Accepted unit spellings are `s|sec|secs|second|seconds`,
//!   `m|min|mins|minute|minutes`, `h|hr|hrs|hour|hours`, `d|day|days`,
//!   `w|wk|wks|week|weeks`, `mo|month|months`, and `y|yr|yrs|year|years`.
//!   Unit spelling is case-insensitive.
//!
//! A relative offset is applied to the current instant truncated to whole
//! seconds: `7d` is one week before now, `+7d` is one week after it.
//!
//! Relative expressions are input convenience only. They add no domain
//! semantics: `7d` never means "due soon" or "overdue", it means a specific
//! instant that would have to be typed out by hand otherwise.
//!
//! # Timezone rule (one rule, applied consistently)
//!
//! Every input that does not carry its own UTC offset — a bare date, a civil
//! date and time, `today`/`yesterday`/`tomorrow`, and the calendar arithmetic
//! behind `mo`/`y` — is interpreted in the **host machine's local time zone**
//! and converted to UTC. RFC 3339 input keeps its own offset, and a relative
//! offset is applied to the current instant.
//!
//! Two consequences are pinned down rather than left to chance:
//!
//! - An ambiguous local time (a clock fall-back overlap) resolves to the
//!   earliest matching instant. A nonexistent local time (a clock
//!   spring-forward skipped it) moves forward past the gap, so `02:30` on a day
//!   whose clock jumps 02:00 to 03:00 resolves to `03:30` on the far side.
//! - Day and week offsets are exact 24-hour durations, while `mo` and `y` use
//!   calendar arithmetic clamped to the end of the target month
//!   (`2026-01-31 + 1mo` is `2026-02-28`).
//!
//! The stored value is always an absolute UTC instant, so downstream code never
//! has to reason about time zones.

use std::fmt;

use chrono::offset::{Local, Offset};
use chrono::{
    DateTime, Days, Duration, Months, NaiveDate, NaiveDateTime, NaiveTime, SecondsFormat,
    SubsecRound, TimeZone, Utc,
};

use crate::output::Output;

/// A date/time value supplied on the command line, resolved to an absolute
/// instant. Displays as the canonical RFC 3339 UTC form sent to the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TimeArg {
    inner: DateTime<Utc>,
}

impl TimeArg {
    /// Returns the underlying UTC instant.
    #[must_use]
    pub fn into_inner(self) -> DateTime<Utc> {
        self.inner
    }
}

impl std::str::FromStr for TimeArg {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        // Relative and keyword forms are resolved against the current instant
        // truncated to whole seconds, so a resolved value never carries clock
        // noise into the request.
        parse_at(raw.trim(), Utc::now().trunc_subsecs(0), &Local)
    }
}

impl fmt::Display for TimeArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `AutoSi` keeps sub-second precision that an explicit RFC 3339 input
        // carried, so normalization never silently moves the instant.
        write!(
            f,
            "{}",
            self.inner.to_rfc3339_opts(SecondsFormat::AutoSi, true)
        )
    }
}

/// Reports the resolved instant of every supplied date/time flag on `--verbose`
/// so the exact value sent to the Public API is inspectable. Flags that were
/// not supplied are skipped.
pub fn report_resolved(out: &mut Output, entries: &[(&str, Option<&TimeArg>)]) {
    for (flag, value) in entries {
        if let Some(value) = value {
            out.verbose(&format!("{flag} resolved to {value}"));
        }
    }
}

/// Parses `raw` against `now` and the local-time rule evaluated through `zone`.
///
/// `now` and `zone` are parameters so relative and calendar behavior is
/// unit-testable without depending on the host clock or its time zone. The
/// production path always supplies the system clock and the system zone.
fn parse_at<Tz: TimeZone>(raw: &str, now: DateTime<Utc>, zone: &Tz) -> Result<TimeArg, String> {
    if raw.is_empty() {
        return Err("date/time value must not be empty".to_string());
    }

    let out_of_range = || format!("date/time {raw:?} is outside the supported range");

    let keyword_days = match raw.to_ascii_lowercase().as_str() {
        "today" => Some(0),
        "yesterday" => Some(-1),
        "tomorrow" => Some(1),
        _ => None,
    };
    if let Some(days) = keyword_days {
        let inner = day_start(now, zone, days).ok_or_else(out_of_range)?;
        return Ok(TimeArg { inner });
    }

    if let Some((sign, count, unit)) = split_relative(raw) {
        let inner = relative_instant(sign, count, unit, now, zone).ok_or_else(out_of_range)?;
        return Ok(TimeArg { inner });
    }

    // Explicit RFC 3339 keeps its own offset; it is only re-expressed in UTC.
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Ok(TimeArg {
            inner: dt.with_timezone(&Utc),
        });
    }

    // Dates must be written in the documented `YYYY-MM-DD` shape: chrono's
    // numeric fields are width-lenient, so the shape is checked here instead of
    // accepting whatever its parser happens to swallow.
    if let Some((date_part, rest)) = raw.split_at_checked(10)
        && starts_with_canonical_date(raw)
    {
        if rest.is_empty() {
            if let Ok(date) = NaiveDate::parse_from_str(date_part, "%Y-%m-%d") {
                let inner =
                    civil_to_utc(date.and_time(NaiveTime::MIN), zone).ok_or_else(out_of_range)?;
                return Ok(TimeArg { inner });
            }
        } else if matches!(rest.as_bytes()[0], b'T' | b't' | b' ') {
            // Civil date and time without an offset: `T` and a space are
            // equivalent. Tried after RFC 3339 so an offset-bearing value
            // is never mis-read.
            let normalized = format!("{date_part} {}", &rest[1..]);
            for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M"] {
                if let Ok(naive) = NaiveDateTime::parse_from_str(&normalized, format) {
                    let inner = civil_to_utc(naive, zone).ok_or_else(out_of_range)?;
                    return Ok(TimeArg { inner });
                }
            }
        }
    }

    Err(invalid(raw))
}

/// The shared parse-failure message for an expression no accepted format fits.
fn invalid(raw: &str) -> String {
    format!(
        "invalid date/time {raw:?}: expected RFC 3339 (2026-09-01T14:30:00Z), \
         a date (2026-09-01), a local date and time (2026-09-01 14:30), \
         today/yesterday/tomorrow, or a relative offset such as 7d, 2w, 12h, \
         1mo or +3h"
    )
}

/// True when `raw` starts with a `YYYY-MM-DD` calendar date.
fn starts_with_canonical_date(raw: &str) -> bool {
    let digits = |range: std::ops::Range<usize>| {
        raw.get(range)
            .is_some_and(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    raw.len() >= 10
        && digits(0..4)
        && raw.as_bytes()[4] == b'-'
        && digits(5..7)
        && raw.as_bytes()[7] == b'-'
        && digits(8..10)
}

/// Start of the local calendar day `days` from `now`'s local day.
fn day_start<Tz: TimeZone>(now: DateTime<Utc>, zone: &Tz, days: i64) -> Option<DateTime<Utc>> {
    let today = now.with_timezone(zone).date_naive();
    let magnitude = days.unsigned_abs();
    let date = if days < 0 {
        today.checked_sub_days(Days::new(magnitude))?
    } else {
        today.checked_add_days(Days::new(magnitude))?
    };
    civil_to_utc(date.and_time(NaiveTime::MIN), zone)
}

/// Resolves a relative offset against `now`.
///
/// Sub-month units are exact durations; `mo`/`y` are calendar arithmetic on the
/// local civil calendar (clamped to the end of the month), which is what makes
/// "one month after January 31st" land on February 28th rather than March 3rd.
fn relative_instant<Tz: TimeZone>(
    sign: i64,
    count: u64,
    unit: RelativeUnit,
    now: DateTime<Utc>,
    zone: &Tz,
) -> Option<DateTime<Utc>> {
    let amount = i64::try_from(count).ok()?.checked_mul(sign)?;
    match unit {
        RelativeUnit::Seconds => now.checked_add_signed(Duration::try_seconds(amount)?),
        RelativeUnit::Minutes => now.checked_add_signed(Duration::try_minutes(amount)?),
        RelativeUnit::Hours => now.checked_add_signed(Duration::try_hours(amount)?),
        RelativeUnit::Days => now.checked_add_signed(Duration::try_days(amount)?),
        RelativeUnit::Weeks => now.checked_add_signed(Duration::try_weeks(amount)?),
        RelativeUnit::Months | RelativeUnit::Years => {
            let months = match unit {
                RelativeUnit::Years => amount.checked_mul(12)?,
                _ => amount,
            };
            let magnitude = u32::try_from(months.unsigned_abs()).ok()?;
            let civil = now.with_timezone(zone).naive_local();
            let shifted = if months < 0 {
                civil.checked_sub_months(Months::new(magnitude))?
            } else {
                civil.checked_add_months(Months::new(magnitude))?
            };
            civil_to_utc(shifted, zone)
        }
    }
}

/// Interprets a timezone-less civil date/time in `zone`.
///
/// A DST overlap (a clock turned back) resolves to the earliest matching
/// instant. A DST gap (a clock spring-forward that skipped this wall-clock
/// time) has no matching instant at all, so the same wall-clock reading is
/// retried hour by hour past the gap — `02:30` on a day whose clock jumps
/// 02:00→03:00 becomes `03:30` on the far side, which is what `mktime`
/// normalization produces and keeps the answer identical on every platform.
fn civil_to_utc<Tz: TimeZone>(naive: NaiveDateTime, zone: &Tz) -> Option<DateTime<Utc>> {
    let platform = zone.from_local_datetime(&naive).earliest().or_else(|| {
        // No instant maps to this wall-clock time: step forward past the gap.
        (1..=48).find_map(|hours| {
            let shifted = naive.checked_add_signed(Duration::try_hours(hours)?)?;
            zone.from_local_datetime(&shifted).earliest()
        })
    })?;
    let base = platform.with_timezone(&Utc);

    // A fall-back overlap has two matching instants and the host conversion may
    // answer with either, so pick the earliest one explicitly. Real-world clock
    // shifts are covered by the 15-to-120 minute steps below.
    let mut earliest = base;
    for minutes in [15, 30, 45, 60, 75, 90, 120] {
        let Some(shift) = Duration::try_minutes(minutes) else {
            continue;
        };
        for candidate in [
            base.checked_sub_signed(shift),
            base.checked_add_signed(shift),
        ] {
            if let Some(candidate) = candidate
                && local_wall_clock(zone, candidate) == Some(naive)
                && candidate < earliest
            {
                earliest = candidate;
            }
        }
    }
    Some(earliest)
}

/// The local wall-clock reading at `instant`.
fn local_wall_clock<Tz: TimeZone>(zone: &Tz, instant: DateTime<Utc>) -> Option<NaiveDateTime> {
    let offset = zone.offset_from_utc_datetime(&instant.naive_utc()).fix();
    instant
        .naive_utc()
        .checked_add_signed(Duration::try_seconds(i64::from(offset.local_minus_utc()))?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelativeUnit {
    Seconds,
    Minutes,
    Hours,
    Days,
    Weeks,
    Months,
    Years,
}

/// Splits `<sign><count><unit>` into its parts.
///
/// A bare expression (`7d`) points backwards in time — the reading that makes
/// `--updated-after 7d` mean "updated in the last week" — and `+` points
/// forwards (`+7d`). `-` is accepted as an explicit backwards marker. Input that
/// is not shaped like this (`2026-09-01`, `20260901`, `soon`) returns `None` so
/// the other formats get their turn.
fn split_relative(raw: &str) -> Option<(i64, u64, RelativeUnit)> {
    let (sign, rest) = match raw.as_bytes().first() {
        Some(b'+') => (1, &raw[1..]),
        Some(b'-') => (-1, &raw[1..]),
        _ => (-1, raw),
    };
    let split = rest.find(|c: char| !c.is_ascii_digit())?;
    let (digits, suffix) = rest.split_at(split);
    if digits.is_empty() || suffix.is_empty() || !suffix.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let count = digits.parse::<u64>().ok()?;
    let unit = match suffix.to_ascii_lowercase().as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => RelativeUnit::Seconds,
        "m" | "min" | "mins" | "minute" | "minutes" => RelativeUnit::Minutes,
        "h" | "hr" | "hrs" | "hour" | "hours" => RelativeUnit::Hours,
        "d" | "day" | "days" => RelativeUnit::Days,
        "w" | "wk" | "wks" | "week" | "weeks" => RelativeUnit::Weeks,
        "mo" | "mos" | "month" | "months" => RelativeUnit::Months,
        "y" | "yr" | "yrs" | "year" | "years" => RelativeUnit::Years,
        _ => return None,
    };
    Some((sign, count, unit))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn east(hours: i32) -> FixedOffset {
        FixedOffset::east_opt(hours * 3600).unwrap()
    }

    fn parse_at_zone(raw: &str, now: &str, zone: FixedOffset) -> String {
        parse_at(raw.trim(), utc(now), &zone).unwrap().to_string()
    }

    #[test]
    fn relative_units_resolve_backwards_from_now() {
        let now = "2026-06-15T12:00:00Z";
        let zone = FixedOffset::east_opt(0).unwrap();
        let cases = [
            ("90s", "2026-06-15T11:58:30Z"),
            ("45m", "2026-06-15T11:15:00Z"),
            ("1h", "2026-06-15T11:00:00Z"),
            ("3h", "2026-06-15T09:00:00Z"),
            ("7d", "2026-06-08T12:00:00Z"),
            ("2w", "2026-06-01T12:00:00Z"),
            ("1mo", "2026-05-15T12:00:00Z"),
            ("1y", "2025-06-15T12:00:00Z"),
            ("-2d", "2026-06-13T12:00:00Z"),
            ("+3h", "2026-06-15T15:00:00Z"),
            ("+1mo", "2026-07-15T12:00:00Z"),
            ("7D", "2026-06-08T12:00:00Z"),
        ];
        for (raw, expected) in cases {
            assert_eq!(parse_at_zone(raw, now, zone), expected, "input {raw:?}");
        }
    }

    #[test]
    fn month_and_year_arithmetic_clamps_to_end_of_month() {
        let zone = FixedOffset::east_opt(0).unwrap();
        assert_eq!(
            parse_at_zone("+1mo", "2026-01-31T12:00:00Z", zone),
            "2026-02-28T12:00:00Z"
        );
        assert_eq!(
            parse_at_zone("+1mo", "2024-01-31T12:00:00Z", zone),
            "2024-02-29T12:00:00Z",
            "leap years keep the extra day"
        );
        assert_eq!(
            parse_at_zone("-1mo", "2026-03-31T12:00:00Z", zone),
            "2026-02-28T12:00:00Z"
        );
        assert_eq!(
            parse_at_zone("+1y", "2024-02-29T12:00:00Z", zone),
            "2025-02-28T12:00:00Z"
        );
    }

    #[test]
    fn relative_days_are_exact_while_months_are_calendar() {
        let zone = FixedOffset::east_opt(0).unwrap();
        // 1 day before March 10 across a DST-free UTC zone is exactly 24h;
        assert_eq!(
            parse_at_zone("1d", "2026-03-10T12:00:00Z", zone),
            "2026-03-09T12:00:00Z"
        );
        // while 1 month keeps the civil time-of-day.
        assert_eq!(
            parse_at_zone("1mo", "2026-04-09T23:30:00Z", zone),
            "2026-03-09T23:30:00Z"
        );
    }

    #[test]
    fn keyword_days_use_the_local_calendar_date_across_utc_midnight() {
        // 01:30Z on 2026-03-08 is 03:30 local in UTC+2: the local day has
        // already turned, so "today" is 2026-03-08 locally.
        let plus_two = east(2);
        assert_eq!(
            parse_at_zone("today", "2026-03-08T01:30:00Z", plus_two),
            "2026-03-07T22:00:00Z"
        );
        assert_eq!(
            parse_at_zone("yesterday", "2026-03-08T01:30:00Z", plus_two),
            "2026-03-06T22:00:00Z"
        );
        assert_eq!(
            parse_at_zone("tomorrow", "2026-03-08T01:30:00Z", plus_two),
            "2026-03-08T22:00:00Z"
        );

        // 02:00Z on 2026-03-08 is still 2026-03-07 21:00 in UTC-5, so "today"
        // resolves to the earlier local day.
        let minus_five = east(-5);
        assert_eq!(
            parse_at_zone("today", "2026-03-08T02:00:00Z", minus_five),
            "2026-03-07T05:00:00Z"
        );
        assert_eq!(
            parse_at_zone("tomorrow", "2026-03-08T02:00:00Z", minus_five),
            "2026-03-08T05:00:00Z"
        );
    }

    #[test]
    fn bare_dates_and_civil_date_times_are_local() {
        let plus_two = east(2);
        assert_eq!(
            parse_at_zone("2026-09-01", "2020-01-01T00:00:00Z", plus_two),
            "2026-08-31T22:00:00Z"
        );
        assert_eq!(
            parse_at_zone("2026-09-01 14:30", "2020-01-01T00:00:00Z", plus_two),
            "2026-09-01T12:30:00Z"
        );
        assert_eq!(
            parse_at_zone("2026-09-01T14:30", "2020-01-01T00:00:00Z", plus_two),
            "2026-09-01T12:30:00Z"
        );
        assert_eq!(
            parse_at_zone("2026-09-01 14:30:15", "2020-01-01T00:00:00Z", plus_two),
            "2026-09-01T12:30:15Z"
        );
        assert_eq!(
            parse_at_zone("2026-09-01 14:30:15.500", "2020-01-01T00:00:00Z", plus_two),
            "2026-09-01T12:30:15.500Z"
        );
    }

    #[test]
    fn explicit_rfc3339_input_is_unchanged() {
        let zone = east(-7);
        // Its own offset wins over the local rule, and no precision is lost.
        assert_eq!(
            parse_at_zone("2026-09-01T14:30:00Z", "2020-01-01T00:00:00Z", zone),
            "2026-09-01T14:30:00Z"
        );
        assert_eq!(
            parse_at_zone(
                "2026-09-01T14:30:00.250+02:00",
                "2020-01-01T00:00:00Z",
                zone
            ),
            "2026-09-01T12:30:00.250Z"
        );
        assert_eq!(
            parse_at_zone("2024-02-29T23:59:59+00:00", "2020-01-01T00:00:00Z", zone),
            "2024-02-29T23:59:59Z"
        );
    }

    #[test]
    fn mixed_usage_resolves_each_flag_independently() {
        let now = "2026-06-15T12:00:00Z";
        let zone = FixedOffset::east_opt(0).unwrap();
        assert_eq!(parse_at_zone("7d", now, zone), "2026-06-08T12:00:00Z");
        assert_eq!(
            parse_at_zone("2026-06-10", now, zone),
            "2026-06-10T00:00:00Z"
        );
        assert_eq!(
            parse_at_zone("2026-06-12T08:00:00+01:00", now, zone),
            "2026-06-12T07:00:00Z"
        );
        assert_eq!(
            parse_at_zone("yesterday", now, zone),
            "2026-06-14T00:00:00Z"
        );
    }

    #[test]
    fn rejects_invalid_expressions() {
        let now = utc("2026-06-15T12:00:00Z");
        let zone = FixedOffset::east_opt(0).unwrap();
        for raw in [
            "",
            " ",
            "soon",
            "now",
            "7",
            "d",
            "7x",
            "12h30",
            "+",
            "-",
            "2026-13-01",
            "2026-02-30",
            "2026-09-01T99:00",
            "2026-09-01 24:00",
            "2026-09-01T14:30:00+99:99",
            "0.5d",
            "2026-09-1",
            "1 sep 2026",
        ] {
            let err = parse_at(raw.trim(), now, &zone).expect_err("{raw:?} must fail");
            assert!(
                err.contains("must not be empty") || err.contains("invalid date/time"),
                "{raw:?}: {err}"
            );
        }
    }

    #[test]
    fn rejects_out_of_range_relative_offsets() {
        let now = utc("2026-06-15T12:00:00Z");
        let zone = FixedOffset::east_opt(0).unwrap();
        // The count is too large for i64 nanoseconds: it must fail cleanly
        // rather than wrap around to an arbitrary instant.
        let err = parse_at("99999999999999999999999d", now, &zone)
            .expect_err("overflowing offset must fail");
        assert!(
            err.contains("out of range") || err.contains("invalid date/time"),
            "{err}"
        );
    }

    #[test]
    fn from_str_uses_the_system_clock_and_zone() {
        let today: TimeArg = "today".parse().unwrap();
        let tomorrow: TimeArg = "tomorrow".parse().unwrap();
        let a_week_ago: TimeArg = "7d".parse().unwrap();
        assert!(today < tomorrow);
        assert!(a_week_ago < today);
        assert!(today.to_string().ends_with('Z'));
        // Explicit RFC 3339 is unaffected by the host clock.
        assert_eq!(
            "2026-09-01T00:00:00Z"
                .parse::<TimeArg>()
                .unwrap()
                .to_string(),
            "2026-09-01T00:00:00Z"
        );
    }
}
