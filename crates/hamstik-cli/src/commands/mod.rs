// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Command dispatch and shared output helpers.

use hamstik_api_client::FollowPolicy;
use serde_json::Value;

use crate::app::{Selection, Session};
use crate::args::{AgentCommand, Command, PaginationArgs};
use crate::config::{Profile, profile_auto_name, unique_profile_name};
use crate::error::CliError;
use crate::output::{self, OutputOptions};

pub mod advanced_dashboard;
pub mod advanced_report;
pub mod agent_skill;
pub mod api;
pub mod auth;
pub mod bulk_preflight;
pub mod commands_manifest;
pub mod complete;
pub mod completion;
pub mod config;
pub mod context_cmd;
pub mod credential;
pub mod doctor;
pub mod dryrun;
pub mod external;
pub mod init;
pub mod label;
pub mod manifest;
pub mod me;
pub mod org;
pub mod project;
pub mod report;
pub mod sprint;
pub mod squeakql;
pub mod stats;
pub mod user;
pub mod work;
pub mod work_context;

/// Page-traversal policy implied by a command's shared pagination flags.
///
/// `--limit` caps the total result, `--all` decides whether cursors are
/// followed, and `--cursor` / `--since-cursor` is the opaque cursor the stream
/// starts after — forwarded verbatim, including when `--all` continues from it.
#[must_use]
pub(crate) fn follow_policy(pagination: &PaginationArgs) -> FollowPolicy {
    FollowPolicy {
        follow: pagination.all,
        max_items: pagination.max_items(),
        start_cursor: pagination.cursor.clone(),
    }
}

/// Runs the selected subcommand against the session.
pub async fn dispatch(session: &mut Session<'_>, command: &Command) -> Result<(), CliError> {
    match command {
        Command::Me => me::run(session).await,
        Command::Auth(args) => auth::run(session, args).await,
        Command::Init => init::run(session),
        Command::Context(args) => context_cmd::run(session, args).await,
        Command::Config(args) => config::run(session, args).await,
        Command::Org(args) => org::run(session, args).await,
        Command::Project(args) => project::run(session, args).await,
        Command::Report(args) => advanced_report::run(session, args).await,
        Command::Dashboard(args) => advanced_dashboard::run(session, args).await,
        Command::Sprint(args) => sprint::run(session, args).await,
        Command::Label(args) => label::run(session, args).await,
        Command::Work(args) => work::run(session, args).await,
        Command::User(args) => user::run(session, args).await,
        Command::Squeakql(args) => squeakql::run(session, args).await,
        Command::Agent(args) => match &args.command {
            AgentCommand::Skill(skill) => agent_skill::run(session, &skill.command),
        },
        Command::Api(args) => api::run(session, args).await,
        Command::Doctor(args) => {
            doctor::run(session, args.local_only, args.bundle.as_deref()).await
        }
        Command::Completion(args) => completion::run(session, args),
        Command::Complete(args) => complete::run(session, args).await,
        Command::Commands(_) => commands_manifest::run(session),
        Command::Version => version(session),
        Command::External(args) => external::run_plugin(session, &args[0], &args[1..]),
    }
}

/// True when the command supports `--dry-run`: only mutation
/// commands preview; reads have nothing to preview.
pub(crate) fn supports_dry_run(command: &Command) -> bool {
    match command {
        Command::Work(args) => match &args.command {
            crate::args::WorkCommand::Context(_) => false,
            crate::args::WorkCommand::List(_)
            | crate::args::WorkCommand::Mine(_)
            | crate::args::WorkCommand::Search { .. }
            | crate::args::WorkCommand::View(_)
            | crate::args::WorkCommand::Transitions { .. }
            | crate::args::WorkCommand::Activity { .. } => false,
            crate::args::WorkCommand::Watcher(args) => {
                !matches!(args.command, crate::args::WorkWatcherCommand::Show { .. })
            }
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
            | crate::args::WorkCommand::Await(_)
            | crate::args::WorkCommand::Bulk(_) => true,
        },
        Command::Project(args) => match &args.command {
            crate::args::ProjectCommand::List { .. }
            | crate::args::ProjectCommand::View { .. }
            | crate::args::ProjectCommand::Report { .. }
            | crate::args::ProjectCommand::Stats { .. }
            | crate::args::ProjectCommand::Activity { .. }
            | crate::args::ProjectCommand::Use { .. } => false,
            crate::args::ProjectCommand::Create(_)
            | crate::args::ProjectCommand::Edit(_)
            | crate::args::ProjectCommand::Archive { .. }
            | crate::args::ProjectCommand::Unarchive { .. } => true,
        },
        Command::Report(args) => matches!(
            args.command,
            crate::args::AdvancedReportCommand::Create { .. }
                | crate::args::AdvancedReportCommand::Edit { .. }
                | crate::args::AdvancedReportCommand::Delete { .. }
        ),
        Command::Sprint(args) => match &args.command {
            crate::args::SprintCommand::List { .. }
            | crate::args::SprintCommand::View { .. }
            | crate::args::SprintCommand::Report { .. }
            | crate::args::SprintCommand::Stats { .. }
            | crate::args::SprintCommand::Transitions { .. } => false,
            crate::args::SprintCommand::Create { .. }
            | crate::args::SprintCommand::Transition { .. }
            | crate::args::SprintCommand::Archive { .. }
            | crate::args::SprintCommand::Unarchive { .. } => true,
        },
        Command::Label(args) => {
            matches!(args.command, crate::args::LabelCommand::Create { .. })
        }
        // the passthrough previews mutations; the api handler
        // rejects --dry-run on GET (reads have nothing to preview).
        Command::Api(args) => {
            matches!(
                args.command,
                crate::args::ApiCommand::Request(_) | crate::args::ApiCommand::Passthrough(_)
            )
        }
        // External plugins make no API calls themselves; `--dry-run` is
        // forwarded to the plugin as `HAMSTIK_DRY_RUN` (see external.rs and
        // the Agent Skill contract) rather than being rejected as a no-op.
        Command::External(_) => true,
        _ => false,
    }
}

/// Returns the profile name the default-organization/project write targets.
///
/// When a profile is already selected it is returned unchanged. Under an
/// ephemeral `HAMSTIK_TOKEN` no profile exists, so one is created on the fly
/// from `GET /api/v1/me` (the same flow `auth login` uses) and made active —
/// SPEC §28 otherwise selects no profile, which would leave the default the
/// caller is about to write permanently unreachable by resolution.
pub(crate) async fn ensure_profile_for_default(
    session: &Session<'_>,
    selection: &Selection,
) -> Result<String, CliError> {
    if let Some(name) = &selection.profile {
        return Ok(name.clone());
    }

    let secret = session.token_for(selection)?;
    let api = session.build_client(selection.host.clone(), secret)?;
    let me = api.whoami().await.map_err(CliError::from_client)?;
    let me = me.value;

    let mut config = session.config.load()?;
    let base = profile_auto_name(selection.host.as_str(), &me.email);
    let name = unique_profile_name(&config, &base, &me.id, selection.host.as_str());
    config.profiles.insert(
        name.clone(),
        Profile {
            host: selection.host.as_str().to_string(),
            user_id: me.id.clone(),
            email: me.email.clone(),
            default_organization: me.default_organization.map(|org| org.slug),
            default_project: None,
        },
    );
    config.active_profile = Some(name.clone());
    // The profile was chosen by this command, not by the user's selection
    // state; make it active so the default written by the caller is actually
    // reachable by resolution. The write is a single atomic replace.
    session.config.save(&config)?;
    Ok(name)
}

/// `hamstik version`: release identity for the running binary.
///
/// JSON output keeps the stable `version` field and adds `commit`/`target`
/// build metadata (additive per design/VERSIONING.md §3). Human output keeps
/// the identity banner and appends the build-identity lines. Neither surface
/// performs I/O: the values are compile-time constants (see
/// [`crate::build_info`]).
fn version(session: &mut Session<'_>) -> Result<(), CliError> {
    use crate::build_info;
    if session.json() {
        emit_json(
            session,
            &serde_json::json!({
                "version": build_info::VERSION,
                "commit": build_info::COMMIT,
                "target": build_info::TARGET,
            }),
        )
    } else {
        let details = format!(
            "{}\n\ncommit    {}\ntarget    {}",
            crate::banner::banner(),
            build_info::short_commit(),
            build_info::TARGET,
        );
        session.out.line(&details).map_err(CliError::general)
    }
}

pub(crate) fn emit_json(session: &mut Session<'_>, value: &Value) -> Result<(), CliError> {
    session.out.json(value).map_err(CliError::general)
}

/// Renders a list collection in the active output mode.
///
/// `--jq` filtering and `--columns` projection are applied by
/// [`crate::output::Output::render_list`] so every collection command — and only
/// the collection commands — share one contract for both.
pub(crate) fn render_list(
    session: &mut Session<'_>,
    json_value: &Value,
    headers: &[&str],
    rows: &[Vec<String>],
) -> Result<(), CliError> {
    let options = session.output_options();
    session
        .out
        .render_list(json_value, headers, rows, &options)
        .map_err(CliError::general)
}

/// Renders a list through the shared output-mode implementation.
///
/// This compatibility wrapper keeps nested list commands on the same JSONL,
/// TSV, jq, and column-selection contract as commands migrated to call
/// [`render_list`] directly.
pub(crate) fn emit_table(
    session: &mut Session<'_>,
    json_value: &Value,
    headers: &[&str],
    rows: &[Vec<String>],
) -> Result<(), CliError> {
    check_columns(headers, &session.output_options())?;
    render_list(session, json_value, headers, rows)
}

/// Validates that every column listed in `options.columns` exists in `headers`.
///
/// An unknown column is invalid input (exit 2) and carries the list of valid
/// names, so the caller can correct the request without reading the source.
pub(crate) fn check_columns(headers: &[&str], options: &OutputOptions) -> Result<(), CliError> {
    output::validate_columns(&options.columns, headers)
        .map(|_| ())
        .map_err(CliError::usage)
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

/// The current UTC time as an RFC 3339 string (second precision).
///
/// Used for informational timestamps in local files; the clock is the
/// process clock and is never treated as authoritative.
#[must_use]
pub fn timestamp_rfc3339() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let days = seconds.div_euclid(86_400);
    let secs_of_day = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}
