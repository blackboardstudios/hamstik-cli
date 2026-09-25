// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work watch` — follow a Work Item's activity and comments by polling.
//!
//! The Public API v1 contract exposes the item's activity feed (strictly-after
//! `since`) and its comment thread (oldest first, cursor-paged). `watch` polls
//! both, renders each entry the server returns, and advances only on success so
//! a transient failure never loses an entry. It is deliberately distinct from
//! `work await`: `await` blocks on one condition and exits, `watch` streams
//! every new entry until interrupted.

use std::collections::HashSet;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use hamstik_api_client::{ActivityOptions, ClientError, HamstikApi, ListOptions};
use humantime::{format_duration, parse_duration};
use serde_json::{Map, Value, json};
use tokio::process::Command as TokioCommand;
use tokio::time::sleep;

use crate::app::Session;
use crate::args::WorkWatchArgs;
use crate::error::CliError;
use crate::output::Mode;

/// Lower bound on the poll interval; keeps tests fast without spinning.
const MIN_INTERVAL: Duration = Duration::from_millis(100);
/// Upper bound on the poll interval; keeps the watch responsive.
const MAX_INTERVAL: Duration = Duration::from_secs(3600);
/// Ceiling for exponential reconnect backoff.
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// One page is the server maximum (1–200), so a poll follows cursors fully.
const PAGE_LIMIT: u32 = 200;

/// A failure from one poll, split so the loop can distinguish a transient
/// server/transport failure (back off and retry) from a local rendering error
/// (stop).
enum PollFailure {
    /// A transport or API error from one of the reads.
    Api(ClientError),
    /// A local output/IO error.
    Local(CliError),
}

/// The mutable cursor/baseline state of one watch session.
struct WatchState {
    /// The Work Item key being watched.
    key: String,
    /// The optional notify hook command.
    notify: Option<String>,
    /// Activity feed baseline; advanced to the newest event seen.
    activity_since: DateTime<Utc>,
    /// Activity ids already rendered, so a re-read never duplicates an event.
    seen_activity: HashSet<String>,
    /// Comment baseline; fixed for the session (comments have no `since`).
    baseline: DateTime<Utc>,
    /// The cursor that fetched the newest comment page, so later polls resume
    /// near the end of the thread instead of re-scanning from the first page.
    comment_page_cursor: Option<String>,
    /// Comment ids already considered, so a re-read never duplicates a reply.
    seen_comments: HashSet<String>,
}

/// Runs `work watch` until interrupted or a fatal error.
pub(super) async fn watch(session: &mut Session<'_>, args: &WorkWatchArgs) -> Result<(), CliError> {
    let interval = parse_interval(&args.interval)?;
    // An unbounded event stream has no table shape: `--json`/`--jsonl` stream
    // NDJSON events and the default mode renders human lines. Reject the
    // table formats explicitly rather than silently ignoring them.
    if matches!(session.out.mode(), Mode::Tsv | Mode::Csv | Mode::Markdown) {
        return Err(CliError::usage(
            "`work watch` streams events; use --json/--jsonl for an event stream or the default human rendering (table formats do not model an unbounded stream)",
        ));
    }
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let baseline = args
        .since
        .map(|since| since.into_inner())
        .unwrap_or_else(Utc::now);
    let mut state = WatchState {
        key: args.key.clone(),
        notify: args.notify.clone(),
        activity_since: baseline,
        seen_activity: HashSet::new(),
        baseline,
        comment_page_cursor: None,
        seen_comments: HashSet::new(),
    };

    // Register the interrupt handler once; a second Ctrl-C is not needed.
    let mut shutdown = Box::pin(tokio::signal::ctrl_c());
    let mut backoff = interval;

    loop {
        match poll_once(session, &api, &org, &project, &mut state).await {
            Ok(()) => backoff = interval,
            Err(PollFailure::Local(err)) => return Err(err),
            Err(PollFailure::Api(err)) => {
                if is_fatal(&err) {
                    return Err(CliError::from_client(err));
                }
                let wait = retry_delay(&err).unwrap_or(backoff).min(MAX_INTERVAL);
                session.out.warn(&format!(
                    "watch: {err}; retrying in {}",
                    format_duration(wait)
                ));
                backoff = (backoff * 2).min(MAX_BACKOFF);
                tokio::select! {
                    _ = &mut shutdown => return interrupted(session, &args.key),
                    () = sleep(wait) => {}
                }
                continue;
            }
        }

        tokio::select! {
            _ = &mut shutdown => return interrupted(session, &args.key),
            () = sleep(interval) => {}
        }
    }
}

/// Polls both streams once. A failure in one stream does not suppress the
/// other; the first error decides the retry.
async fn poll_once(
    session: &mut Session<'_>,
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    state: &mut WatchState,
) -> Result<(), PollFailure> {
    let activity = poll_activity(session, api, org, project, state).await;
    let comments = poll_comments(session, api, org, project, state).await;
    activity?;
    comments?;
    Ok(())
}

/// Fetches and renders new activity events (oldest first, following cursors).
async fn poll_activity(
    session: &mut Session<'_>,
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    state: &mut WatchState,
) -> Result<(), PollFailure> {
    // `since` is strictly-after; subtract a millisecond so events sharing the
    // newest timestamp are re-read (and deduplicated) rather than skipped.
    let since = (state.activity_since - ChronoDuration::milliseconds(1))
        .to_rfc3339_opts(SecondsFormat::AutoSi, true);
    let mut cursor: Option<String> = None;

    loop {
        let opts = ActivityOptions {
            limit: Some(PAGE_LIMIT),
            cursor: cursor.clone(),
            since: Some(since.clone()),
        };
        let response = api
            .list_work_item_activity(org, project, &state.key, opts)
            .await
            .map_err(PollFailure::Api)?;
        let page = response.value.page.clone();

        if let Some(items) = response.raw.get("items").and_then(Value::as_array) {
            for item in items {
                let id = string_field(item, "id");
                if id.is_empty() || state.seen_activity.contains(&id) {
                    continue;
                }
                state.seen_activity.insert(id.clone());
                if let Some(created) = parse_ts(&string_field(item, "createdAt"))
                    && created > state.activity_since
                {
                    state.activity_since = created;
                }
                publish(session, state, "activity", item)
                    .await
                    .map_err(PollFailure::Local)?;
            }
        }

        if page.has_more {
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        } else {
            break;
        }
    }
    Ok(())
}

/// Fetches and renders new comments (oldest first, following cursors).
async fn poll_comments(
    session: &mut Session<'_>,
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    state: &mut WatchState,
) -> Result<(), PollFailure> {
    let mut cursor = state.comment_page_cursor.clone();

    loop {
        let opts = ListOptions {
            limit: Some(PAGE_LIMIT),
            cursor: cursor.clone(),
        };
        let response = api
            .list_comments(org, project, &state.key, opts)
            .await
            .map_err(PollFailure::Api)?;
        let page = response.value.page.clone();

        if let Some(items) = response.raw.get("items").and_then(Value::as_array) {
            for item in items {
                let id = string_field(item, "id");
                if id.is_empty() || state.seen_comments.contains(&id) {
                    continue;
                }
                state.seen_comments.insert(id.clone());
                let created = parse_ts(&string_field(item, "createdAt"));
                if created.is_some_and(|created| created > state.baseline) {
                    publish(session, state, "comment", item)
                        .await
                        .map_err(PollFailure::Local)?;
                }
            }
        }

        // Remember the cursor that fetched this (the newest) page so the next
        // poll resumes here instead of re-walking the whole thread.
        state.comment_page_cursor = cursor.clone();

        if page.has_more {
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        } else {
            break;
        }
    }
    Ok(())
}

/// Renders one server event and runs the optional notify hook.
async fn publish(
    session: &mut Session<'_>,
    state: &WatchState,
    kind: &str,
    item: &Value,
) -> Result<(), CliError> {
    match session.out.mode() {
        Mode::Json | Mode::JsonLines => {
            let mut envelope = Map::new();
            envelope.insert("type".to_string(), json!(kind));
            envelope.insert("item".to_string(), json!(state.key));
            envelope.insert(kind.to_string(), item.clone());
            session
                .out
                .emit_event(&Value::Object(envelope))
                .map_err(CliError::general)?;
        }
        _ => {
            session
                .out
                .human(&render_human_line(kind, item))
                .map_err(CliError::general)?;
            session.out.flush().map_err(CliError::general)?;
        }
    }

    if let Some(command) = &state.notify {
        run_notify(session, command, state, kind, item).await;
    }
    Ok(())
}

/// Renders a human-readable single line for one event.
fn render_human_line(kind: &str, item: &Value) -> String {
    let created = string_field(item, "createdAt");
    let id = string_field(item, "id");
    if kind == "activity" {
        let action = string_field(item, "action");
        let actor = item
            .get("actor")
            .and_then(|actor| actor.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        let detail = item
            .get("detail")
            .filter(|detail| !detail.is_null())
            .map(Value::to_string)
            .unwrap_or_default();
        format!("[{created}] activity {action} by {actor} {detail}")
            .trim_end()
            .to_string()
    } else {
        let author = item
            .get("author")
            .and_then(|author| author.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        let body = item
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or("(deleted)");
        let preview: String = body.chars().take(120).collect();
        format!("[{created}] comment {id} by {author}: {preview}")
    }
}

/// Runs the `--notify` hook for one event through the platform shell.
///
/// The hook's stdout is discarded so it can never corrupt a `--json` event
/// stream; stderr is inherited for diagnostics. A non-zero exit or spawn
/// failure is reported and never stops the watch.
async fn run_notify(
    session: &mut Session<'_>,
    command: &str,
    state: &WatchState,
    kind: &str,
    item: &Value,
) {
    let mut child = if cfg!(windows) {
        let mut cmd = TokioCommand::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    } else {
        let mut cmd = TokioCommand::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    };
    let id = string_field(item, "id");
    child
        .env("HAMSTIK_WATCH_ITEM", &state.key)
        .env("HAMSTIK_WATCH_TYPE", kind)
        .env("HAMSTIK_WATCH_ID", &id)
        .env(
            "HAMSTIK_WATCH_JSON",
            serde_json::to_string(item).unwrap_or_default(),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    if let Some(action) = item.get("action").and_then(Value::as_str) {
        child.env("HAMSTIK_WATCH_ACTION", action);
    }
    match child.status().await {
        Ok(status) if status.success() => {}
        Ok(status) => session.out.warn(&format!(
            "watch: notify hook for {kind} {id} exited with {status}"
        )),
        Err(err) => session
            .out
            .warn(&format!("watch: failed to run notify hook: {err}")),
    }
}

/// The first string field of `item` (`""` when absent).
fn string_field(item: &Value, name: &str) -> String {
    item.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Parses an RFC 3339 timestamp into UTC.
fn parse_ts(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

/// Reports the stop on stderr and returns success (Ctrl-C is a clean stop).
fn interrupted(session: &mut Session<'_>, key: &str) -> Result<(), CliError> {
    session.out.warn(&format!("watch: stopped watching {key}"));
    Ok(())
}

/// True when a transport/API error cannot fix itself and must stop the watch.
///
/// Authentication, authorization, not-found, and other non-5xx API responses
/// cannot fix themselves, so the watch stops with the server's own error.
/// Network failures, 5xx responses, and rate limits are transient.
fn is_fatal(err: &ClientError) -> bool {
    match err {
        ClientError::Api(api) => api.status < 500 && api.code != "RATE_LIMITED",
        ClientError::Network { .. } => false,
        ClientError::Protocol(_) | ClientError::Host(_) => true,
    }
}

/// The server's `Retry-After` delay for a rate limit, when present.
fn retry_delay(err: &ClientError) -> Option<Duration> {
    match err {
        ClientError::Api(api) if api.code == "RATE_LIMITED" => {
            Some(api.retry_after.unwrap_or_else(|| Duration::from_secs(1)))
        }
        _ => None,
    }
}

/// Parses and bounds a `--interval` value.
fn parse_interval(input: &str) -> Result<Duration, CliError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(CliError::usage("interval must not be empty"));
    }
    let duration = parse_duration(trimmed).map_err(|_| {
        CliError::usage(format!(
            "invalid interval duration: {trimmed}; use formats like 2s or 500ms"
        ))
    })?;
    if duration < MIN_INTERVAL {
        return Err(CliError::usage(format!(
            "interval {} is below the 100ms lower bound",
            format_duration(duration)
        )));
    }
    if duration > MAX_INTERVAL {
        return Err(CliError::usage(format!(
            "interval {} exceeds the 1h upper bound",
            format_duration(duration)
        )));
    }
    Ok(duration)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_bounds_intervals() {
        assert_eq!(parse_interval("2s").unwrap(), Duration::from_secs(2));
        assert_eq!(parse_interval("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_interval("100ms").unwrap(), Duration::from_millis(100));
    }

    #[test]
    fn rejects_out_of_range_intervals() {
        assert!(parse_interval("10ms").is_err());
        assert!(parse_interval("2h").is_err());
        assert!(parse_interval("").is_err());
        assert!(parse_interval("nope").is_err());
    }

    #[test]
    fn renders_human_activity_and_comment_lines() {
        let activity = json!({
            "id": "a1",
            "action": "status_changed",
            "actor": {"publicId": "usr_x", "name": "Ada"},
            "detail": {"from": "todo", "to": "done"},
            "createdAt": "2026-01-02T00:00:00Z"
        });
        let line = render_human_line("activity", &activity);
        assert!(line.contains("status_changed"));
        assert!(line.contains("Ada"));

        let comment = json!({
            "id": "c1",
            "author": {"publicId": "usr_x", "name": "Ada"},
            "body": "hello world",
            "createdAt": "2026-01-02T00:00:00Z"
        });
        let line = render_human_line("comment", &comment);
        assert!(line.contains("comment c1"));
        assert!(line.contains("Ada"));
        assert!(line.contains("hello world"));
    }

    #[test]
    fn classifies_transient_and_fatal_errors() {
        use hamstik_api_client::error::{ApiError, NetworkStage};
        use std::collections::BTreeMap;

        let api = |status: u16, code: &str| {
            ClientError::Api(ApiError {
                status,
                code: code.to_string(),
                message: code.to_string(),
                request_id: None,
                field_errors: BTreeMap::new(),
                details: None,
                retry_after: None,
                rate_limit: None,
            })
        };

        assert!(is_fatal(&api(401, "AUTH_REQUIRED")));
        assert!(is_fatal(&api(403, "FORBIDDEN")));
        assert!(is_fatal(&api(404, "NOT_FOUND")));
        assert!(!is_fatal(&api(500, "INTERNAL_ERROR")));
        assert!(!is_fatal(&api(429, "RATE_LIMITED")));
        assert!(!is_fatal(&ClientError::Network {
            message: "boom".to_string(),
            stage: NetworkStage::Connection,
        }));
    }
}
