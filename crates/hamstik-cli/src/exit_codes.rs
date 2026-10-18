// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! The stable exit-code reference (CLI-73).
//!
//! One source of truth for the machine-readable exit-code table: every entry
//! is built directly from the [`crate::exit`] constants that error rendering
//! maps onto, so the table cannot drift from actual behavior. The human table
//! (`hamstik commands --exit-codes`) and the `exitCodes` block in the
//! `commands --json` manifest both render from this module.

use serde_json::{Value, json};

use crate::exit;

/// Schema version for the machine-readable exit-code document.
///
/// Bump only on a breaking change to the document layout; new entries and
/// text edits are additive and keep this version.
pub(crate) const EXIT_CODES_VERSION: u64 = 1;

/// One stable process exit code with its meaning and remediation hint.
pub(crate) struct ExitCodeEntry {
    /// The process exit code (SPEC §52).
    pub code: i32,
    /// Stable snake_case name for scripts that match on the name.
    pub name: &'static str,
    /// One-line meaning, aligned with the documented table.
    pub meaning: &'static str,
    /// Suggested remediation for agents and scripts.
    pub remediation: &'static str,
}

/// The full stable exit-code table, in code order.
///
/// The `code` values reference [`crate::exit`] constants directly, so a
/// renumbering in `exit.rs` propagates here (and the contract tests fail
/// until the documented table is deliberately reviewed).
pub(crate) fn table() -> Vec<ExitCodeEntry> {
    vec![
        ExitCodeEntry {
            code: exit::SUCCESS,
            name: "success",
            meaning: "Command completed successfully.",
            remediation: "No action needed.",
        },
        ExitCodeEntry {
            code: exit::GENERAL,
            name: "general",
            meaning: "General/uncategorized failure.",
            remediation: "Inspect the structured error on stderr and correct the cause before retrying.",
        },
        ExitCodeEntry {
            code: exit::USAGE,
            name: "usage",
            meaning: "Usage error or invalid local command input.",
            remediation: "Check `hamstik <command> --help`; the error names the offending argument or value.",
        },
        ExitCodeEntry {
            code: exit::AUTHENTICATION,
            name: "authentication",
            meaning: "Authentication failure (missing/invalid credential).",
            remediation: "Refresh the credential with `hamstik auth login` or a new HAMSTIK_TOKEN; never retry blindly.",
        },
        ExitCodeEntry {
            code: exit::AUTHORIZATION,
            name: "authorization",
            meaning: "Authorization or insufficient scope.",
            remediation: "Request the missing scope or permission; do not switch identities silently.",
        },
        ExitCodeEntry {
            code: exit::NOT_FOUND,
            name: "not_found",
            meaning: "Resource not found.",
            remediation: "Verify the resolved Organization/Project context and the identifier before concluding the resource does not exist.",
        },
        ExitCodeEntry {
            code: exit::CONFLICT,
            name: "conflict",
            meaning: "Conflict / concurrency / idempotency conflict.",
            remediation: "Re-read the resource, reconcile the intended change, and retry only the merged change; never blind-retry with --force.",
        },
        ExitCodeEntry {
            code: exit::RATE_LIMITED,
            name: "rate_limited",
            meaning: "Rate limited.",
            remediation: "Honor the CLI's retry policy and the server's Retry-After; do not tight-loop.",
        },
        ExitCodeEntry {
            code: exit::NETWORK,
            name: "network",
            meaning: "Network / transport failure.",
            remediation: "Check connectivity, proxy, and host; the structured error names the earliest failing stage.",
        },
        ExitCodeEntry {
            code: exit::SERVER,
            name: "server",
            meaning: "Server / internal API failure.",
            remediation: "Retry after a delay if the failure looks transient; correlate server-side diagnostics by request id (`hamstik replay`).",
        },
        ExitCodeEntry {
            code: exit::CONFIGURATION,
            name: "configuration",
            meaning: "Local configuration or credential-store failure.",
            remediation: "Run `hamstik doctor` and repair the local configuration or OS credential store.",
        },
    ]
}

/// The machine-readable exit-code document (`exitCodesVersion: 1`).
pub(crate) fn document() -> Value {
    json!({
        "exitCodesVersion": EXIT_CODES_VERSION,
        "exitCodes": entries_json(),
    })
}

/// The `exitCodes` array alone, for embedding in the command manifest.
pub(crate) fn entries_json() -> Value {
    table()
        .into_iter()
        .map(|entry| {
            json!({
                "code": entry.code,
                "name": entry.name,
                "meaning": entry.meaning,
                "remediation": entry.remediation,
            })
        })
        .collect::<Vec<_>>()
        .into()
}

/// Human-readable table rows: one per exit code, in code order.
pub(crate) fn rows() -> Vec<Vec<String>> {
    table()
        .into_iter()
        .map(|entry| {
            vec![
                entry.code.to_string(),
                entry.name.to_string(),
                entry.meaning.to_string(),
                entry.remediation.to_string(),
            ]
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_every_stable_exit_code_in_order() {
        let entries = table();
        assert_eq!(entries.len(), 11);
        for (index, entry) in entries.iter().enumerate() {
            assert_eq!(entry.code, index as i32, "codes are contiguous from 0");
            assert!(!entry.name.is_empty());
            assert!(!entry.meaning.is_empty());
            assert!(!entry.remediation.is_empty());
        }
        assert_eq!(entries[0].code, crate::exit::SUCCESS);
        assert_eq!(entries[10].code, crate::exit::CONFIGURATION);
    }

    #[test]
    fn document_is_versioned_and_matches_the_table() {
        let document = document();
        assert_eq!(document["exitCodesVersion"], json!(EXIT_CODES_VERSION));
        let codes = document["exitCodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["code"].as_i64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(codes, (0..=10).collect::<Vec<_>>());
    }
}
