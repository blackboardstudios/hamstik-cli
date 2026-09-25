// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik milestone` (list / view / create / edit / transitions / transition
//! / releases / release / events).

use serde_json::json;

use hamstik_api_client::{
    CreateOrganizationMilestoneRequest, ListMilestonesOptions, MilestoneDetailQuery,
    MilestoneEventsQuery, MilestoneReleasesQuery, MutateOrganizationMilestoneReleaseRequest,
    OrganizationMilestone, OrganizationMilestoneDetail, OrganizationMilestoneEvent,
    OrganizationMilestoneTransitionList, Page, PageItems, TransitionOrganizationMilestoneRequest,
    UpdateOrganizationMilestoneRequest, follow_with,
};

use crate::app::Session;
use crate::args::{
    MilestoneArgs, MilestoneCommand, MilestoneReleaseCommand, MilestoneStateArg, PaginationArgs,
};
use crate::error::CliError;
use crate::time_arg;
use crate::time_arg::TimeArg;

use super::dryrun;
use super::org::render_lines;
use super::release::{create_description, dash, edit_description, idem_key, owner_assignment};
use super::{check_columns, emit_view, follow_policy, render_list};

/// Runs the `milestone` subcommands.
pub async fn run(session: &mut Session<'_>, args: &MilestoneArgs) -> Result<(), CliError> {
    match &args.command {
        MilestoneCommand::List {
            state,
            include_archived,
            pagination,
        } => {
            list(
                session,
                state.map(MilestoneStateArg::as_str),
                *include_archived,
                pagination,
            )
            .await
        }
        MilestoneCommand::View {
            id,
            release_after,
            release_limit,
        } => view(session, id, release_after.as_deref(), *release_limit).await,
        MilestoneCommand::Create {
            name,
            description,
            description_file,
            description_editor,
            owner,
            target_date,
            idempotency_key,
        } => {
            create(
                session,
                name.as_deref(),
                description.as_deref(),
                description_file.as_deref(),
                *description_editor,
                owner.as_deref(),
                target_date.as_ref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        MilestoneCommand::Edit {
            id,
            name,
            description,
            description_file,
            description_editor,
            clear_description,
            owner,
            target_date,
            clear_target_date,
            reason,
            force,
            idempotency_key,
        } => {
            edit(
                session,
                id,
                name.as_deref(),
                description.as_deref(),
                description_file.as_deref(),
                *description_editor,
                *clear_description,
                owner.as_deref(),
                target_date.as_ref(),
                *clear_target_date,
                reason.as_deref(),
                *force,
                idempotency_key.as_deref(),
            )
            .await
        }
        MilestoneCommand::Transitions { id } => transitions(session, id).await,
        MilestoneCommand::Transition {
            id,
            target,
            reason,
            force,
            idempotency_key,
        } => {
            transition(
                session,
                id,
                target.as_str(),
                reason.as_deref(),
                *force,
                idempotency_key.as_deref(),
            )
            .await
        }
        MilestoneCommand::Releases {
            id,
            release_after,
            pagination,
        } => releases(session, id, release_after.as_deref(), pagination).await,
        MilestoneCommand::Release { command } => match command {
            MilestoneReleaseCommand::Add {
                id,
                release_id,
                reason,
                force,
                idempotency_key,
            } => {
                mutate_release(
                    session,
                    id,
                    release_id,
                    "add",
                    false,
                    reason.as_deref(),
                    *force,
                    idempotency_key.as_deref(),
                )
                .await
            }
            MilestoneReleaseCommand::Remove {
                id,
                release_id,
                confirm_remove,
                reason,
                force,
                idempotency_key,
            } => {
                mutate_release(
                    session,
                    id,
                    release_id,
                    "remove",
                    *confirm_remove,
                    reason.as_deref(),
                    *force,
                    idempotency_key.as_deref(),
                )
                .await
            }
        },
        MilestoneCommand::Events {
            id,
            before_event_id,
            limit,
        } => events(session, id, before_event_id.as_deref(), *limit).await,
    }
}

fn milestone_row(milestone: &OrganizationMilestone) -> Vec<String> {
    vec![
        milestone.id.clone(),
        milestone.name.clone(),
        milestone.state.clone(),
        dash(milestone.target_date.as_deref()),
        milestone
            .release_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "-".to_string()),
        milestone
            .work_item_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "-".to_string()),
        milestone
            .completed_work_item_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "-".to_string()),
    ]
}

const MILESTONE_COLUMNS: [&str; 7] = ["ID", "NAME", "STATE", "TARGET", "RELEASES", "ITEMS", "DONE"];

fn finish_milestone_list(
    session: &mut Session<'_>,
    page: &PageItems<OrganizationMilestone>,
) -> Result<(), CliError> {
    let rows: Vec<Vec<String>> = page.items.iter().map(milestone_row).collect();
    let json_value = json!({ "items": page.raw_items, "page": page.page });
    check_columns(&MILESTONE_COLUMNS, &session.output_options())?;
    render_list(session, &json_value, &MILESTONE_COLUMNS, &rows)
}

async fn list(
    session: &mut Session<'_>,
    state: Option<&str>,
    include_archived: Option<bool>,
    pagination: &PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let opts = ListMilestonesOptions {
        limit: pagination.directory_page_size(),
        cursor: pagination.cursor.clone(),
        state: state.map(str::to_string),
        include_archived,
    };

    if pagination.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let opts = ListMilestonesOptions {
                cursor,
                ..opts.clone()
            };
            async move {
                let response = fetch_api.list_organization_milestones(&org, opts).await?;
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        })
        .await
        .map_err(CliError::from_client)?;
        finish_milestone_list(session, &page)
    } else {
        let response = api
            .list_organization_milestones(&org, opts)
            .await
            .map_err(CliError::from_client)?;
        finish_milestone_list(
            session,
            &PageItems::new(response.value.items, &response.raw, response.value.page),
        )
    }
}

fn render_milestone(
    session: &mut Session<'_>,
    milestone: &OrganizationMilestone,
) -> Result<(), CliError> {
    let lines = [
        ("name", milestone.name.clone()),
        ("id", milestone.id.clone()),
        ("state", milestone.state.clone()),
        ("description", dash(milestone.description.as_deref())),
        (
            "owner",
            milestone
                .owner_name
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("target", dash(milestone.target_date.as_deref())),
        ("completed", dash(milestone.completed_at.as_deref())),
        ("archived", dash(milestone.archived_at.as_deref())),
        ("revision", milestone.revision.to_string()),
        (
            "releases",
            milestone
                .release_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "work items",
            milestone
                .work_item_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "completed",
            milestone
                .completed_work_item_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("created", milestone.created_at.clone()),
        ("updated", milestone.updated_at.clone()),
    ];
    render_lines(session, &lines)
}

async fn view(
    session: &mut Session<'_>,
    id: &str,
    release_after: Option<&str>,
    release_limit: Option<u32>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .get_organization_milestone(
            &org,
            id,
            MilestoneDetailQuery {
                release_after: release_after.map(str::to_string),
                limit: release_limit,
            },
        )
        .await
        .map_err(CliError::from_client)?;
    let detail: OrganizationMilestoneDetail = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_milestone(session, &detail.milestone)?;
        if detail.releases.is_empty() {
            session
                .out
                .line("\nreleases: (none)")
                .map_err(CliError::general)?;
        } else {
            session.out.line("\nreleases:").map_err(CliError::general)?;
            for release in &detail.releases {
                session
                    .out
                    .line(&format!(
                        "  {}  {}  {}  {}/{} done  ({})",
                        release.id,
                        release.name,
                        release.state,
                        release.completed_work_items,
                        release.total_work_items,
                        release.project_key,
                    ))
                    .map_err(CliError::general)?;
            }
        }
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
async fn create(
    session: &mut Session<'_>,
    name: Option<&str>,
    description: Option<&str>,
    description_file: Option<&str>,
    description_editor: bool,
    owner: Option<&str>,
    target_date: Option<&TimeArg>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    time_arg::report_resolved(&mut session.out, &[("--target-date", target_date)]);
    let org = session.require_org(&selection)?;

    let name = match name {
        Some(name) => name.to_string(),
        None if session.can_prompt() => session
            .prompt
            .read_line("Milestone name: ")
            .map_err(|err| CliError::general(format!("prompt failed: {err}")))?,
        None => return Err(CliError::usage("missing required option --name")),
    };
    if name.trim().is_empty() {
        return Err(CliError::usage("name must not be empty"));
    }
    let owner_id = owner_assignment(session, owner).await?;
    let body = CreateOrganizationMilestoneRequest {
        name,
        description: create_description(
            description,
            description_file,
            description_editor,
            session,
        )?,
        owner_id,
        target_date: target_date.map(|date| Some(date.to_string())),
    };
    let idempotency = idem_key(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "milestone.create",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/milestones",
                path: format!("/api/v1/organizations/{org}/milestones"),
                resolved: json!({ "organization": org }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let api = session.api(&selection)?;
    let response = api
        .create_organization_milestone(&org, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let milestone = response.value.clone();
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "milestone.create",
        &milestone.id,
        None,
        Some(milestone.revision),
        response.request_id.as_deref(),
    );
    emit_view(session, &response.raw, &milestone.id, |session| {
        render_milestone(session, &milestone)
    })
}
#[allow(clippy::too_many_arguments)]
async fn edit(
    session: &mut Session<'_>,
    id: &str,
    name: Option<&str>,
    description: Option<&str>,
    description_file: Option<&str>,
    description_editor: bool,
    clear_description: bool,
    owner: Option<&str>,
    target_date: Option<&TimeArg>,
    clear_target_date: bool,
    reason: Option<&str>,
    force: bool,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    time_arg::report_resolved(&mut session.out, &[("--target-date", target_date)]);
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;

    let owner_id = owner_assignment(session, owner).await?;
    let body = UpdateOrganizationMilestoneRequest {
        name: name.map(|value| Some(value.to_string())),
        description: edit_description(
            description,
            description_file,
            description_editor,
            clear_description,
            session,
            "a milestone description edit",
        )?,
        owner_id,
        target_date: match (target_date, clear_target_date) {
            (Some(date), _) => Some(Some(date.to_string())),
            (None, true) => Some(None),
            (None, false) => None,
        },
        reason: reason.map(str::to_string),
    };
    if body.is_empty() {
        return Err(CliError::usage("no changes specified"));
    }

    let (if_match, revision_before) = if force {
        ("*".to_string(), None)
    } else {
        let current = api
            .get_organization_milestone(&org, id, MilestoneDetailQuery::default())
            .await
            .map_err(CliError::from_client)?;
        let etag = current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?;
        (etag, Some(current.value.milestone.revision))
    };
    let idempotency = idem_key(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "milestone.edit",
                method: "PATCH",
                path_template: "/api/v1/organizations/{organization}/milestones/{milestoneId}",
                path: format!("/api/v1/organizations/{org}/milestones/{id}"),
                resolved: json!({ "organization": org, "milestone": id }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .update_organization_milestone(&org, id, &body, &if_match, &idempotency)
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
        "milestone.edit",
        id,
        revision_before,
        Some(response.value.revision),
        response.request_id.as_deref(),
    );
    let milestone = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_milestone(session, &milestone)
    })
}

async fn transitions(session: &mut Session<'_>, id: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .list_organization_milestone_transitions(&org, id)
        .await
        .map_err(CliError::from_client)?;
    let list: OrganizationMilestoneTransitionList = response.value.clone();
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
                session
                    .out
                    .line(&format!("  {}", transition.target_state))
                    .map_err(CliError::general)?;
            }
        }
        Ok(())
    })
}

async fn transition(
    session: &mut Session<'_>,
    id: &str,
    target: &str,
    reason: Option<&str>,
    force: bool,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;

    let (if_match, revision_before) = if force {
        ("*".to_string(), None)
    } else {
        let current = api
            .get_organization_milestone(&org, id, MilestoneDetailQuery::default())
            .await
            .map_err(CliError::from_client)?;
        let etag = current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?;
        (etag, Some(current.value.milestone.revision))
    };

    let body = TransitionOrganizationMilestoneRequest {
        target_state: target.to_string(),
        reason: reason.map(str::to_string),
    };
    let idempotency = idem_key(idempotency_key)?;

    // Client-side prevalidation improves UX only; the server remains
    // authoritative for whether the transition is legal (SPEC §48).
    if !force {
        let allowed = api
            .list_organization_milestone_transitions(&org, id)
            .await
            .map_err(CliError::from_client)?;
        let permitted = allowed
            .value
            .transitions
            .iter()
            .any(|transition| transition.target_state == target);
        if !permitted {
            let options: Vec<&str> = allowed
                .value
                .transitions
                .iter()
                .map(|transition| transition.target_state.as_str())
                .collect();
            return Err(CliError::usage(format!(
                "milestone transition to {target} is not allowed from {}; available: {}",
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
                operation: "milestone.transition",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/milestones/{milestoneId}/transitions",
                path: format!("/api/v1/organizations/{org}/milestones/{id}/transitions"),
                resolved: json!({
                    "organization": org,
                    "milestone": id,
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
        .transition_organization_milestone(&org, id, &body, &if_match, &idempotency)
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
        "milestone.transition",
        id,
        revision_before,
        Some(response.value.revision),
        response.request_id.as_deref(),
    );
    let milestone = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_milestone(session, &milestone)
    })
}

const RELEASES_COLUMNS: [&str; 8] = [
    "ID", "NAME", "STATE", "PROJECT", "TARGET", "RELEASED", "ITEMS", "DONE",
];

fn release_rows(releases: &[hamstik_api_client::OrganizationMilestoneRelease]) -> Vec<Vec<String>> {
    releases
        .iter()
        .map(|release| {
            vec![
                release.id.clone(),
                release.name.clone(),
                release.state.clone(),
                release.project_key.clone(),
                dash(release.target_date.as_deref()),
                dash(release.release_date.as_deref()),
                release.total_work_items.to_string(),
                release.completed_work_items.to_string(),
            ]
        })
        .collect()
}

async fn releases(
    session: &mut Session<'_>,
    id: &str,
    release_after: Option<&str>,
    pagination: &PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let query = MilestoneReleasesQuery {
        cursor: release_after.map(str::to_string),
        limit: pagination.directory_page_size(),
    };
    let response = api
        .list_organization_milestone_releases(&org, id, query)
        .await
        .map_err(CliError::from_client)?;

    if pagination.all && response.value.page.has_more {
        let scope = response.value.scope.clone();
        let page = follow_with(follow_policy(pagination), {
            let fetch_api = api.clone();
            let org = org.clone();
            move |cursor| {
                let fetch_api = fetch_api.clone();
                let org = org.clone();
                async move {
                    let response = fetch_api
                        .list_organization_milestone_releases(
                            &org,
                            id,
                            MilestoneReleasesQuery {
                                cursor,
                                ..MilestoneReleasesQuery::default()
                            },
                        )
                        .await?;
                    Ok(PageItems::new(
                        response.value.items,
                        &response.raw,
                        Page {
                            limit: response.value.page.limit,
                            has_more: response.value.page.has_more,
                            next_cursor: response.value.page.next_after,
                        },
                    ))
                }
            }
        })
        .await
        .map_err(CliError::from_client)?;
        let rows = release_rows(&page.items);
        let json_value = json!({ "items": page.raw_items, "page": page.page, "scope": scope });
        check_columns(&RELEASES_COLUMNS, &session.output_options())?;
        render_list(session, &json_value, &RELEASES_COLUMNS, &rows)
    } else {
        let rows = release_rows(&response.value.items);
        let page_raw = response
            .raw
            .get("page")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let json_value = json!({
            "items": response.raw.get("items").cloned().unwrap_or_else(|| json!([])),
            "page": page_raw,
            "scope": response.value.scope,
        });
        check_columns(&RELEASES_COLUMNS, &session.output_options())?;
        render_list(session, &json_value, &RELEASES_COLUMNS, &rows)
    }
}
#[allow(clippy::too_many_arguments)]
async fn mutate_release(
    session: &mut Session<'_>,
    id: &str,
    release_id: &str,
    mode: &str,
    confirm_remove: bool,
    reason: Option<&str>,
    force: bool,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    if mode == "remove" && !confirm_remove {
        return Err(CliError::usage(
            "removing a release from a milestone requires --confirm-remove",
        ));
    }
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;

    let (if_match, revision_before) = if force {
        ("*".to_string(), None)
    } else {
        let current = api
            .get_organization_milestone(&org, id, MilestoneDetailQuery::default())
            .await
            .map_err(CliError::from_client)?;
        let etag = current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?;
        (etag, Some(current.value.milestone.revision))
    };
    let body = MutateOrganizationMilestoneReleaseRequest {
        release_version_id: release_id.to_string(),
        mode: mode.to_string(),
        confirm_remove: confirm_remove.then_some(true),
        reason: reason.map(str::to_string),
    };
    let idempotency = idem_key(idempotency_key)?;
    let operation = format!("milestone.release.{mode}");

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: &operation,
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/milestones/{milestoneId}/releases",
                path: format!("/api/v1/organizations/{org}/milestones/{id}/releases"),
                resolved: json!({
                    "organization": org,
                    "milestone": id,
                    "release": release_id,
                    "mode": mode,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .mutate_organization_milestone_release(&org, id, &body, &if_match, &idempotency)
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
        &operation,
        id,
        revision_before,
        Some(response.value.revision),
        response.request_id.as_deref(),
    );
    let milestone = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_milestone(session, &milestone)
    })
}

async fn events(
    session: &mut Session<'_>,
    id: &str,
    before_event_id: Option<&str>,
    limit: Option<u32>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .list_organization_milestone_events(
            &org,
            id,
            MilestoneEventsQuery {
                before_event_id: before_event_id.map(str::to_string),
                limit,
            },
        )
        .await
        .map_err(CliError::from_client)?;
    let items: Vec<OrganizationMilestoneEvent> = response.value.items.clone();
    emit_view(session, &response.raw, id, |session| {
        if items.is_empty() {
            session.out.line("(no events)").map_err(CliError::general)?;
        }
        for event in &items {
            let line = serde_json::to_string(event).map_err(CliError::general)?;
            session
                .out
                .line(&format!("  {line}"))
                .map_err(CliError::general)?;
        }
        if let Some(next) = &response.value.next_before_event_id {
            session
                .out
                .line(&format!(
                    "older events available; pass --before-event-id {next}"
                ))
                .map_err(CliError::general)?;
        }
        Ok(())
    })
}
