// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik schedule` — thin wrappers around periodic export/report snapshots.
//!
//! The CLI ships no daemon. A schedule definition is a plain TOML file under
//! `<config-dir>/schedules/<name>.toml` holding a `hamstik` argument vector.
//! An external scheduler (cron, systemd timers, Task Scheduler) invokes
//! `hamstik schedule run <name>`, which re-executes that exact command, so a
//! scheduled run produces the same bytes as the equivalent manual invocation.
//! Definitions never contain credentials; the child inherits the caller's
//! environment and resolved context exactly as a manual run would.

use std::process::Command as ProcessCommand;

use serde_json::json;

use crate::app::Session;
use crate::args::{ScheduleArgs, ScheduleCommand};
use crate::config::{
    MAX_SCHEDULE_ARGV, MAX_SCHEDULES, SCHEDULE_VERSION, ScheduleDefinition, ScheduleStore,
};
use crate::error::CliError;

use super::emit_json;

/// Runs a `schedule` subcommand.
pub fn run(session: &mut Session<'_>, args: &ScheduleArgs) -> Result<(), CliError> {
    match &args.command {
        ScheduleCommand::List => list(session),
        ScheduleCommand::Save {
            name,
            force,
            description,
            command,
        } => save(session, name, *force, description.as_deref(), command),
        ScheduleCommand::Delete { name } => delete(session, name),
        ScheduleCommand::Run { name } => run_definition(session, name),
    }
}

/// Locates the schedule store for the session's config.
fn store(session: &Session<'_>) -> ScheduleStore {
    ScheduleStore::adjacent_to(session.config.path())
}

/// Lists saved definitions (name + command), sorted by name.
fn list(session: &mut Session<'_>) -> Result<(), CliError> {
    let store = store(session);
    let schedules = store.list()?;
    if session.json() {
        let definitions: Vec<serde_json::Value> = schedules
            .iter()
            .map(|(name, definition)| {
                json!({
                    "name": name,
                    "command": definition.command,
                    "description": definition.description,
                    "savedAt": definition.saved_at,
                    "path": store.path_for(name).display().to_string(),
                })
            })
            .collect();
        return emit_json(
            session,
            &json!({ "scheduleVersion": SCHEDULE_VERSION, "schedules": definitions }),
        );
    }
    if schedules.is_empty() {
        return session
            .out
            .human("no schedules; save one with `hamstik schedule save <name> -- <command...>`")
            .map_err(CliError::general);
    }
    for (name, definition) in &schedules {
        session
            .out
            .line(&format!(
                "{name:<24} {}",
                shell_preview(&definition.command)
            ))
            .map_err(CliError::general)?;
    }
    Ok(())
}

/// A human-readable preview of a stored command line. Values that contain
/// whitespace are quoted only for display; execution always uses the exact
/// argv, never this string.
fn shell_preview(command: &[String]) -> String {
    command
        .iter()
        .map(|arg| {
            if arg.contains(char::is_whitespace) {
                format!("{arg:?}")
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Saves (or replaces) a schedule definition.
fn save(
    session: &mut Session<'_>,
    name: &str,
    force: bool,
    description: Option<&str>,
    command: &[String],
) -> Result<(), CliError> {
    ScheduleStore::validate_name(name)?;
    if command.is_empty() {
        return Err(CliError::usage(
            "the scheduled command must not be empty; pass it after `--`",
        ));
    }
    if command.len() > MAX_SCHEDULE_ARGV {
        return Err(CliError::usage(format!(
            "the scheduled command has {} arguments; the limit is {MAX_SCHEDULE_ARGV}",
            command.len()
        )));
    }
    let store = store(session);
    let exists = store.path_for(name).exists();
    if exists && !force {
        return Err(CliError::usage(format!(
            "a schedule named {name:?} already exists; re-run with --force to replace it"
        )));
    }
    if !exists && store.list()?.len() >= MAX_SCHEDULES {
        return Err(CliError::usage(format!(
            "already holding {MAX_SCHEDULES} schedules; delete one first"
        )));
    }
    let definition = ScheduleDefinition {
        version: SCHEDULE_VERSION,
        saved_at: Some(crate::commands::timestamp_rfc3339()),
        description: description.map(str::to_string),
        command: command.to_vec(),
    };
    let path = store.save(name, &definition)?;
    crate::audit::record(
        &session.config,
        &mut session.out,
        "schedule.save",
        name,
        None,
    );
    if session.json() {
        return emit_json(
            session,
            &json!({
                "scheduleVersion": SCHEDULE_VERSION,
                "saved": true,
                "name": name,
                "command": definition.command,
                "path": path.display().to_string(),
            }),
        );
    }
    session
        .out
        .line(&format!("saved schedule {name}"))
        .map_err(CliError::general)
}

/// Deletes a saved definition.
fn delete(session: &mut Session<'_>, name: &str) -> Result<(), CliError> {
    let store = store(session);
    let path = store.delete(name)?;
    crate::audit::record(
        &session.config,
        &mut session.out,
        "schedule.delete",
        name,
        None,
    );
    if session.json() {
        return emit_json(
            session,
            &json!({
                "scheduleVersion": SCHEDULE_VERSION,
                "deleted": true,
                "name": name,
                "path": path.display().to_string(),
            }),
        );
    }
    session
        .out
        .line(&format!("deleted schedule {name}"))
        .map_err(CliError::general)
}

/// Re-executes the stored command through the running binary.
///
/// The child inherits this process's environment, working directory, and
/// stdio, so its output is byte-identical to a manual run of the same command
/// line. The child's exit code becomes this invocation's exit code.
fn run_definition(session: &mut Session<'_>, name: &str) -> Result<(), CliError> {
    let store = store(session);
    let definition = store.load(name)?;
    crate::audit::record(
        &session.config,
        &mut session.out,
        "schedule.run",
        name,
        None,
    );
    let program = std::env::current_exe().map_err(|err| {
        CliError::general(format!(
            "cannot locate the running hamstik executable: {err}"
        ))
    })?;
    let status = ProcessCommand::new(&program)
        .args(&definition.command)
        .status()
        .map_err(|err| CliError::general(format!("cannot run schedule {name:?}: {err}")))?;
    if !status.success() {
        // A signal-terminated child has no code on Unix; report a generic
        // failure rather than silently claiming success.
        session.exit_code = status.code().unwrap_or(1);
    }
    Ok(())
}
