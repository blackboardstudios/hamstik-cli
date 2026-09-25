// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik agent validate` — verify the local agent harness.
//!
//! The command is deliberately offline-first and distinct from `hamstik
//! doctor`: it focuses on the agent harness — the installed skill's metadata
//! against the running CLI version, credential-shaped content in the config
//! directory, and bundled OpenAPI snapshot freshness — rather than
//! network/diagnostic health. Credential-shaped values are reported by
//! location and kind and are never printed.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::app::Session;
use crate::args::ValidateArgs;
use crate::error::CliError;
use crate::exit;
use crate::terminal::{SGR_GREEN, SGR_RED, SGR_YELLOW, color_probe, paint};

use super::agent_skill;
use super::emit_json;

/// The checked-in OpenAPI snapshot compared against the live contract.
const BUNDLED_OPENAPI: &str = include_str!("../../../../openapi/hamstik-v1.json");

/// Maximum number of config-directory files inspected.
const MAX_SCAN_FILES: usize = 256;
/// Maximum size of one inspected config file (1 MiB).
const MAX_SCAN_BYTES: u64 = 1024 * 1024;
/// Maximum config-directory recursion depth.
const MAX_SCAN_DEPTH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Pass,
    Fail,
    Skipped,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Skipped => "skipped",
        }
    }
}

/// One harness check.
struct Check {
    id: &'static str,
    name: &'static str,
    status: CheckStatus,
    detail: String,
    remediation: Option<String>,
}

impl Check {
    fn pass(id: &'static str, name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            id,
            name,
            status: CheckStatus::Pass,
            detail: detail.into(),
            remediation: None,
        }
    }

    fn fail(id: &'static str, name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            id,
            name,
            status: CheckStatus::Fail,
            detail: detail.into(),
            remediation: None,
        }
    }

    fn skipped(id: &'static str, name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            id,
            name,
            status: CheckStatus::Skipped,
            detail: detail.into(),
            remediation: None,
        }
    }

    fn with_remediation(mut self, remediation: impl Into<String>) -> Self {
        self.remediation = Some(remediation.into());
        self
    }
}

/// Runs `hamstik agent validate`.
pub async fn run(session: &mut Session<'_>, args: &ValidateArgs) -> Result<(), CliError> {
    let mut checks = skill_checks(session, args.path.as_deref());
    checks.push(credential_check(session));
    checks.push(snapshot_check(session, args.offline).await);
    render(session, checks)
}

/// The skill metadata and command-surface checks.
fn skill_checks(session: &Session<'_>, path: Option<&Path>) -> Vec<Check> {
    let validation = match agent_skill::validate(session, path) {
        Ok(validation) => validation,
        Err(error) => {
            return vec![
                Check::fail("skill.metadata", "skill metadata", error.message).with_remediation(
                    "run `hamstik agent skill install` to install the canonical skill, or pass an explicit SKILL.md path",
                ),
            ];
        }
    };

    let metadata = if validation.compatible {
        Check::pass(
            "skill.metadata",
            "skill metadata",
            format!(
                "skill version {} is compatible with CLI {} (requires ≥ {})",
                if validation.version.is_empty() {
                    "unversioned".to_string()
                } else {
                    validation.version.clone()
                },
                env!("CARGO_PKG_VERSION"),
                if validation.minimum.is_empty() {
                    "any".to_string()
                } else {
                    validation.minimum.clone()
                },
            ),
        )
    } else {
        Check::fail(
            "skill.metadata",
            "skill metadata",
            format!(
                "installed skill requires CLI ≥ {} but this binary is {} (outdated skill or CLI)",
                validation.minimum,
                env!("CARGO_PKG_VERSION"),
            ),
        )
        .with_remediation(
            "update the CLI, or reinstall the matching skill with `hamstik agent skill install --force`",
        )
    };

    let commands = if validation.failures.is_empty() {
        Check::pass(
            "skill.commands",
            "skill commands",
            format!(
                "all {} referenced command paths and options exist in this binary",
                validation.references.len()
            ),
        )
    } else {
        Check::fail(
            "skill.commands",
            "skill commands",
            validation.failures.join("; "),
        )
        .with_remediation(
            "reinstall the canonical skill with `hamstik agent skill install --force`",
        )
    };

    vec![metadata, commands]
}

/// The credential-shaped-content scan of the config directory.
fn credential_check(session: &Session<'_>) -> Check {
    let config_dir = session
        .config
        .path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let findings = scan_credentials(&config_dir);
    if findings.is_empty() {
        return Check::pass(
            "config.credentials",
            "config credentials",
            format!(
                "no credential-shaped content found under {}",
                config_dir.display()
            ),
        );
    }
    let locations = findings
        .iter()
        .map(|finding| format!("{}:{} ({})", finding.path, finding.line, finding.kind))
        .collect::<Vec<_>>()
        .join(", ");
    Check::fail(
        "config.credentials",
        "config credentials",
        format!(
            "found {} credential-shaped value(s) at {locations}; values are never displayed",
            findings.len(),
        ),
    )
    .with_remediation(
        "remove the value and store credentials with `hamstik auth login` or the HAMSTIK_TOKEN environment variable",
    )
}

/// The OpenAPI snapshot-freshness check (skipped offline).
async fn snapshot_check(session: &Session<'_>, offline: bool) -> Check {
    if offline {
        return Check::skipped(
            "openapi.freshness",
            "OpenAPI snapshot",
            "skipped by --offline; the bundled snapshot was not compared to the live contract",
        );
    }
    // Host/context failures must not abort the harness report, and their
    // messages may echo config-file content, so only a generic reason is kept.
    let selection = match session.selection() {
        Ok(selection) => selection,
        Err(_) => {
            return Check::skipped(
                "openapi.freshness",
                "OpenAPI snapshot",
                "skipped: host context unavailable",
            );
        }
    };
    let api = match session.public_api(&selection) {
        Ok(api) => api,
        Err(_) => {
            return Check::skipped(
                "openapi.freshness",
                "OpenAPI snapshot",
                "skipped: live contract client unavailable",
            );
        }
    };
    match api.get_open_api().await {
        Ok(response) => compare_snapshot(&response.value),
        Err(error) => Check::skipped(
            "openapi.freshness",
            "OpenAPI snapshot",
            format!(
                "skipped: live contract unavailable ({}); the bundled snapshot was not compared",
                error.network_stage().as_str(),
            ),
        ),
    }
}

/// Compares the bundled snapshot with the live contract.
fn compare_snapshot(live: &Value) -> Check {
    let bundled: Value = match serde_json::from_str(BUNDLED_OPENAPI) {
        Ok(value) => value,
        Err(error) => {
            return Check::fail(
                "openapi.freshness",
                "OpenAPI snapshot",
                format!("the bundled OpenAPI snapshot is invalid: {error}"),
            )
            .with_remediation("this is a bug; report it with the CLI version");
        }
    };
    if bundled == *live {
        return Check::pass(
            "openapi.freshness",
            "OpenAPI snapshot",
            "bundled OpenAPI snapshot matches the live Public API contract",
        );
    }
    let bundled_operations = operation_ids(&bundled);
    let live_operations = operation_ids(live);
    let added = live_operations.difference(&bundled_operations).count();
    let removed = bundled_operations.difference(&live_operations).count();
    Check::fail(
        "openapi.freshness",
        "OpenAPI snapshot",
        format!(
            "bundled OpenAPI snapshot (version {}) differs from the live contract (version {}): {} operation(s) added, {} removed",
            document_version(&bundled),
            document_version(live),
            added,
            removed,
        ),
    )
    .with_remediation(
        "refresh the snapshot with `scripts/update-openapi.sh --update` and reclassify openapi/api-parity.json",
    )
}

/// The `operationId` set of an OpenAPI document.
fn operation_ids(document: &Value) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    if let Some(paths) = document.pointer("/paths").and_then(Value::as_object) {
        for item in paths.values() {
            if let Some(methods) = item.as_object() {
                for operation in methods.values() {
                    if let Some(id) = operation.get("operationId").and_then(Value::as_str) {
                        ids.insert(id.to_string());
                    }
                }
            }
        }
    }
    ids
}

/// The `info.version` of an OpenAPI document.
fn document_version(document: &Value) -> &str {
    document
        .pointer("/info/version")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

/// Renders the harness report and sets the exit code.
fn render(session: &mut Session<'_>, checks: Vec<Check>) -> Result<(), CliError> {
    let passed = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Pass)
        .count();
    let failed = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .count();
    let skipped = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Skipped)
        .count();
    let exit_code = if failed == 0 {
        exit::SUCCESS
    } else {
        exit::GENERAL
    };
    let ok = exit_code == exit::SUCCESS;

    if session.json() {
        let items: Vec<Value> = checks
            .iter()
            .map(|check| {
                let mut value = json!({
                    "id": check.id,
                    "name": check.name,
                    "status": check.status.as_str(),
                    "ok": check.status == CheckStatus::Pass,
                    "detail": check.detail,
                });
                if let Some(remediation) = &check.remediation {
                    value["remediation"] = Value::String(remediation.clone());
                }
                value
            })
            .collect();
        emit_json(
            session,
            &json!({
                "schemaVersion": 1,
                "command": "agent validate",
                "checks": items,
                "summary": {"pass": passed, "fail": failed, "skipped": skipped},
                "ok": ok,
                "exitCode": exit_code,
            }),
        )?;
    } else if !session.out.is_quiet() {
        let color = color_probe(
            session.env,
            session.global.no_color,
            session.env.stdout_is_terminal(),
        )
        .ok;
        for check in &checks {
            let (marker, sgr) = match check.status {
                CheckStatus::Pass => ("ok", SGR_GREEN),
                CheckStatus::Fail => ("FAIL", SGR_RED),
                CheckStatus::Skipped => ("skip", SGR_YELLOW),
            };
            let marker = paint(color, sgr, marker);
            session
                .out
                .line(&format!(
                    "[{marker:<4}] {:<20} {}",
                    check.name, check.detail
                ))
                .map_err(CliError::general)?;
            if let Some(remediation) = &check.remediation {
                session
                    .out
                    .line(&format!("       hint: {remediation}"))
                    .map_err(CliError::general)?;
            }
        }
        session
            .out
            .line(&format!(
                "summary: {passed} passed, {failed} failed, {skipped} skipped"
            ))
            .map_err(CliError::general)?;
        session
            .out
            .line(if ok {
                "harness ready."
            } else {
                "harness not ready."
            })
            .map_err(CliError::general)?;
    }

    session.exit_code = exit_code;
    Ok(())
}

/// A credential-shaped value located without retaining the value itself.
struct CredentialFinding {
    path: String,
    line: usize,
    kind: &'static str,
}

/// Scans a config directory for credential-shaped content.
fn scan_credentials(root: &Path) -> Vec<CredentialFinding> {
    let mut findings = Vec::new();
    let mut files = 0usize;
    walk_config(root, root, 0, &mut files, &mut findings);
    findings
}

/// Recursively visits config files with bounded depth and count.
fn walk_config(
    root: &Path,
    directory: &Path,
    depth: usize,
    files: &mut usize,
    findings: &mut Vec<CredentialFinding>,
) {
    if depth > MAX_SCAN_DEPTH || *files >= MAX_SCAN_FILES {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        if *files >= MAX_SCAN_FILES {
            break;
        }
        let path = entry.path();
        // `DirEntry::metadata` does not traverse symlinks, so links are skipped.
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            walk_config(root, &path, depth + 1, files, findings);
            continue;
        }
        if !metadata.is_file() || metadata.len() > MAX_SCAN_BYTES {
            continue;
        }
        *files += 1;
        scan_file(root, &path, findings);
    }
}

/// Scans one text file line by line, recording only location and kind.
fn scan_file(root: &Path, path: &Path, findings: &mut Vec<CredentialFinding>) {
    let Ok(bytes) = fs::read(path) else {
        return;
    };
    // Skip binary files (a NUL byte in the leading window).
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return;
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return;
    };
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();
    for (index, line) in text.lines().enumerate() {
        if let Some(kind) = credential_shape(line) {
            findings.push(CredentialFinding {
                path: relative.clone(),
                line: index + 1,
                kind,
            });
        }
    }
}

/// Classifies one line as credential-shaped, if it is.
fn credential_shape(line: &str) -> Option<&'static str> {
    let lowered = line.to_ascii_lowercase();
    if lowered.contains("-----begin") && lowered.contains("private key-----") {
        return Some("private-key");
    }
    if contains_bearer_token(line) {
        return Some("bearer-token");
    }
    if contains_jwt(line) {
        return Some("jwt");
    }
    if contains_prefixed_run(line, &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"], 20) {
        return Some("github-token");
    }
    if contains_slack_token(line) {
        return Some("slack-token");
    }
    if contains_aws_access_key(line) {
        return Some("aws-access-key");
    }
    if contains_assigned_secret(&lowered) {
        return Some("assigned-secret");
    }
    None
}

/// True when the line carries a `Bearer <token>` authorization value.
fn contains_bearer_token(line: &str) -> bool {
    let lowered = line.to_ascii_lowercase();
    let mut search = 0;
    while let Some(relative) = lowered[search..].find("bearer") {
        let start = search + relative;
        let after = start + "bearer".len();
        let rest = line[after..].trim_start();
        // Require at least one separator before the value.
        if rest.len() < line[after..].len() {
            let value = rest
                .split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ';' || c == ',')
                .next()
                .unwrap_or_default();
            if value.len() >= 8
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
            {
                return true;
            }
        }
        search = after;
    }
    false
}

/// True when the line contains a JWT-shaped `header.payload.signature` value.
fn contains_jwt(line: &str) -> bool {
    let mut search = 0;
    while let Some(relative) = line[search..].find("eyJ") {
        let start = search + relative;
        let end = line[start..]
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
            .map(|offset| start + offset)
            .unwrap_or(line.len());
        let candidate = &line[start..end];
        let parts: Vec<&str> = candidate.split('.').collect();
        if parts.len() == 3 && parts.iter().all(|part| part.len() >= 8) {
            return true;
        }
        search = end.max(start + 3);
    }
    false
}

/// True when `line` contains `prefix` followed by at least `minimum` alphanumerics.
fn contains_prefixed_run(line: &str, prefixes: &[&str], minimum: usize) -> bool {
    for prefix in prefixes {
        let mut search = 0;
        while let Some(relative) = line[search..].find(prefix) {
            let start = search + relative;
            let run = line[start + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .count();
            if run >= minimum {
                return true;
            }
            search = start + prefix.len();
        }
    }
    false
}

/// True when the line contains a Slack `xox?` token.
fn contains_slack_token(line: &str) -> bool {
    let mut search = 0;
    while let Some(relative) = line[search..].find("xox") {
        let start = search + relative;
        let mut chars = line[start + 3..].chars();
        let kind = chars.next();
        let dash = chars.next();
        if matches!(kind, Some('b' | 'a' | 'p' | 'r' | 's')) && dash == Some('-') {
            let run = chars
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .count();
            if run >= 10 {
                return true;
            }
        }
        search = start + 3;
    }
    false
}

/// True when the line contains an AWS access-key id.
fn contains_aws_access_key(line: &str) -> bool {
    let mut search = 0;
    while let Some(relative) = line[search..].find("AKIA") {
        let start = search + relative;
        let run = line[start + 4..]
            .chars()
            .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            .count();
        if run >= 16 {
            return true;
        }
        search = start + 4;
    }
    false
}

/// True when the line assigns a non-placeholder value to a secret-shaped key.
fn contains_assigned_secret(lowered: &str) -> bool {
    let content = lowered.split('#').next().unwrap_or(lowered);
    for (index, character) in content.char_indices() {
        if character != '=' && character != ':' {
            continue;
        }
        let key = content[..index]
            .trim_end()
            .rsplit(|c: char| !is_key_char(c))
            .next()
            .unwrap_or_default();
        if !is_secret_key(key) {
            continue;
        }
        let value = content[index + 1..]
            .trim_start_matches([' ', '\t', '"', '\''])
            .trim_end_matches([' ', '\t', '"', '\'', ',', ']', '}', ';']);
        if !value.is_empty() && !is_placeholder(value) {
            return true;
        }
    }
    false
}

/// Characters that may appear in an assignment key.
fn is_key_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '_' || character == '-'
}

/// True when a key name names a credential.
fn is_secret_key(key: &str) -> bool {
    const EXACT: &[&str] = &[
        "token",
        "password",
        "secret",
        "authorization",
        "apikey",
        "pat",
        "credential",
        "credentials",
    ];
    const SUFFIXES: &[&str] = &["_token", "_password", "_secret", "_key", "_pat"];
    EXACT.contains(&key) || SUFFIXES.iter().any(|suffix| key.ends_with(suffix))
}

/// True when a value is an obvious placeholder rather than a real credential.
fn is_placeholder(value: &str) -> bool {
    let value = value.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
    if value.is_empty() {
        return true;
    }
    if value.starts_with('<') && value.ends_with('>') {
        return true;
    }
    if value.starts_with('$') {
        return true;
    }
    matches!(
        value.as_str(),
        "null"
            | "true"
            | "false"
            | "none"
            | "changeme"
            | "change-me"
            | "example"
            | "placeholder"
            | "redacted"
            | "..."
            | "xxx"
            | "your-token-here"
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn detects_known_credential_shapes() {
        assert_eq!(
            credential_shape("-----BEGIN RSA PRIVATE KEY-----"),
            Some("private-key")
        );
        assert_eq!(
            credential_shape("Authorization: Bearer abcdefgh1234"),
            Some("bearer-token")
        );
        assert_eq!(
            credential_shape(
                "token = eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"
            ),
            Some("jwt")
        );
        assert_eq!(
            credential_shape("key = ghp_abcdefghijklmnopqrstuvwxyz0123"),
            Some("github-token")
        );
        assert_eq!(
            credential_shape("slack = xoxb-1234567890abcdef"),
            Some("slack-token")
        );
        assert_eq!(
            credential_shape("aws = AKIAIOSFODNN7EXAMPLE"),
            Some("aws-access-key")
        );
        assert_eq!(
            credential_shape("token = \"a-real-secret-value\""),
            Some("assigned-secret")
        );
        assert_eq!(
            credential_shape("client_secret = \"s3cr3t-value\""),
            Some("assigned-secret")
        );
    }

    #[test]
    fn ignores_placeholders_and_ordinary_config() {
        assert_eq!(credential_shape("token = \"\""), None);
        assert_eq!(credential_shape("token = \"<ORG>\""), None);
        assert_eq!(credential_shape("token = \"${HAMSTIK_TOKEN}\""), None);
        assert_eq!(credential_shape("token = \"changeme\""), None);
        assert_eq!(credential_shape("editor = \"vim\""), None);
        assert_eq!(credential_shape("output = \"json\""), None);
        assert_eq!(credential_shape("git_branch_template = \"{key}\""), None);
        assert_eq!(credential_shape("project = \"token-store\""), None);
        assert_eq!(credential_shape("# token = abcdefghijkl"), None);
        assert_eq!(credential_shape("monkey = \"banana\""), None);
    }

    #[test]
    fn scans_a_directory_without_printing_values() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "version = 2\n[settings]\neditor = \"vim\"\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("queries.toml"),
            "version = 1\ntoken = \"super-secret-value\"\n",
        )
        .unwrap();
        let findings = scan_credentials(dir.path());
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path, "queries.toml");
        assert_eq!(findings[0].line, 2);
        assert_eq!(findings[0].kind, "assigned-secret");
    }

    #[test]
    fn clean_directory_has_no_findings() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(
            dir.path().join("config.toml"),
            "version = 2\n[settings]\neditor = \"code\"\n",
        )
        .unwrap();
        assert!(scan_credentials(dir.path()).is_empty());
    }
}
