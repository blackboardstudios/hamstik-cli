// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik replay` — show recent failed-request journal entries.
//!
//! This is a strictly local read. It prints the redacted request context the
//! CLI captured when a request failed so a user or support engineer can
//! correlate the failure with server-side diagnostics using the request id.
//! It never re-sends a request and never contacts the network.

use serde_json::Value;

use crate::app::Session;
use crate::error::CliError;
use crate::journal;

use super::emit_json;

/// Runs `hamstik replay`.
pub fn run(session: &mut Session<'_>, last: usize) -> Result<(), CliError> {
    if last == 0 {
        return Err(CliError::usage("--last must be at least 1"));
    }

    let entries = journal::read(last).map_err(CliError::general)?;

    if session.json() {
        return emit_json(session, &Value::Array(entries));
    }

    if entries.is_empty() {
        return session
            .out
            .line(
                "no failed-request journal entries; server errors and network failures \
                 are recorded locally as they happen",
            )
            .map_err(CliError::general);
    }

    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|entry| {
            vec![
                text(entry, "when"),
                text(entry, "method"),
                text(entry, "status"),
                text(entry, "requestId"),
                text(entry, "path"),
                text(entry, "durationMs"),
            ]
        })
        .collect();

    session
        .out
        .table(
            &["WHEN", "METHOD", "STATUS", "REQUEST ID", "PATH", "MS"],
            &rows,
        )
        .map_err(CliError::general)
}

/// Renders one journal field for the human table, using `-` when absent.
fn text(entry: &Value, key: &str) -> String {
    match entry.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        _ => "-".to_string(),
    }
}
