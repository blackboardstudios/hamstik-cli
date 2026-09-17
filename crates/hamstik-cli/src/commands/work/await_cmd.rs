// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work await` — poll until a Work Item reaches a server-reported condition.

use std::time::Duration;

use humantime::{format_duration, parse_duration};
use tokio::time::{Instant, sleep};

use hamstik_api_client::{ClientError, WorkItem};

use crate::app::Session;
use crate::error::CliError;

use super::emit_json;

const MAX_TIMEOUT: Duration = Duration::from_secs(3600); // 1 hour
const MIN_POLL_INTERVAL: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub(super) async fn await_item(
    session: &mut Session<'_>,
    args: &crate::args::WorkAwaitArgs,
) -> Result<(), CliError> {
    let timeout = parse_duration_arg(&args.timeout)?;
    let target_statuses: Vec<String> = args.status.iter().map(|s| s.as_str().to_string()).collect();

    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let start = Instant::now();
    let deadline = start + timeout;
    let mut poll_interval = MIN_POLL_INTERVAL;
    let mut last_item: Option<WorkItem> = None;

    loop {
        if Instant::now() >= deadline {
            let last_status = last_item
                .as_ref()
                .map(|i| i.status.clone())
                .unwrap_or_else(|| "unknown".to_string());
            let elapsed = format_duration(start.elapsed());
            let target_desc = if target_statuses.is_empty() {
                "reachable".to_string()
            } else {
                format!("status in {:?}", target_statuses)
            };
            return Err(CliError::general(format!(
                "timed out after {} waiting for {} to reach {}; last observed status: {}",
                elapsed, args.key, target_desc, last_status
            )));
        }

        match api.get_work_item(&org, &project, &args.key).await {
            Ok(response) => {
                last_item = Some(response.value.clone());
                let current_status = &response.value.status;
                let condition_met =
                    target_statuses.is_empty() || target_statuses.contains(current_status);

                if condition_met {
                    if session.json() {
                        let value =
                            serde_json::to_value(&response.value).map_err(CliError::general)?;
                        emit_json(session, &value)?;
                    } else {
                        session
                            .out
                            .line(&format!(
                                "{} reached target condition (status: {})",
                                args.key, current_status
                            ))
                            .map_err(CliError::general)?;
                    }
                    return Ok(());
                }

                // Condition not met yet; wait and retry.
                let remaining = deadline.duration_since(Instant::now());
                if remaining.is_zero() {
                    continue;
                }
                let wait = poll_interval.min(remaining);
                sleep(wait).await;
                poll_interval = (poll_interval * 2).min(MAX_BACKOFF);
            }
            Err(ClientError::Api(api_err)) => match api_err.code.as_str() {
                "NOT_FOUND" => {
                    return Err(CliError::not_found(format!(
                        "work item {} not found",
                        args.key
                    )));
                }
                "RATE_LIMITED" => {
                    let retry_after = api_err
                        .retry_after
                        .unwrap_or_else(|| Duration::from_secs(1));
                    let remaining = deadline.duration_since(Instant::now());
                    if retry_after > remaining {
                        // Would exceed timeout; fall through to timeout check on next loop.
                        sleep(remaining).await;
                    } else {
                        sleep(retry_after).await;
                    }
                }
                "AUTH_REQUIRED" | "INVALID_TOKEN" => {
                    return Err(CliError::auth(format!(
                        "authentication failed while waiting for {}",
                        args.key
                    )));
                }
                "FORBIDDEN" | "INSUFFICIENT_SCOPE" => {
                    return Err(CliError::authorization(format!(
                        "authorization failed while waiting for {}",
                        args.key
                    )));
                }
                _ => {
                    // Other API errors: back off and retry within budget.
                    let remaining = deadline.duration_since(Instant::now());
                    if remaining.is_zero() {
                        continue;
                    }
                    let wait = poll_interval.min(remaining);
                    sleep(wait).await;
                    poll_interval = (poll_interval * 2).min(MAX_BACKOFF);
                }
            },
            Err(ClientError::Network { .. }) => {
                let remaining = deadline.duration_since(Instant::now());
                if remaining.is_zero() {
                    continue;
                }
                let wait = poll_interval.min(remaining);
                sleep(wait).await;
                poll_interval = (poll_interval * 2).min(MAX_BACKOFF);
            }
            Err(err) => {
                let remaining = deadline.duration_since(Instant::now());
                if remaining.is_zero() {
                    continue;
                }
                let wait = poll_interval.min(remaining);
                sleep(wait).await;
                poll_interval = (poll_interval * 2).min(MAX_BACKOFF);
                let _ = err;
            }
        }
    }
}

fn parse_duration_arg(input: &str) -> Result<Duration, CliError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(CliError::usage("timeout must not be empty"));
    }
    let duration = parse_duration(trimmed).map_err(|_| {
        CliError::usage(format!(
            "invalid timeout duration: {trimmed}; use formats like 10m, 1h, 30s"
        ))
    })?;
    if duration > MAX_TIMEOUT {
        return Err(CliError::usage(format!(
            "timeout {} exceeds the 1h upper bound",
            humantime::format_duration(duration)
        )));
    }
    if duration.is_zero() {
        return Err(CliError::usage("timeout must be greater than zero"));
    }
    Ok(duration)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::time::Duration as StdDuration;

    #[test]
    fn parses_common_durations() {
        assert_eq!(
            parse_duration_arg("10m").unwrap(),
            StdDuration::from_secs(600)
        );
        assert_eq!(
            parse_duration_arg("1h").unwrap(),
            StdDuration::from_secs(3600)
        );
        assert_eq!(
            parse_duration_arg("30s").unwrap(),
            StdDuration::from_secs(30)
        );
        assert_eq!(
            parse_duration_arg("1h").unwrap(),
            StdDuration::from_secs(3600)
        );
        assert_eq!(
            parse_duration_arg("30m").unwrap(),
            StdDuration::from_secs(1800)
        );
    }

    #[test]
    fn rejects_invalid_durations() {
        assert!(parse_duration_arg("foo").is_err());
        assert!(parse_duration_arg("").is_err());
        assert!(parse_duration_arg("1x").is_err());
    }

    #[test]
    fn rejects_timeout_above_max() {
        let err = parse_duration_arg("2h").unwrap_err();
        assert!(err.to_string().contains("1h upper bound"));
    }
}
