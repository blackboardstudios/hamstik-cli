// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik agent skill` — install and validate the canonical bundled Agent
//! Skill.
//!
//! `install` writes the skill compiled into this binary into an Agent Skills
//! discovery location (the current project's portable `.agents/skills/hamstik/`
//! directory by default, or the user-level portable location with `--global`).
//! It never silently overwrites a locally modified installed skill: `--force`
//! is required for replacement, and an identical re-install is idempotent.
//!
//! `check` validates a skill against this binary's command surface and
//! compatibility metadata: frontmatter version metadata, referenced command
//! paths, and referenced options must all exist. The default validates the
//! installed skill; an explicit path validates any skill file, which is what
//! CI runs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use directories::UserDirs;
use serde_json::{Value, json};

use crate::app::Session;
use crate::args::SkillCommand;
use crate::error::CliError;
use crate::exit;
use crate::fsutil;
use crate::terminal::{SGR_GREEN, SGR_RED, color_probe, paint};

use super::commands_manifest;
use super::emit_json;

/// The canonical Agent Skill compiled into this binary.
const CANONICAL_SKILL: &str = include_str!("../../../../skills/hamstik/SKILL.md");

/// The user-level Agent Skills root override (the directory that should
/// contain the portable `skills/hamstik/` location).
const SKILL_HOME_ENV: &str = "HAMSTIK_SKILL_HOME";

/// The bundled skill's parsed frontmatter, computed once.
fn bundled_frontmatter() -> &'static BTreeMap<String, String> {
    static FRONTMATTER: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    FRONTMATTER.get_or_init(|| parse_frontmatter(CANONICAL_SKILL))
}

/// The skill's declared version and CLI minimum, from the bundled skill.
fn bundled_metadata() -> (String, String) {
    let frontmatter = bundled_frontmatter();
    (
        frontmatter
            .get("skill-version")
            .cloned()
            .unwrap_or_default(),
        frontmatter
            .get("minimum-cli-version")
            .cloned()
            .unwrap_or_default(),
    )
}

/// Dispatches the `agent skill` subcommands.
pub(crate) fn run(session: &mut Session<'_>, command: &SkillCommand) -> Result<(), CliError> {
    match command {
        SkillCommand::Install { global, force } => install(session, *global, *force),
        SkillCommand::Check { path } => check(session, path.as_deref()),
    }
}

/// The user-level portable Agent Skills root (`~/.agents` unless
/// `HAMSTIK_SKILL_HOME` overrides it).
fn global_skill_home(session: &Session<'_>) -> Result<PathBuf, CliError> {
    if let Some(home) = session
        .env
        .var(SKILL_HOME_ENV)
        .filter(|home| !home.trim().is_empty())
    {
        return Ok(PathBuf::from(home.trim().to_string()));
    }
    match UserDirs::new() {
        Some(dirs) => Ok(dirs.home_dir().to_path_buf()),
        None => Err(CliError::config(format!(
            "cannot resolve the user home directory for the global Agent Skills location; set {SKILL_HOME_ENV} to the directory that should contain `skills/hamstik/`",
        ))),
    }
}

/// Installs the bundled skill into a discovery location.
fn install(session: &mut Session<'_>, global: bool, force: bool) -> Result<(), CliError> {
    let root = if global {
        global_skill_home(session)?
    } else {
        session.cwd.clone()
    };
    let destination = root
        .join(".agents")
        .join("skills")
        .join("hamstik")
        .join("SKILL.md");

    let action = if destination.is_file() {
        let existing = fs::read(&destination).map_err(|error| io_error(&error, &destination))?;
        if existing == CANONICAL_SKILL.as_bytes() {
            // Idempotent re-install: identical content needs no replacement.
            "unchanged".to_string()
        } else if force {
            "replaced".to_string()
        } else {
            return Err(CliError::general(format!(
                "refusing to replace locally modified Agent Skill at {} (pass --force to replace it explicitly)",
                destination.display(),
            )));
        }
    } else {
        "created".to_string()
    };

    fsutil::write_atomic(&destination, CANONICAL_SKILL.as_bytes())
        .map_err(|error| io_error(&error, &destination))?;

    let (version, minimum) = bundled_metadata();
    let target = if global { "global" } else { "project" };

    if session.json() {
        emit_json(
            session,
            &json!({
                "schemaVersion": 1,
                "command": "agent skill install",
                "target": target,
                "path": destination.display().to_string(),
                "skillVersion": version,
                "minimumCliVersion": minimum,
                "action": action,
            }),
        )?;
    } else if !session.out.is_quiet() {
        session
            .out
            .human(&format!(
                "installed hamstik agent skill (version {version}, requires CLI ≥ {minimum}) to {} ({action})",
                destination.display(),
            ))
            .map_err(|error| io_error(&error, &destination))?;
    }
    Ok(())
}

/// Structured result of validating one Agent Skill without rendering.
pub(crate) struct SkillValidation {
    /// The validated skill file.
    pub path: PathBuf,
    /// The skill's declared version (empty when absent).
    pub version: String,
    /// The skill's declared minimum CLI version (empty when absent).
    pub minimum: String,
    /// Whether the running CLI satisfies the declared minimum.
    pub compatible: bool,
    /// Command-surface failures (unknown paths or options).
    pub failures: Vec<String>,
    /// Referenced command paths and their flags.
    pub references: Vec<(Vec<String>, Vec<String>)>,
    /// True when metadata and every command reference validate.
    pub ok: bool,
}

/// Validates a skill file — or the installed skill when `path` is `None` —
/// against this binary's command surface and compatibility metadata.
pub(crate) fn validate(
    session: &Session<'_>,
    path: Option<&Path>,
) -> Result<SkillValidation, CliError> {
    // Resolve the skill: an explicit path wins; otherwise the first installed
    // location (project, then global).
    let resolved = match path {
        Some(explicit) => Some(explicit.to_path_buf()),
        None => installed_skill_path(session)?,
    };
    let Some(skill_path) = resolved else {
        return Err(CliError::not_found(
            "no installed Agent Skill found (run `hamstik agent skill install` first, or pass an explicit SKILL.md path)",
        ));
    };
    let text = fs::read_to_string(&skill_path).map_err(|error| io_error(&error, &skill_path))?;

    // Compatibility metadata from the skill frontmatter.
    let frontmatter = parse_frontmatter(&text);
    let version = frontmatter
        .get("skill-version")
        .cloned()
        .unwrap_or_default();
    let minimum = frontmatter
        .get("minimum-cli-version")
        .cloned()
        .unwrap_or_default();
    let compatible = minimum.is_empty()
        || (minimum.split('.').all(|part| part.parse::<u64>().is_ok())
            && semver_at_least(env!("CARGO_PKG_VERSION"), &minimum));

    // Command-surface references, validated against the live manifest.
    let manifest = commands_manifest::build()?;
    let Some(entries) = manifest["commands"].as_array() else {
        return Err(CliError::general(
            "command manifest is missing its `commands` array",
        ));
    };
    let by_path: BTreeMap<&str, &Value> = entries
        .iter()
        .filter_map(|entry| Some((entry["command"].as_str()?, entry)))
        .collect();

    let global_options: Vec<&str> = by_path
        .get("hamstik")
        .and_then(|entry| entry["options"].as_array())
        .map(|options| options.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut failures: Vec<String> = Vec::new();
    let mut references: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    for (line_number, line) in extract_command_references(&text) {
        let mut current: Vec<String> = Vec::new();
        let mut flags: Vec<String> = Vec::new();
        for token in line {
            if let Some(flag) = flag_token(&token) {
                flags.push(flag);
                continue;
            }
            // Placeholders (`<command>`, `<ITEM-KEY>`, …) cannot be validated.
            if token.starts_with('<') && token.ends_with('>') {
                continue;
            }
            // Descend while the token names a subcommand of the current node.
            let mut child = current.clone();
            child.push(token.clone());
            let mut child_key = child.join(" ");
            child_key = format!("hamstik {child_key}");
            if by_path.contains_key(child_key.as_str()) {
                current = child;
            }
            // Otherwise the token is a positional or flag value: stop
            // descending (remaining tokens can only add flags or positionals).
        }
        let key = if current.is_empty() {
            "hamstik".to_string()
        } else {
            format!("hamstik {}", current.join(" "))
        };
        let Some(entry) = by_path.get(key.as_str()) else {
            failures.push(format!(
                "unknown command path: `{key}` (skill line {line_number})"
            ));
            continue;
        };
        // Every node accepts the root's global options (`--json`, `--org`, …).
        let mut known: Vec<&str> = entry["options"]
            .as_array()
            .map(|options| options.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        known.extend(global_options.iter().copied());
        for flag in &flags {
            if !known.contains(&flag.as_str()) {
                failures.push(format!(
                    "unknown option `{flag}` for `{key}` (skill line {line_number})"
                ));
            }
        }
        references.push((current, flags));
    }

    let ok = failures.is_empty() && compatible;
    Ok(SkillValidation {
        path: skill_path,
        version,
        minimum,
        compatible,
        failures,
        references,
        ok,
    })
}

/// Validates an Agent Skill against this binary's command surface.
fn check(session: &mut Session<'_>, path: Option<&Path>) -> Result<(), CliError> {
    let validation = validate(session, path)?;
    let overall_ok = validation.ok;
    if session.json() {
        emit_json(
            session,
            &json!({
                "schemaVersion": 1,
                "command": "agent skill check",
                "path": validation.path.display().to_string(),
                "skillVersion": validation.version,
                "minimumCliVersion": validation.minimum,
                "cliVersion": env!("CARGO_PKG_VERSION"),
                "compatible": validation.compatible,
                "commandsReferenced": validation
                    .references
                    .iter()
                    .map(|(command_path, flags)| {
                        json!({
                            "command": if command_path.is_empty() {
                                Value::String("hamstik".to_string())
                            } else {
                                Value::String(command_path.join(" "))
                            },
                            "flags": flags,
                        })
                    })
                    .collect::<Vec<_>>(),
                "failures": validation.failures,
                "ok": overall_ok,
                "exitCode": if overall_ok { exit::SUCCESS } else { exit::GENERAL },
            }),
        )?;
    } else if !session.out.is_quiet() {
        let color = color_probe(
            session.env,
            session.global.color_mode(),
            session.env.stdout_is_terminal(),
        )
        .ok;
        for (marker, sgr, message) in report_lines(
            &validation.path,
            &validation.version,
            &validation.minimum,
            &validation.failures,
            validation.references.len(),
            validation.compatible,
            overall_ok,
        ) {
            let painted = paint(color, sgr, &format!("[{marker}]"));
            session
                .out
                .human(&format!("{painted} {message}"))
                .map_err(|error| io_error(&error, &validation.path))?;
        }
    }
    if !overall_ok {
        session.exit_code = exit::GENERAL;
    }
    Ok(())
}

/// The human report lines for `agent skill check`.
fn report_lines(
    skill_path: &Path,
    version: &str,
    minimum: &str,
    failures: &[String],
    reference_count: usize,
    version_ok: bool,
    overall_ok: bool,
) -> Vec<(&'static str, &'static str, String)> {
    let mut lines = vec![
        (
            "ok",
            SGR_GREEN,
            format!("skill present: {}", skill_path.display()),
        ),
        if version_ok {
            (
                "ok",
                SGR_GREEN,
                format!(
                    "skill version {} compatible with CLI {} (requires ≥ {minimum})",
                    if version.is_empty() {
                        "unversioned".to_string()
                    } else {
                        version.to_string()
                    },
                    env!("CARGO_PKG_VERSION"),
                ),
            )
        } else {
            (
                "fail",
                SGR_RED,
                format!(
                    "skill requires CLI ≥ {minimum} but this binary is {}",
                    env!("CARGO_PKG_VERSION"),
                ),
            )
        },
    ];
    for failure in failures {
        lines.push(("fail", SGR_RED, failure.clone()));
    }
    if overall_ok {
        lines.push((
            "ok",
            SGR_GREEN,
            format!(
                "all {reference_count} referenced command paths and options exist in this binary"
            ),
        ));
    } else {
        lines.push((
            "fail",
            SGR_RED,
            "skill references do not match this binary's command surface".to_string(),
        ));
    }
    lines
}

/// Normalizes a token into a long option name when it is a flag.
///
/// `--flag` and `--flag=value` shapes are recognized; universal clap
/// help/version flags are skipped. Short flags cannot be validated against
/// the long-form manifest and are ignored.
fn flag_token(token: &str) -> Option<String> {
    if token == "--help" || token == "-h" || token == "--version" || token == "-V" {
        return None;
    }
    if let Some(rest) = token.strip_prefix("--") {
        let head = match rest.split_once('=') {
            Some((head, _)) => head,
            None => rest,
        };
        return Some(format!("--{head}"));
    }
    None
}

/// Extracts `hamstik …` shell command lines from the skill's fenced code
/// blocks, honoring `\` continuations and inline ` #` comments, and yields
/// them as tokenized lines paired with their source line numbers.
fn extract_command_references(text: &str) -> Vec<(usize, Vec<String>)> {
    let mut references = Vec::new();
    let mut in_fence = false;
    let mut joined: Option<(usize, String)> = None;
    for (offset, raw) in text.lines().enumerate() {
        let line_number = offset + 1;
        if raw.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence {
            continue;
        }
        let trimmed = raw.trim();
        // Join `\`-terminated continuations into one logical line.
        let (number, logical) = match joined.take() {
            Some((number, mut logical)) => {
                logical.push(' ');
                logical.push_str(trimmed);
                (number, logical)
            }
            None => (line_number, trimmed.to_string()),
        };
        if logical.trim_end().ends_with('\\') {
            let base = logical
                .trim_end()
                .trim_end_matches('\\')
                .trim_end()
                .to_string();
            joined = Some((number, base));
            continue;
        }
        // Drop inline ` #` comments and skip non-command lines.
        let logical = match logical.find(" #") {
            Some(at) => logical[..at].trim_end().to_string(),
            None => logical,
        };
        let mut tokens = tokenize(&logical);
        let Some(binary) = tokens.first().cloned() else {
            continue;
        };
        if binary == "hamstik" || binary.ends_with("/hamstik") {
            tokens.remove(0);
        } else {
            continue;
        }
        if tokens.is_empty() {
            continue;
        }
        references.push((number, tokens));
    }
    references
}

/// Minimal shell-like tokenizer honoring single and double quotes.
fn tokenize(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for character in line.chars() {
        if let Some(open) = quote {
            if character == open {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        match character {
            '\'' | '"' => quote = Some(character),
            ' ' | '\t' => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Parses the skill's YAML-ish frontmatter into flat `key: value` pairs
/// (nested `metadata` keys flatten to their own names).
fn parse_frontmatter(text: &str) -> BTreeMap<String, String> {
    let mut frontmatter = BTreeMap::new();
    let mut in_block = false;
    for line in text.lines() {
        if line.trim() == "---" && !in_block && frontmatter.is_empty() {
            in_block = true;
            continue;
        }
        if in_block {
            if line.trim() == "---" {
                break;
            }
            if let Some((key, value)) = split_frontmatter_pair(line) {
                frontmatter.insert(key, value);
            }
        }
    }
    frontmatter
}

/// Splits one frontmatter line into a key and an unquoted value.
fn split_frontmatter_pair(line: &str) -> Option<(String, String)> {
    let (key, value) = line.split_once(':')?;
    let key = key
        .trim_end()
        .rsplit(' ')
        .next()
        .unwrap_or_default()
        .to_string();
    let value = value
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();
    if key.is_empty() || value.is_empty() {
        return None;
    }
    Some((key, value))
}

/// Compares dotted version strings: `actual >= minimum`?
fn semver_at_least(actual: &str, minimum: &str) -> bool {
    let parse = |version: &str| -> Vec<u64> {
        version
            .split('.')
            .map(|part| part.trim().parse().unwrap_or(0))
            .collect()
    };
    let mut actual = parse(actual);
    let mut minimum = parse(minimum);
    let length = actual.len().max(minimum.len());
    actual.resize(length, 0);
    minimum.resize(length, 0);
    // Equal-length numeric vectors compare numerically element by element.
    actual >= minimum
}

/// Wraps an I/O error into a CLI error naming the path.
fn io_error(error: &std::io::Error, path: &Path) -> CliError {
    if error.kind() == std::io::ErrorKind::NotFound {
        return CliError::not_found(format!("{}: {error}", path.display()));
    }
    CliError::general(format!("{}: {error}", path.display()))
}

/// The first installed skill found (project location, then global).
fn installed_skill_path(session: &Session<'_>) -> Result<Option<PathBuf>, CliError> {
    for root in [session.cwd.clone(), global_skill_home(session)?] {
        let candidate = root
            .join(".agents")
            .join("skills")
            .join("hamstik")
            .join("SKILL.md");
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_command_lines_with_continuations_and_comments() {
        let skill = r##"```bash
hamstik --json --no-input --org <ORG> org work --project <KEY> --overdue true
hamstik me \
  --json
hamstik work list --json   # reads only
```
Not a command: `hamstik work view`.
```bash
./target/release/hamstik work view <KEY>
```"##;
        let references = extract_command_references(skill);
        assert_eq!(references.len(), 4);
        assert_eq!(references[0].0, 2);
        assert_eq!(
            references[0].1,
            vec![
                "--json",
                "--no-input",
                "--org",
                "<ORG>",
                "org",
                "work",
                "--project",
                "<KEY>",
                "--overdue",
                "true"
            ]
        );
        assert_eq!(references[1].0, 3);
        assert_eq!(references[1].1, vec!["me", "--json"]);
        assert_eq!(references[2].0, 5);
        assert_eq!(references[2].1, vec!["work", "list", "--json"]);
        assert_eq!(references[3].0, 9);
        assert_eq!(references[3].1, vec!["work", "view", "<KEY>"]);
    }

    #[test]
    fn ignores_quoted_hashes_and_plain_text() {
        let references = extract_command_references(
            "# Heading\n\n```text\nhamstik squeakql validate --file QUERY.sqql\n```\n",
        );
        assert_eq!(references.len(), 1);
        assert_eq!(
            references[0].1,
            vec!["squeakql", "validate", "--file", "QUERY.sqql"]
        );
        assert_eq!(
            tokenize("comment \"a # b\" 'c # d'"),
            vec!["comment", "a # b", "c # d"]
        );
    }

    #[test]
    fn parses_frontmatter_and_flattens_metadata() {
        let frontmatter = parse_frontmatter(
            "---\nname: hamstik\nskill-version: 0.2.0\nmetadata:\n  minimum-cli-version: \"0.1.0\"\n---\nbody",
        );
        assert_eq!(frontmatter.get("skill-version"), Some(&"0.2.0".to_string()));
        assert_eq!(
            frontmatter.get("minimum-cli-version"),
            Some(&"0.1.0".to_string())
        );
        assert_eq!(frontmatter.get("name"), Some(&"hamstik".to_string()));
    }

    #[test]
    fn compares_versions_segment_wise() {
        assert!(semver_at_least("0.1.2", "0.1.0"));
        assert!(semver_at_least("0.1.0", "0.1.0"));
        assert!(semver_at_least("0.10.0", "0.9.0"));
        assert!(semver_at_least("1.0.0", "0.1.0"));
        assert!(!semver_at_least("0.1.0", "0.2.0"));
        assert!(!semver_at_least("0.1.0", "1.0.0"));
    }

    #[test]
    fn normalizes_flags_and_skips_universals() {
        assert_eq!(flag_token("--org=ACME"), Some("--org".to_string()));
        assert_eq!(flag_token("--no-input"), Some("--no-input".to_string()));
        assert_eq!(flag_token("-h"), None);
        assert_eq!(flag_token("--help"), None);
        assert_eq!(flag_token("-V"), None);
        assert_eq!(flag_token("work"), None);
        assert_eq!(flag_token("<KEY>"), None);
    }

    #[test]
    fn bundled_skill_carries_compatible_metadata() {
        assert!(!CANONICAL_SKILL.is_empty());
        assert!(parse_frontmatter(CANONICAL_SKILL).contains_key("skill-version"));
    }
}
