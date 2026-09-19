// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Append-only audit log for state-changing CLI operations.
//!
//! Each successful mutation appends exactly one JSON line to the per-user audit
//! log. The record is deliberately narrow — what ran, when, what it targeted,
//! the revision before/after when the CLI knows them, and the server-assigned
//! request id:
//!
//! ```json
//! {"when":"2026-01-01T00:00:00Z","command":"work.edit","target":"HAM-1",
//!  "revisionBefore":7,"revisionAfter":8,"requestId":"0f2c…"}
//! ```
//!
//! `revisionBefore`/`revisionAfter` are `null` for creates and for mutations the
//! Public API does not revision-guard (or return a revision for). No request or
//! response bodies, titles, descriptions, tokens, or credentials are ever
//! written; only identifiers and the command path are.
//!
//! The server-side audit record stays authoritative; this log is a local,
//! best-effort convenience. If the log cannot be written the CLI warns but does
//! **not** fail the mutation.
//!
//! Location and rotation are documented in the user documentation
//! (`README.md` — "Local mutation audit log") and reported by `hamstik doctor`.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde_json::json;

use crate::config::ConfigStore;
use crate::output::Output;

/// The default audit log file name.
const FILE_NAME: &str = "audit.log";

/// Environment override for the audit log path.
///
/// Mirrors `HAMSTIK_CONFIG`: it exists so the location can be pinned in tests,
/// containers, and automation without inventing a second configuration
/// mechanism. An empty value is ignored.
const PATH_ENV: &str = "HAMSTIK_AUDIT_LOG";

/// Returns the effective audit log path for the current platform.
///
/// The log lives in the per-user state directory (it is state, not data the
/// user would migrate):
///   - Linux:  `$XDG_STATE_HOME/hamstik/audit.log`, default `~/.local/state/hamstik/audit.log`
///   - macOS:  `~/Library/Application Support/hamstik/audit.log`
///   - Windows: `%LOCALAPPDATA%\hamstik\audit.log`
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

/// Whether the audit log is enabled for this run (default: enabled).
///
/// Opt-out is `audit_log = false` under `[settings]` in the configuration file
/// (`hamstik config set audit_log false`). An unreadable configuration never
/// silences the audit log: losing the trail is worse than recording when the
/// setting could not be confirmed.
#[must_use]
pub fn enabled(config: &ConfigStore) -> bool {
    match config.load() {
        Ok(cfg) => cfg.settings.and_then(|s| s.audit_log).unwrap_or(true),
        Err(_) => true,
    }
}

/// Appends one audit entry for a completed mutation.
///
/// Any I/O error is swallowed after emitting a warning; a broken audit log
/// must not block the user's workflow.
pub fn record(
    config: &ConfigStore,
    out: &mut Output,
    command: &str,
    target: &str,
    request_id: Option<&str>,
) {
    record_with_revisions(config, out, command, target, None, None, request_id);
}

/// Appends one audit entry, including the revision the CLI saw before the
/// mutation and the revision reported after it (`None` when unknown).
pub fn record_with_revisions(
    config: &ConfigStore,
    out: &mut Output,
    command: &str,
    target: &str,
    revision_before: Option<i64>,
    revision_after: Option<i64>,
    request_id: Option<&str>,
) {
    if !enabled(config) {
        return;
    }

    let Some(log_path) = path() else {
        return;
    };

    let record = entry(command, target, revision_before, revision_after, request_id);
    if let Err(err) = write_entry(&log_path, &record) {
        out.warn(&format!(
            "audit log write failed ({}): {err}; continuing without audit",
            log_path.display()
        ));
    }
}

/// Builds one audit record.
///
/// Every value is written as a JSON string/number, so nothing can smuggle a
/// body or credential into a record: only the arguments below appear.
fn entry(
    command: &str,
    target: &str,
    revision_before: Option<i64>,
    revision_after: Option<i64>,
    request_id: Option<&str>,
) -> serde_json::Value {
    json!({
        "when": crate::commands::timestamp_rfc3339(),
        "command": command,
        "target": target,
        "revisionBefore": revision_before,
        "revisionAfter": revision_after,
        "requestId": request_id,
    })
}

fn write_entry(path: &Path, entry: &serde_json::Value) -> Result<(), String> {
    use std::io::Write as _;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;

    // The record is non-secret in the credential sense but still describes
    // private work; keep it owner-only like the config and context files.
    let _ = crate::fsutil::restrict_permissions(path);

    let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    writeln!(file, "{line}").map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use tempfile::TempDir;

    #[test]
    fn path_returns_some_when_dirs_available() {
        // On a normal developer/CI machine a home directory resolves.
        assert!(path().is_some());
    }

    #[test]
    fn entry_contains_only_the_documented_fields() {
        let record = entry("work.edit", "HAM-1", Some(7), Some(8), Some("req-1"));
        let keys: BTreeSet<&str> = record
            .as_object()
            .map(|map| map.keys().map(String::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            keys,
            BTreeSet::from([
                "command",
                "requestId",
                "revisionAfter",
                "revisionBefore",
                "target",
                "when"
            ])
        );
        assert_eq!(record["command"], "work.edit");
        assert_eq!(record["target"], "HAM-1");
        assert_eq!(record["revisionBefore"], 7);
        assert_eq!(record["revisionAfter"], 8);
        assert_eq!(record["requestId"], "req-1");
    }

    #[test]
    fn entry_omits_unknown_revisions_and_request_ids_as_null() {
        let record = entry("work.create", "HAM-2", None, None, None);
        assert!(record["revisionBefore"].is_null());
        assert!(record["revisionAfter"].is_null());
        assert!(record["requestId"].is_null());
        assert!(record["when"].is_string());
    }

    #[test]
    fn entry_never_carries_secrets_or_bodies() {
        // `target` is the only free-form caller input; a value that looks like a
        // credential is still recorded verbatim and never expanded, and no field
        // other than the documented six exists.
        let record = entry("work.edit", "HAM-1", None, None, Some("req"));
        let serialized = record.to_string();
        for forbidden in [
            "Authorization",
            "Bearer",
            "token",
            "password",
            "body",
            "title",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "unexpected {forbidden:?} in audit record: {serialized}"
            );
        }
    }

    fn write_two_entries(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let e1 = entry("work.create", "HAM-1", None, Some(1), Some("r1"));
        let e2 = entry("work.edit", "HAM-1", Some(1), Some(2), Some("r2"));
        write_entry(path, &e1)?;
        write_entry(path, &e2)?;
        Ok(())
    }

    #[test]
    fn write_entry_creates_parent_dirs() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new()?;
        let log_path = dir.path().join("nested").join("audit.log");
        write_entry(
            &log_path,
            &entry("work.edit", "HAM-1", Some(1), Some(2), Some("req-1")),
        )?;
        assert!(log_path.exists());
        let content = std::fs::read_to_string(&log_path)?;
        let parsed: serde_json::Value = serde_json::from_str(content.trim_end())?;
        assert_eq!(parsed["command"], "work.edit");
        assert_eq!(parsed["target"], "HAM-1");
        assert_eq!(parsed["requestId"], "req-1");
        Ok(())
    }

    #[test]
    fn write_entry_appends_one_line_each() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new()?;
        let log_path = dir.path().join("audit.log");
        write_two_entries(&log_path)?;
        let content = std::fs::read_to_string(&log_path)?;
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in &lines {
            let parsed: serde_json::Value = serde_json::from_str(line)?;
            assert!(parsed["when"].is_string());
        }
        let first: serde_json::Value = serde_json::from_str(lines[0])?;
        let second: serde_json::Value = serde_json::from_str(lines[1])?;
        assert_eq!(first["command"], "work.create");
        assert_eq!(second["command"], "work.edit");
        Ok(())
    }

    #[test]
    fn enabled_defaults_to_true() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new()?;
        let store = ConfigStore::new(dir.path().join("config.toml"));
        assert!(enabled(&store));

        std::fs::write(
            dir.path().join("config.toml"),
            "version = 2\n[settings]\naudit_log = true\n",
        )?;
        assert!(enabled(&store));
        Ok(())
    }

    #[test]
    fn enabled_is_false_only_when_explicitly_disabled() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new()?;
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "version = 2\n[settings]\naudit_log = false\n")?;
        let store = ConfigStore::new(config_path);
        assert!(!enabled(&store));
        Ok(())
    }

    #[test]
    fn enabled_survives_an_unreadable_config() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "this is not [toml").unwrap();
        let store = ConfigStore::new(config_path);
        assert!(enabled(&store));
    }
}
