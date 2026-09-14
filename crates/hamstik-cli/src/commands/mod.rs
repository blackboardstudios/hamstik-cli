// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Command dispatch and shared output helpers.

use serde_json::Value;

use crate::app::Session;
use crate::args::Command;
use crate::error::CliError;

pub mod api;
pub mod auth;
pub mod bulk_preflight;
pub mod completion;
pub mod context_cmd;
pub mod credential;
pub mod doctor;
pub mod dryrun;
pub mod label;
pub mod me;
pub mod org;
pub mod project;
pub mod sprint;
pub mod squeakql;
pub mod user;
pub mod work;

/// Runs the selected subcommand against the session.
pub async fn dispatch(session: &mut Session<'_>, command: &Command) -> Result<(), CliError> {
    match command {
        Command::Me => me::run(session).await,
        Command::Auth(args) => auth::run(session, args).await,
        Command::Context(args) => context_cmd::run(session, args).await,
        Command::Org(args) => org::run(session, args).await,
        Command::Project(args) => project::run(session, args).await,
        Command::Sprint(args) => sprint::run(session, args).await,
        Command::Label(args) => label::run(session, args).await,
        Command::Work(args) => work::run(session, args).await,
        Command::User(args) => user::run(session, args).await,
        Command::Squeakql(args) => squeakql::run(session, args).await,
        Command::Api(args) => api::run(session, args).await,
        Command::Doctor { local_only } => doctor::run(session, *local_only).await,
        Command::Completion(args) => completion::run(session, args),
        Command::Version => version(session),
    }
}

/// True when the command supports `--dry-run` (CLI-10): only mutation
/// commands preview; reads have nothing to preview.
pub(crate) fn supports_dry_run(command: &Command) -> bool {
    match command {
        Command::Work(args) => match &args.command {
            crate::args::WorkCommand::List(_)
            | crate::args::WorkCommand::Mine(_)
            | crate::args::WorkCommand::Search { .. }
            | crate::args::WorkCommand::View { .. }
            | crate::args::WorkCommand::Transitions { .. }
            | crate::args::WorkCommand::Activity { .. } => false,
            crate::args::WorkCommand::Label(args) => {
                matches!(
                    args.command,
                    crate::args::WorkLabelCommand::Add { .. }
                        | crate::args::WorkLabelCommand::Remove { .. }
                )
            }
            crate::args::WorkCommand::Attachment(args) => {
                matches!(
                    args.command,
                    crate::args::WorkAttachmentCommand::Upload { .. }
                        | crate::args::WorkAttachmentCommand::Delete { .. }
                )
            }
            crate::args::WorkCommand::Comment(args) => {
                matches!(
                    args.command,
                    crate::args::CommentCommand::Add { .. }
                        | crate::args::CommentCommand::Edit { .. }
                        | crate::args::CommentCommand::Delete { .. }
                )
            }
            crate::args::WorkCommand::Link(args) => {
                matches!(
                    args.command,
                    crate::args::WorkLinkCommand::Add { .. }
                        | crate::args::WorkLinkCommand::Delete { .. }
                )
            }
            crate::args::WorkCommand::Create(_)
            | crate::args::WorkCommand::Edit(_)
            | crate::args::WorkCommand::Transition { .. }
            | crate::args::WorkCommand::Start { .. }
            | crate::args::WorkCommand::Close { .. }
            | crate::args::WorkCommand::Archive { .. }
            | crate::args::WorkCommand::Unarchive { .. }
            | crate::args::WorkCommand::Delete { .. }
            | crate::args::WorkCommand::Bulk(_) => true,
        },
        Command::Project(args) => match &args.command {
            crate::args::ProjectCommand::List { .. }
            | crate::args::ProjectCommand::View { .. }
            | crate::args::ProjectCommand::Activity { .. }
            | crate::args::ProjectCommand::Use { .. } => false,
            crate::args::ProjectCommand::Create(_)
            | crate::args::ProjectCommand::Edit(_)
            | crate::args::ProjectCommand::Archive { .. }
            | crate::args::ProjectCommand::Unarchive { .. } => true,
        },
        Command::Sprint(args) => match &args.command {
            crate::args::SprintCommand::List { .. }
            | crate::args::SprintCommand::View { .. }
            | crate::args::SprintCommand::Transitions { .. } => false,
            crate::args::SprintCommand::Create { .. }
            | crate::args::SprintCommand::Transition { .. } => true,
        },
        Command::Label(args) => {
            matches!(args.command, crate::args::LabelCommand::Create { .. })
        }
        _ => false,
    }
}

fn version(session: &mut Session<'_>) -> Result<(), CliError> {
    let version = env!("CARGO_PKG_VERSION");
    if session.json() {
        emit_json(session, &serde_json::json!({ "version": version }))
    } else {
        session
            .out
            .line(&crate::banner::banner())
            .map_err(CliError::general)
    }
}

pub(crate) fn emit_json(session: &mut Session<'_>, value: &Value) -> Result<(), CliError> {
    session.out.json(value).map_err(CliError::general)
}

/// Renders a list: JSON body verbatim, or a table (first column in quiet mode).
pub(crate) fn emit_table(
    session: &mut Session<'_>,
    json_value: &Value,
    headers: &[&str],
    rows: &[Vec<String>],
) -> Result<(), CliError> {
    if session.json() {
        emit_json(session, json_value)?;
    } else if session.out.is_quiet() {
        for row in rows {
            if let Some(cell) = row.first() {
                session.out.line(cell).map_err(CliError::general)?;
            }
        }
    } else {
        session
            .out
            .table(headers, rows)
            .map_err(CliError::general)?;
    }
    Ok(())
}

/// Renders a single resource: JSON body verbatim, quiet identifier, or detail.
pub(crate) fn emit_view<F>(
    session: &mut Session<'_>,
    json_value: &Value,
    quiet_id: &str,
    detail: F,
) -> Result<(), CliError>
where
    F: FnOnce(&mut Session<'_>) -> Result<(), CliError>,
{
    if session.json() {
        emit_json(session, json_value)?;
    } else if session.out.is_quiet() {
        session.out.line(quiet_id).map_err(CliError::general)?;
    } else {
        detail(session)?;
    }
    Ok(())
}

/// Resolves a user-facing user argument into a `usr_` public ID.
///
/// Accepts an existing public ID verbatim or `me`, which is resolved through
/// `GET /me` (SPEC: writes accept only UUIDs or `usr_` public IDs; `me` is a
/// CLI-side convenience). The literal `none` is forwarded for callers that
/// use it as a clear-assignment sentinel.
pub(crate) async fn resolve_user_arg(
    session: &Session<'_>,
    value: &str,
) -> Result<String, CliError> {
    if value.eq_ignore_ascii_case("none") {
        return Ok(value.to_string());
    }
    if value.starts_with("usr_") {
        return Ok(value.to_string());
    }
    if !value.eq_ignore_ascii_case("me") {
        return Err(CliError::usage(format!(
            "invalid user argument {value:?}; expected `me`, `none`, or a `usr_` public ID"
        )));
    }
    let selection = session.selection()?;
    let api = session.api(&selection)?;
    let response = api.whoami().await.map_err(CliError::from_client)?;
    Ok(response.value.public_id.clone())
}
