// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Local, redacted journal of failed Public API requests.
//!
//! When a request fails, the CLI appends one JSON line describing the request
//! *shape* so support can correlate a local failure with the authoritative
//! server-side diagnostics using the server request id:
//!
//! ```json
//! {"when":"2026-01-02T09:12:44Z","method":"GET",
//!  "path":"/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
//!  "headerIntent":["accept","authorization"],"requestId":"0f2c9b1e",
//!  "status":500,"durationMs":184,"transport":false}
//! ```
//!
//! The record is deliberately narrow. It never contains a token,
//! `Authorization` value, request or response body, or query string: only the
//! method, the path, the *names* of the headers the request intended to send,
//! the status, the server request id, and the elapsed time. The journal is
//! local-only and is never uploaded automatically.
//!
//! Retention is a documented, bounded policy:
//!
//! - entries older than [`MAX_AGE`] (7 days) are dropped;
//! - at most [`MAX_ENTRIES`] (500) entries are retained;
//! - compaction runs once the file exceeds [`MAX_BYTES`] (256 KiB) or its
//!   oldest entry passes [`MAX_AGE`], so a healthy journal is append-only in
//!   the common case.
//!
//! Writing is best-effort: a journal I/O failure never changes a command's
//! outcome, and a missing entry simply means the record could not be written.
//!
//! Location and policy are documented in the user documentation (`README.md`
//! — "Failed-request journal") and reported by `hamstik doctor`.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use directories::ProjectDirs;
use hamstik_api_client::{RequestObservation, RequestObserver};
use serde_json::{Value, json};

/// The default journal file name.
const FILE_NAME: &str = "request-journal.log";

/// Environment override for the journal path.
///
/// Mirrors `HAMSTIK_AUDIT_LOG`: it exists so the location can be pinned in
/// tests, containers, and automation without inventing a second configuration
/// mechanism. An empty value is ignored.
const PATH_ENV: &str = "HAMSTIK_REQUEST_JOURNAL";

/// Maximum age of a retained entry (7 days).
pub const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Maximum number of retained entries.
pub const MAX_ENTRIES: usize = 500;

/// Compact once the file grows beyond this many bytes.
pub const MAX_BYTES: u64 = 256 * 1024;

/// Returns the effective journal path for the current platform.
///
/// The journal lives in the per-user state directory (it is state, not data
/// the user would migrate):
///   - Linux:  `$XDG_STATE_HOME/hamstik/request-journal.log`,
///     default `~/.local/state/hamstik/request-journal.log`
///   - macOS:  `~/Library/Application Support/hamstik/request-journal.log`
///   - Windows: `%LOCALAPPDATA%\hamstik\request-journal.log`
///
/// macOS and Windows have no XDG state directory, so the per-user data-local
/// directory is used there. Returns `None` only when no home directory can be
/// resolved at all.
#[must_use]
pub fn path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os(PATH_ENV)
        && !explicit.is_empty()
    {
        return Some(PathBuf::from(explicit));
    }
    let dirs = ProjectDirs::from("com", "blackboard", "hamstik")?;
    let base = dirs.state_dir().unwrap_or_else(|| dirs.data_local_dir());
    Some(base.join(FILE_NAME))
}

/// An observer that appends one redacted entry per failed request.
///
/// The observer performs synchronous, best-effort local I/O; it never blocks
/// on the network and never panics.
pub struct JournalObserver;

impl RequestObserver for JournalObserver {
    fn observe(&self, observation: &RequestObservation) {
        record(observation);
    }
}

/// Appends one journal entry for a failed request.
///
/// Any I/O error is swallowed: a broken journal must not change the command's
/// outcome. Compaction is attempted opportunistically after each append.
fn record(observation: &RequestObservation) {
    let Some(journal_path) = path() else {
        return;
    };
    let entry = entry(observation);
    if append(&journal_path, &entry).is_ok() {
        let _ = compact_if_needed(&journal_path);
    }
}

/// Builds one journal record.
///
/// Every value is written as a JSON scalar or array of header names, so
/// nothing can smuggle a body, query string, or credential into a record.
fn entry(observation: &RequestObservation) -> Value {
    json!({
        "when": crate::commands::timestamp_rfc3339(),
        "method": observation.method,
        "path": observation.path,
        "headerIntent": observation.header_names,
        "requestId": observation.request_id,
        "status": observation.status,
        "durationMs": observation.duration.as_millis().min(u128::from(u64::MAX)) as u64,
        "transport": observation.transport,
    })
}

/// Appends one serialized record, creating the parent directory if needed.
fn append(journal_path: &Path, entry: &Value) -> Result<(), String> {
    use std::io::Write as _;

    if let Some(parent) = journal_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal_path)
        .map_err(|e| e.to_string())?;

    // The record is non-secret in the credential sense but still describes
    // private work; keep it owner-only like the config and audit files.
    let _ = crate::fsutil::restrict_permissions(journal_path);

    let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())?;
    Ok(())
}

/// Reads up to the `last` most recent entries, oldest first.
///
/// Unreadable or corrupt lines are skipped rather than failing the read: the
/// journal is a diagnostic convenience, not a source of truth.
pub fn read(last: usize) -> Result<Vec<Value>, String> {
    let Some(journal_path) = path() else {
        return Ok(Vec::new());
    };
    read_from(&journal_path, last)
}

fn read_from(journal_path: &Path, last: usize) -> Result<Vec<Value>, String> {
    let content = match std::fs::read_to_string(journal_path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(format!("cannot read {}: {err}", journal_path.display())),
    };
    let mut entries: Vec<Value> = content
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    if entries.len() > last {
        entries.drain(..entries.len() - last);
    }
    Ok(entries)
}

/// Compacts the journal when it has grown beyond [`MAX_BYTES`] or its oldest
/// entry has passed [`MAX_AGE`].
fn compact_if_needed(journal_path: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(journal_path).map_err(|e| e.to_string())?;
    if metadata.len() > MAX_BYTES || oldest_entry_expired(journal_path)? {
        return compact(journal_path);
    }
    Ok(())
}

/// Whether the first parseable entry is older than [`MAX_AGE`].
///
/// Reads only the leading lines, so the age trigger stays cheap on a healthy
/// append-only journal. Corrupt leading lines are skipped; compaction drops
/// them regardless.
fn oldest_entry_expired(journal_path: &Path) -> Result<bool, String> {
    use std::io::{BufRead as _, BufReader};

    let file = std::fs::File::open(journal_path).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(file);
    let cutoff = Utc::now() - ChronoDuration::from_std(MAX_AGE).unwrap_or_default();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Ok(false);
        }
        let Some(entry) = serde_json::from_str::<Value>(line.trim()).ok() else {
            continue;
        };
        return Ok(!within_age(&entry, cutoff));
    }
}

/// Rewrites the journal, retaining only entries within [`MAX_AGE`] and the
/// most recent [`MAX_ENTRIES`] of those.
fn compact(journal_path: &Path) -> Result<(), String> {
    let content = std::fs::read_to_string(journal_path).map_err(|e| e.to_string())?;
    let cutoff = Utc::now() - ChronoDuration::from_std(MAX_AGE).unwrap_or_default();
    let mut entries: Vec<Value> = content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| within_age(entry, cutoff))
        .collect();
    if entries.len() > MAX_ENTRIES {
        entries.drain(..entries.len() - MAX_ENTRIES);
    }
    let mut rewritten = String::new();
    for entry in &entries {
        rewritten.push_str(&serde_json::to_string(entry).map_err(|e| e.to_string())?);
        rewritten.push('\n');
    }
    crate::fsutil::write_atomic(journal_path, rewritten.as_bytes()).map_err(|e| e.to_string())
}

/// Whether an entry's `when` timestamp is at or after `cutoff`.
///
/// Entries whose timestamp cannot be parsed are dropped during compaction;
/// the writer always emits RFC 3339, so an unparseable line is corruption.
fn within_age(entry: &Value, cutoff: DateTime<Utc>) -> bool {
    entry
        .get("when")
        .and_then(Value::as_str)
        .and_then(|when| DateTime::parse_from_rfc3339(when).ok())
        .is_some_and(|when| when.with_timezone(&Utc) >= cutoff)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn observation() -> RequestObservation {
        RequestObservation {
            method: "GET".to_string(),
            path: "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1".to_string(),
            header_names: vec!["accept".to_string(), "authorization".to_string()],
            status: Some(500),
            request_id: Some("req-1".to_string()),
            duration: Duration::from_millis(184),
            transport: false,
        }
    }

    #[test]
    fn entry_contains_only_the_documented_fields() {
        let record = entry(&observation());
        let keys: std::collections::BTreeSet<&str> = record
            .as_object()
            .map(|map| map.keys().map(String::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                "durationMs",
                "headerIntent",
                "method",
                "path",
                "requestId",
                "status",
                "transport",
                "when",
            ])
        );
        assert_eq!(record["method"], "GET");
        assert_eq!(record["status"], 500);
        assert_eq!(record["requestId"], "req-1");
        assert_eq!(record["durationMs"], 184);
        assert_eq!(record["transport"], false);
        assert_eq!(record["headerIntent"], json!(["accept", "authorization"]));
    }

    #[test]
    fn entry_never_carries_secrets_bodies_or_query_strings() {
        let mut obs = observation();
        // A path is the only free-form server-facing string; a query string is
        // never part of it, and no value can leak a credential.
        obs.path = "/api/v1/organizations/acme/work-items".to_string();
        let serialized = entry(&obs).to_string();
        for forbidden in [
            "Authorization:",
            "Bearer",
            "token=",
            "password",
            "body",
            "?query",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "unexpected {forbidden:?} in journal record: {serialized}"
            );
        }
    }

    #[test]
    fn append_then_read_roundtrips_in_order() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("request-journal.log");
        append(&log, &entry(&observation())).unwrap();
        let mut second = observation();
        second.request_id = Some("req-2".to_string());
        second.status = None;
        second.transport = true;
        append(&log, &entry(&second)).unwrap();

        let entries = read_from(&log, 10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["requestId"], "req-1");
        assert_eq!(entries[1]["requestId"], "req-2");
        assert!(entries[1]["status"].is_null());
        assert_eq!(entries[1]["transport"], true);
    }

    #[test]
    fn read_returns_only_the_most_recent_entries() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("request-journal.log");
        for index in 0..5 {
            let mut obs = observation();
            obs.request_id = Some(format!("req-{index}"));
            append(&log, &entry(&obs)).unwrap();
        }
        let entries = read_from(&log, 2).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["requestId"], "req-3");
        assert_eq!(entries[1]["requestId"], "req-4");
    }

    #[test]
    fn read_of_a_missing_journal_is_empty() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("absent.log");
        assert!(read_from(&log, 10).unwrap().is_empty());
    }

    #[test]
    fn compact_drops_entries_older_than_max_age() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("request-journal.log");
        let old = json!({
            "when": "2000-01-01T00:00:00Z",
            "method": "GET",
            "path": "/api/v1/me",
            "headerIntent": ["accept"],
            "requestId": null,
            "status": 500,
            "durationMs": 1,
            "transport": false,
        });
        let recent = entry(&observation());
        std::fs::write(
            &log,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&old).unwrap(),
                serde_json::to_string(&recent).unwrap()
            ),
        )
        .unwrap();

        compact(&log).unwrap();
        let entries = read_from(&log, 10).unwrap();
        assert_eq!(entries.len(), 1, "old entry must expire");
        assert_eq!(entries[0]["requestId"], "req-1");
    }

    #[test]
    fn compact_retains_at_most_max_entries() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("request-journal.log");
        let mut contents = String::new();
        for index in 0..(MAX_ENTRIES + 25) {
            let mut obs = observation();
            obs.request_id = Some(format!("req-{index}"));
            contents.push_str(&serde_json::to_string(&entry(&obs)).unwrap());
            contents.push('\n');
        }
        std::fs::write(&log, contents).unwrap();

        compact(&log).unwrap();
        let entries = read_from(&log, usize::MAX).unwrap();
        assert_eq!(entries.len(), MAX_ENTRIES);
        // The oldest entries are the ones dropped.
        assert_eq!(entries[0]["requestId"], "req-25");
        assert_eq!(
            entries[MAX_ENTRIES - 1]["requestId"],
            format!("req-{}", MAX_ENTRIES + 24)
        );
    }

    #[test]
    fn compact_if_needed_leaves_small_journals_untouched() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("request-journal.log");
        append(&log, &entry(&observation())).unwrap();
        let before = std::fs::metadata(&log).unwrap().len();
        compact_if_needed(&log).unwrap();
        assert_eq!(std::fs::metadata(&log).unwrap().len(), before);
    }

    #[test]
    fn compact_if_needed_prunes_a_small_journal_whose_oldest_entry_expired() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("request-journal.log");
        let old = json!({
            "when": "2000-01-01T00:00:00Z",
            "method": "GET",
            "path": "/api/v1/me",
            "headerIntent": ["accept"],
            "requestId": null,
            "status": 500,
            "durationMs": 1,
            "transport": false,
        });
        std::fs::write(
            &log,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&old).unwrap(),
                serde_json::to_string(&entry(&observation())).unwrap()
            ),
        )
        .unwrap();

        compact_if_needed(&log).unwrap();
        let entries = read_from(&log, 10).unwrap();
        assert_eq!(entries.len(), 1, "the expired entry must be pruned");
        assert_eq!(entries[0]["requestId"], "req-1");
    }

    #[test]
    fn entry_timestamps_are_parseable_rfc3339() {
        let record = entry(&observation());
        let when = record["when"].as_str().unwrap();
        let parsed = DateTime::parse_from_rfc3339(when).unwrap();
        assert!(parsed.with_timezone(&Utc) <= Utc::now());
    }
}
