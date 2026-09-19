// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik sprint` (list / view / create / transitions / transition / report).

use serde_json::json;

use hamstik_api_client::{
    CompletionAction, CreateSprintRequest, ListOptions, PageItems, Sprint, SprintTransitionList,
    TransitionSprintRequest, follow_with, generate_key, validate_key,
};

use crate::app::Session;
use crate::args::{ReportPageArgs, SprintArgs, SprintCommand};
use crate::error::CliError;

use super::dryrun;
use super::org::render_lines;
use super::{check_columns, emit_view, follow_policy, render_list};

/// Runs the `sprint` subcommands.
pub async fn run(session: &mut Session<'_>, args: &SprintArgs) -> Result<(), CliError> {
    match &args.command {
        SprintCommand::List {
            project,
            pagination,
        } => list(session, project.as_deref(), pagination).await,
        SprintCommand::View { id, project } => view(session, id, project.as_deref()).await,
        SprintCommand::Create {
            name,
            start_date,
            end_date,
            goal,
            target_points,
            project,
            idempotency_key,
        } => {
            create(
                session,
                name,
                start_date.as_deref(),
                end_date.as_deref(),
                goal.as_deref(),
                *target_points,
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        SprintCommand::Report(args) => {
            report(session, &args.id, args.project.as_deref(), &args.page).await
        }
        SprintCommand::Transitions { id, project } => {
            transitions(session, id, project.as_deref()).await
        }
        SprintCommand::Archive {
            id,
            force,
            idempotency_key,
            project,
        } => {
            change_archive(
                session,
                id,
                true,
                *force,
                idempotency_key.as_deref(),
                project.as_deref(),
            )
            .await
        }
        SprintCommand::Unarchive {
            id,
            force,
            idempotency_key,
            project,
        } => {
            change_archive(
                session,
                id,
                false,
                *force,
                idempotency_key.as_deref(),
                project.as_deref(),
            )
            .await
        }
        SprintCommand::Transition {
            id,
            target,
            move_to_backlog,
            move_to_sprint,
            force,
            project,
            idempotency_key,
        } => {
            transition(
                session,
                id,
                target.as_str(),
                *move_to_backlog,
                move_to_sprint.as_deref(),
                *force,
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
    }
}

/// Resolves the effective project key: explicit flag wins over context.
fn require_project(session: &Session<'_>, explicit: Option<&str>) -> Result<String, CliError> {
    if let Some(project) = explicit {
        return Ok(project.to_string());
    }
    let selection = session.selection()?;
    session.require_project(&selection)
}

async fn list(
    session: &mut Session<'_>,
    project_flag: Option<&str>,
    pagination: &crate::args::PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    if pagination.all {
        let limit = pagination.directory_page_size();
        let fetch_api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let project = project.clone();
            async move {
                let response = fetch_api
                    .list_sprints(&org, &project, ListOptions { limit, cursor })
                    .await?;
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        })
        .await
        .map_err(CliError::from_client)?;
        let rows: Vec<Vec<String>> = page.items.iter().map(sprint_row).collect();
        let json_value = json!({ "items": page.raw_items, "page": page.page });
        check_columns(
            &["ID", "NAME", "STATE", "START", "END", "TARGET"],
            &session.output_options(),
        )?;
        render_list(
            session,
            &json_value,
            &["ID", "NAME", "STATE", "START", "END", "TARGET"],
            &rows,
        )
    } else {
        let response = api
            .list_sprints(
                &org,
                &project,
                ListOptions {
                    limit: pagination.directory_page_size(),
                    cursor: pagination.cursor.clone(),
                },
            )
            .await
            .map_err(CliError::from_client)?;
        let rows: Vec<Vec<String>> = response.value.items.iter().map(sprint_row).collect();
        check_columns(
            &["ID", "NAME", "STATE", "START", "END", "TARGET"],
            &session.output_options(),
        )?;
        render_list(
            session,
            &response.raw,
            &["ID", "NAME", "STATE", "START", "END", "TARGET"],
            &rows,
        )
    }
}

fn sprint_row(sprint: &Sprint) -> Vec<String> {
    vec![
        sprint.id.clone(),
        sprint.name.clone(),
        sprint.state.clone(),
        sprint.start_date.clone().unwrap_or_else(|| "-".to_string()),
        sprint.end_date.clone().unwrap_or_else(|| "-".to_string()),
        sprint
            .target_points
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string()),
    ]
}

async fn view(
    session: &mut Session<'_>,
    id: &str,
    project_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;
    let response = api
        .get_sprint(&org, &project, id)
        .await
        .map_err(CliError::from_client)?;
    let sprint = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_sprint(session, &sprint)
    })
}

fn render_sprint(session: &mut Session<'_>, sprint: &Sprint) -> Result<(), CliError> {
    let lines = [
        ("name", sprint.name.clone()),
        ("id", sprint.id.clone()),
        ("state", sprint.state.clone()),
        (
            "start",
            sprint.start_date.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "end",
            sprint.end_date.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "goal",
            sprint.goal.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "target points",
            sprint
                .target_points
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "archived",
            sprint
                .archived_at
                .clone()
                .unwrap_or_else(|| "no".to_string()),
        ),
        ("revision", sprint.revision.to_string()),
    ];
    render_lines(session, &lines)
}

/// `hamstik sprint report <id>`: reads the server Sprint delivery report.
///
/// The report is rendered from the server payload only; `--json` echoes that
/// payload verbatim. Pagination applies to the report's change feed (`items`),
/// which is why `--all` is not offered here.
async fn report(
    session: &mut Session<'_>,
    id: &str,
    project_flag: Option<&str>,
    page: &ReportPageArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;
    let response = api
        .get_sprint_report(
            &org,
            &project,
            id,
            ListOptions {
                limit: page.limit,
                cursor: page.cursor.clone(),
            },
        )
        .await
        .map_err(CliError::from_client)?;
    let report = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        super::report::render_sprint_report(session, &report)
    })
}

fn idem_key(flag: Option<&str>) -> Result<String, CliError> {
    match flag {
        Some(key) => {
            validate_key(key).map_err(|err| CliError::usage(err.to_string()))?;
            Ok(key.to_string())
        }
        None => Ok(generate_key()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn create(
    session: &mut Session<'_>,
    name: &Option<String>,
    start_date: Option<&str>,
    end_date: Option<&str>,
    goal: Option<&str>,
    target_points: Option<i64>,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;

    let name = match name {
        Some(name) => name.clone(),
        None if session.can_prompt() => session
            .prompt
            .read_line("Sprint name: ")
            .map_err(|err| CliError::general(format!("prompt failed: {err}")))?,
        None => return Err(CliError::usage("missing required option --name")),
    };
    if name.trim().is_empty() {
        return Err(CliError::usage("name must not be empty"));
    }

    let body = CreateSprintRequest {
        name,
        start_date: start_date.map(|v| v.to_string()).map(Some),
        end_date: end_date.map(|d| d.to_string()).map(Some),
        goal: goal.map(str::to_string),
        target_points,
    };
    let idempotency = idem_key(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "sprint.create",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/sprints",
                path: format!("/api/v1/organizations/{org}/projects/{project}/sprints"),
                resolved: json!({ "organization": org, "project": project }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let api = session.api(&selection)?;
    let response = api
        .create_sprint(&org, &project, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let sprint = response.value.clone();
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "sprint.create",
        &sprint.id.clone(),
        None,
        Some(sprint.revision),
        response.request_id.as_deref(),
    );
    emit_view(session, &response.raw, &sprint.id.clone(), |session| {
        render_sprint(session, &sprint)
    })
}

async fn transitions(
    session: &mut Session<'_>,
    id: &str,
    project_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;
    let response = api
        .list_sprint_transitions(&org, &project, id)
        .await
        .map_err(CliError::from_client)?;
    let list: SprintTransitionList = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        session
            .out
            .line(&format!("current state: {}", list.current_state))
            .map_err(CliError::general)?;
        if list.transitions.is_empty() {
            session
                .out
                .line("(no transitions available)")
                .map_err(CliError::general)?;
        } else {
            session.out.line("available:").map_err(CliError::general)?;
            for transition in &list.transitions {
                let requires = if transition.requires_completion_action {
                    " (requires --move-to-backlog or --move-to-sprint)"
                } else {
                    ""
                };
                session
                    .out
                    .line(&format!("  {}{}", transition.target_state, requires))
                    .map_err(CliError::general)?;
            }
        }
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
async fn transition(
    session: &mut Session<'_>,
    id: &str,
    target: &str,
    move_to_backlog: bool,
    move_to_sprint: Option<&str>,
    force: bool,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    if move_to_backlog && move_to_sprint.is_some() {
        return Err(CliError::usage(
            "cannot combine --move-to-backlog and --move-to-sprint",
        ));
    }
    if target != "done" && (move_to_backlog || move_to_sprint.is_some()) {
        return Err(CliError::usage(
            "completion options are only accepted when the target state is done",
        ));
    }

    let completion_action = if target == "done" && (move_to_backlog || move_to_sprint.is_some()) {
        Some(match move_to_sprint {
            Some(sprint_id) => CompletionAction::Sprint {
                target_sprint_id: sprint_id.to_string(),
            },
            None => CompletionAction::Backlog,
        })
    } else {
        None
    };

    let (if_match, revision_before) = if force {
        ("*".to_string(), None)
    } else {
        let current = api
            .get_sprint(&org, &project, id)
            .await
            .map_err(CliError::from_client)?;
        let etag = current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?;
        (etag, Some(current.value.revision))
    };

    let body = TransitionSprintRequest {
        target_state: target.to_string(),
        completion_action,
    };
    let idempotency = idem_key(idempotency_key)?;

    // Client-side prevalidation improves UX only; the server remains
    // authoritative for whether the transition is legal (SPEC §48).
    if !force {
        let allowed = api
            .list_sprint_transitions(&org, &project, id)
            .await
            .map_err(CliError::from_client)?;
        let permitted = allowed
            .value
            .transitions
            .iter()
            .any(|t| t.target_state == target);
        if !permitted {
            let options: Vec<&str> = allowed
                .value
                .transitions
                .iter()
                .map(|t| t.target_state.as_str())
                .collect();
            return Err(CliError::usage(format!(
                "sprint transition to {target} is not allowed from {}; available: {}",
                allowed.value.current_state,
                if options.is_empty() {
                    "none".to_string()
                } else {
                    options.join(", ")
                }
            )));
        }
    }

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "sprint.transition",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/sprints/{sprintId}/transitions",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/sprints/{id}/transitions"
                ),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "sprint": id,
                    "targetState": target,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .transition_sprint(&org, &project, id, &body, &if_match, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "sprint.transition",
        id,
        revision_before,
        Some(response.value.revision),
        response.request_id.as_deref(),
    );
    let sprint = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_sprint(session, &sprint)
    })
}

/// Archives or unarchives a Sprint with its Sprint ETag.
async fn change_archive(
    session: &mut Session<'_>,
    id: &str,
    archived: bool,
    force: bool,
    idempotency_key: Option<&str>,
    project_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    let (if_match, revision_before) = if force {
        ("*".to_string(), None)
    } else {
        let current = api
            .get_sprint(&org, &project, id)
            .await
            .map_err(CliError::from_client)?;
        let etag = current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?;
        (etag, Some(current.value.revision))
    };
    let idempotency = match idempotency_key {
        Some(key) => {
            validate_key(key).map_err(|err| CliError::usage(err.to_string()))?;
            key.to_string()
        }
        None => generate_key(),
    };

    if session.global.dry_run {
        let (operation, suffix) = if archived {
            ("sprint.archive", "/archive")
        } else {
            ("sprint.unarchive", "/unarchive")
        };
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation,
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/sprints/{sprintId}/archive",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/sprints/{id}{suffix}"
                ),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "sprint": id,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(json!({})),
                notes: Vec::new(),
            },
        );
    }

    let response = if archived {
        api.archive_sprint(&org, &project, id, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?
    } else {
        api.unarchive_sprint(&org, &project, id, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?
    };
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let command_name = if archived {
        "sprint.archive"
    } else {
        "sprint.unarchive"
    };
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        command_name,
        id,
        revision_before,
        Some(response.value.revision),
        response.request_id.as_deref(),
    );
    let sprint = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_sprint(session, &sprint)
    })
}
