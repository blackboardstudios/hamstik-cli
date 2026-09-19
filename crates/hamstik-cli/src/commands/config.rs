// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik config` — inspect and modify global CLI configuration.

use crate::app::Session;
use crate::args::ConfigCommand;
use crate::config::{ConfigFile, ConfigSettings};
use crate::error::CliError;

/// Maximum accepted length for one stored value.
///
/// Values land in a shared file that also holds every profile, so a single
/// absurd write must not be able to bloat it.
const MAX_VALUE_LEN: usize = 1_024;

/// Output modes the CLI implements; any other spelling is a mistake caught at
/// write time rather than a surprise at render time.
const OUTPUT_MODES: &[&str] = &["human", "json", "jsonl", "tsv", "quiet"];

/// Supported configuration keys and their descriptions.
const VALID_KEYS: &[&str] = &[
    "profile",
    "organization",
    "project",
    "editor",
    "pager",
    "output",
    "git_branch_template",
    "audit_log",
];

/// Runs the selected `config` subcommand.
pub async fn run(
    session: &mut Session<'_>,
    args: &crate::args::ConfigArgs,
) -> Result<(), CliError> {
    match &args.command {
        ConfigCommand::Path => cmd_path(session).await,
        ConfigCommand::List => cmd_list(session).await,
        ConfigCommand::Get { key } => cmd_get(session, key).await,
        ConfigCommand::Set { key, value } => cmd_set(session, key, value).await,
        ConfigCommand::Unset { key } => cmd_unset(session, key).await,
    }
}

async fn cmd_path(session: &mut Session<'_>) -> Result<(), CliError> {
    let path = session.config.path().display().to_string();
    if session.json() {
        emit_json(session, &serde_json::json!({"path": path}))?;
    } else {
        session.out.line(&path).map_err(CliError::general)?;
    }
    Ok(())
}

async fn cmd_list(session: &mut Session<'_>) -> Result<(), CliError> {
    let config = session.config.load()?;
    let mut items: Vec<(&str, &str)> = Vec::new();

    if let Some(settings) = &config.settings {
        if let Some(editor) = &settings.editor {
            items.push(("editor", editor));
        }
        if let Some(pager) = &settings.pager {
            items.push(("pager", pager));
        }
        if let Some(output) = &settings.output {
            items.push(("output", output));
        }
        if let Some(git_branch_template) = &settings.git_branch_template {
            items.push(("git_branch_template", git_branch_template));
        }
        if let Some(audit_log) = settings.audit_log {
            items.push(("audit_log", if audit_log { "true" } else { "false" }));
        }
    }

    if let Some(profile_name) = &config.active_profile {
        items.push(("profile", profile_name));
        if let Some(profile) = config.profiles.get(profile_name) {
            if let Some(org) = &profile.default_organization {
                items.push(("organization", org));
            }
            if let Some(proj) = &profile.default_project {
                items.push(("project", proj));
            }
        }
    }

    if session.json() {
        let map: serde_json::Map<_, _> = items
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::json!(v)))
            .collect();
        emit_json(session, &serde_json::Value::Object(map))?;
    } else {
        for (key, value) in &items {
            session
                .out
                .line(&format!("{key}={value}"))
                .map_err(CliError::general)?;
        }
        if items.is_empty() {
            session
                .out
                .line("no configuration values are set")
                .map_err(CliError::general)?;
        }
    }
    Ok(())
}

async fn cmd_get(session: &mut Session<'_>, key: &str) -> Result<(), CliError> {
    validate_key(key)?;
    let config = session.config.load()?;

    let value = match key {
        "profile" => config.active_profile.as_deref(),
        "organization" => config
            .active_profile
            .as_ref()
            .and_then(|name| config.profiles.get(name))
            .and_then(|p| p.default_organization.as_deref()),
        "project" => config
            .active_profile
            .as_ref()
            .and_then(|name| config.profiles.get(name))
            .and_then(|p| p.default_project.as_deref()),
        "editor" => config.settings.as_ref().and_then(|s| s.editor.as_deref()),
        "pager" => config.settings.as_ref().and_then(|s| s.pager.as_deref()),
        "output" => config.settings.as_ref().and_then(|s| s.output.as_deref()),
        "git_branch_template" => config
            .settings
            .as_ref()
            .and_then(|s| s.git_branch_template.as_deref()),
        "audit_log" => match config.settings.as_ref().and_then(|s| s.audit_log) {
            Some(value) => Some(if value { "true" } else { "false" }),
            None => None,
        },
        _ => {
            return Err(CliError::config(format!(
                "unknown configuration key: {key}"
            )));
        }
    };

    if session.json() {
        emit_json(
            session,
            &serde_json::json!({"key": key, "value": value.unwrap_or("")}),
        )?;
    } else if let Some(v) = value {
        session.out.line(v).map_err(CliError::general)?;
    } else {
        session
            .out
            .line(&format!("{key} is not set"))
            .map_err(CliError::general)?;
    }
    Ok(())
}

async fn cmd_set(session: &mut Session<'_>, key: &str, value: &str) -> Result<(), CliError> {
    validate_key(key)?;
    validate_value(key, value)?;
    reject_credential_value(value)?;

    let mut config = session.config.load()?;

    match key {
        "profile" => {
            if !config.profiles.contains_key(value) {
                return Err(CliError::config(format!("no such profile: {value}")));
            }
            config.active_profile = Some(value.to_string());
        }
        "organization" => {
            let profile_name = require_active_profile(&config)?;
            let profile = config
                .profiles
                .get_mut(&profile_name)
                .ok_or_else(|| CliError::config("active profile not found in config"))?;
            profile.default_organization = Some(value.to_string());
        }
        "project" => {
            let profile_name = require_active_profile(&config)?;
            let profile = config
                .profiles
                .get_mut(&profile_name)
                .ok_or_else(|| CliError::config("active profile not found in config"))?;
            profile.default_project = Some(value.to_string());
        }
        _ => {
            let settings = config.settings.get_or_insert(ConfigSettings::default());
            match key {
                "editor" => settings.editor = Some(value.to_string()),
                "pager" => settings.pager = Some(value.to_string()),
                "output" => settings.output = Some(value.to_string()),
                "git_branch_template" => settings.git_branch_template = Some(value.to_string()),
                "audit_log" => settings.audit_log = Some(parse_bool(value)?),
                _ => unreachable!(),
            }
        }
    }

    session.config.save(&config)?;
    crate::audit::record(&session.config, &mut session.out, "config.set", key, None);
    if session.json() {
        emit_json(
            session,
            &serde_json::json!({"key": key, "value": value, "set": true}),
        )?;
    } else {
        session
            .out
            .line(&format!("set {key}={value}"))
            .map_err(CliError::general)?;
    }
    Ok(())
}

async fn cmd_unset(session: &mut Session<'_>, key: &str) -> Result<(), CliError> {
    validate_key(key)?;
    let mut config = session.config.load()?;

    // `take()` decides `changed` from what was actually stored, so unsetting a
    // key that was never set rewrites nothing and reports honestly.
    let changed = match key {
        "profile" => config.active_profile.take().is_some(),
        "organization" => {
            let profile_name = require_active_profile(&config)?;
            config
                .profiles
                .get_mut(&profile_name)
                .is_some_and(|profile| profile.default_organization.take().is_some())
        }
        "project" => {
            let profile_name = require_active_profile(&config)?;
            config
                .profiles
                .get_mut(&profile_name)
                .is_some_and(|profile| profile.default_project.take().is_some())
        }
        _ => config.settings.as_mut().is_some_and(|settings| match key {
            "editor" => settings.editor.take().is_some(),
            "pager" => settings.pager.take().is_some(),
            "output" => settings.output.take().is_some(),
            "git_branch_template" => settings.git_branch_template.take().is_some(),
            "audit_log" => settings.audit_log.take().is_some(),
            _ => false,
        }),
    };

    if changed {
        session.config.save(&config)?;
        crate::audit::record(&session.config, &mut session.out, "config.unset", key, None);
        if session.json() {
            emit_json(session, &serde_json::json!({"key": key, "unset": true}))?;
        } else {
            session
                .out
                .line(&format!("unset {key}"))
                .map_err(CliError::general)?;
        }
    } else if session.json() {
        emit_json(session, &serde_json::json!({"key": key, "unset": false}))?;
    } else {
        session
            .out
            .line(&format!("{key} was not set"))
            .map_err(CliError::general)?;
    }
    Ok(())
}

/// Parses the only values a boolean setting accepts.
fn parse_bool(value: &str) -> Result<bool, CliError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(CliError::config(format!(
            "audit_log must be true or false (got {value})"
        ))),
    }
}

/// Validates purely local values before they are written.
///
/// Only client-side shape is checked here. Organization and project values are
/// server-owned identifiers, so the server stays authoritative for whether one
/// exists; this CLI rejects only values that could never work (empty, control
/// characters, absurd length) plus the preference spellings it owns outright.
fn validate_value(key: &str, value: &str) -> Result<(), CliError> {
    if value.trim().is_empty() {
        return Err(CliError::config(format!(
            "{key} must not be empty; remove the value instead with `hamstik config unset {key}`"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(CliError::config(format!(
            "{key} must not contain newlines or control characters"
        )));
    }
    let length = value.chars().count();
    if length > MAX_VALUE_LEN {
        return Err(CliError::config(format!(
            "{key} must be at most {MAX_VALUE_LEN} characters (got {length})"
        )));
    }
    if key == "output" && !OUTPUT_MODES.contains(&value) {
        return Err(CliError::config(format!(
            "invalid output preference: {value}; valid output modes: {}",
            OUTPUT_MODES.join(", ")
        )));
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), CliError> {
    if !VALID_KEYS.contains(&key) {
        return Err(CliError::config(format!(
            "unknown configuration key: {key}; valid keys: {valid}",
            valid = VALID_KEYS.join(", ")
        )));
    }
    Ok(())
}

fn require_active_profile(config: &ConfigFile) -> Result<String, CliError> {
    config
        .active_profile
        .clone()
        .ok_or_else(|| CliError::config("no active profile; set a profile first with `config set profile <name>` or `auth login`"))
}

/// Rejects values that look like credentials before they reach disk.
///
/// Configuration is a plain, world-readable-by-design file: tokens stay in the
/// OS credential store or `HAMSTIK_TOKEN` and are never written here, so a
/// value that reads like a secret is refused with the two real alternatives.
fn reject_credential_value(value: &str) -> Result<(), CliError> {
    let lowered = value.to_lowercase();
    if lowered.contains("token") || lowered.contains("secret") || lowered.contains("password") {
        return Err(CliError::config(
            "refusing to store a credential-like value in configuration; keep credentials in the OS \
             credential store (`hamstik auth login`) or set HAMSTIK_TOKEN instead",
        ));
    }
    Ok(())
}

fn emit_json(session: &mut Session<'_>, value: &serde_json::Value) -> Result<(), CliError> {
    crate::commands::emit_json(session, value)
}
