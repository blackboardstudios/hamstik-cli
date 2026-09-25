// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik release` (list / view / create / edit / transitions / transition /
//! archive / restore / scope / item / bulk-membership / announcement / audit).

use serde_json::{Value, json};

use hamstik_api_client::{
    AnnouncementDraftRevisionRequest, BulkReleaseMembershipRequest, CreateReleaseVersionRequest,
    EditReleaseAnnouncementDraftRequest, GenerateReleaseAnnouncementDraftRequest,
    GenerateReleaseAuditReportRequest, ListReleaseAuditReportsOptions, ListReleaseVersionsOptions,
    MutateWorkItemReleaseVersionsRequest, Page, PageItems, PublishReleaseAnnouncementRequest,
    ReleaseScopeQuery, ReleaseVersion, ReleaseVersionLifecycleRequest,
    ReleaseVersionTransitionList, TransitionReleaseVersionRequest, UpdateReleaseVersionRequest,
    follow_with, generate_key, validate_key,
};

use crate::app::Session;
use crate::args::{
    PaginationArgs, ReleaseAnnouncementArgs, ReleaseAnnouncementCommand,
    ReleaseAnnouncementDraftCommand, ReleaseArgs, ReleaseAuditArgs, ReleaseAuditCommand,
    ReleaseAuditFormatArg, ReleaseAuditKindArg, ReleaseCommand, ReleaseItemCommand,
};
use crate::error::CliError;
use crate::input::{resolve_json, resolve_text};
use crate::time_arg;
use crate::time_arg::TimeArg;

use super::dryrun;
use super::org::render_lines;
use super::sprint::require_project;
use super::{check_columns, emit_json, emit_view, follow_policy, render_list};

/// Runs the `release` subcommands.
pub async fn run(session: &mut Session<'_>, args: &ReleaseArgs) -> Result<(), CliError> {
    match &args.command {
        ReleaseCommand::List {
            project,
            state,
            include_archived,
            pagination,
        } => {
            list(
                session,
                project.as_deref(),
                state.map(super::super::args::ReleaseStateArg::as_str),
                *include_archived,
                pagination,
            )
            .await
        }
        ReleaseCommand::View { id, project } => view(session, id, project.as_deref()).await,
        ReleaseCommand::Create {
            name,
            display_version,
            description,
            description_file,
            description_editor,
            owner,
            target_date,
            release_date,
            project,
            idempotency_key,
        } => {
            create(
                session,
                name.as_deref(),
                display_version.as_deref(),
                description.as_deref(),
                description_file.as_deref(),
                *description_editor,
                owner.as_deref(),
                target_date.as_ref(),
                release_date.as_ref(),
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        ReleaseCommand::Edit {
            id,
            name,
            display_version,
            clear_display_version,
            description,
            description_file,
            description_editor,
            clear_description,
            owner,
            target_date,
            clear_target_date,
            release_date,
            clear_release_date,
            force,
            project,
            idempotency_key,
        } => {
            edit(
                session,
                id,
                name.as_deref(),
                display_version.as_deref(),
                *clear_display_version,
                description.as_deref(),
                description_file.as_deref(),
                *description_editor,
                *clear_description,
                owner.as_deref(),
                target_date.as_ref(),
                *clear_target_date,
                release_date.as_ref(),
                *clear_release_date,
                *force,
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        ReleaseCommand::Transitions { id, project } => {
            transitions(session, id, project.as_deref()).await
        }
        ReleaseCommand::Transition {
            id,
            target,
            confirm_incomplete_scope,
            reason,
            force,
            project,
            idempotency_key,
        } => {
            transition(
                session,
                id,
                target.as_str(),
                *confirm_incomplete_scope,
                reason.as_deref(),
                *force,
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        ReleaseCommand::Archive {
            id,
            reason,
            force,
            project,
            idempotency_key,
        } => {
            lifecycle(
                session,
                id,
                true,
                reason.as_deref(),
                *force,
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        ReleaseCommand::Restore {
            id,
            reason,
            force,
            project,
            idempotency_key,
        } => {
            lifecycle(
                session,
                id,
                false,
                reason.as_deref(),
                *force,
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        ReleaseCommand::Scope {
            id,
            search,
            status,
            item_type,
            priority,
            assignee,
            sort,
            direction,
            project,
            pagination,
        } => {
            scope(
                session,
                id,
                search.as_deref(),
                status.as_deref(),
                item_type.as_deref(),
                priority.as_deref(),
                assignee.as_deref(),
                sort.as_deref(),
                direction.as_deref(),
                project.as_deref(),
                pagination,
            )
            .await
        }
        ReleaseCommand::Item(args) => run_item(session, args).await,
        ReleaseCommand::BulkMembership {
            file,
            project,
            idempotency_key,
        } => {
            bulk_membership(
                session,
                file,
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        ReleaseCommand::Announcement(args) => run_announcement(session, args).await,
        ReleaseCommand::Audit(args) => run_audit(session, args).await,
    }
}

async fn run_item(
    session: &mut Session<'_>,
    args: &crate::args::ReleaseItemArgs,
) -> Result<(), CliError> {
    match &args.command {
        ReleaseItemCommand::List { key, project } => {
            item_list(session, key, project.as_deref()).await
        }
        ReleaseItemCommand::Add {
            key,
            release_ids,
            confirm_released_scope_correction,
            reason,
            force,
            project,
            idempotency_key,
        } => {
            item_mutate(
                session,
                key,
                "add",
                release_ids,
                *confirm_released_scope_correction,
                reason.as_deref(),
                *force,
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        ReleaseItemCommand::Remove {
            key,
            release_ids,
            confirm_released_scope_correction,
            reason,
            force,
            project,
            idempotency_key,
        } => {
            item_mutate(
                session,
                key,
                "remove",
                release_ids,
                *confirm_released_scope_correction,
                reason.as_deref(),
                *force,
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        ReleaseItemCommand::Replace {
            key,
            release_ids,
            confirm_released_scope_correction,
            reason,
            force,
            project,
            idempotency_key,
        } => {
            item_mutate(
                session,
                key,
                "replace",
                release_ids,
                *confirm_released_scope_correction,
                reason.as_deref(),
                *force,
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
    }
}

async fn run_announcement(
    session: &mut Session<'_>,
    args: &ReleaseAnnouncementArgs,
) -> Result<(), CliError> {
    match &args.command {
        ReleaseAnnouncementCommand::Current {
            release_id,
            project,
        } => announcement_current(session, release_id, project.as_deref()).await,
        ReleaseAnnouncementCommand::Revision {
            release_id,
            revision,
            project,
        } => announcement_revision(session, release_id, *revision, project.as_deref()).await,
        ReleaseAnnouncementCommand::Draft(draft) => match &draft.command {
            ReleaseAnnouncementDraftCommand::Show {
                release_id,
                project,
            } => draft_show(session, release_id, project.as_deref()).await,
            ReleaseAnnouncementDraftCommand::Generate {
                release_id,
                revision,
                confirm_replace_organization,
                project,
                idempotency_key,
            } => {
                draft_generate(
                    session,
                    release_id,
                    *revision,
                    *confirm_replace_organization,
                    project.as_deref(),
                    idempotency_key.as_deref(),
                )
                .await
            }
            ReleaseAnnouncementDraftCommand::Edit {
                release_id,
                file,
                revision,
                project,
            } => draft_edit(session, release_id, file, *revision, project.as_deref()).await,
            ReleaseAnnouncementDraftCommand::Discard {
                release_id,
                revision,
                project,
            } => draft_discard(session, release_id, *revision, project.as_deref()).await,
        },
        ReleaseAnnouncementCommand::Publish {
            release_id,
            revision,
            confirm_empty,
            reason,
            project,
            idempotency_key,
        } => {
            announcement_publish(
                session,
                release_id,
                *revision,
                *confirm_empty,
                reason.as_deref(),
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
    }
}

async fn run_audit(session: &mut Session<'_>, args: &ReleaseAuditArgs) -> Result<(), CliError> {
    match &args.command {
        ReleaseAuditCommand::Generate {
            kind,
            release,
            from,
            through,
            project,
            idempotency_key,
        } => {
            audit_generate(
                session,
                *kind,
                release.as_deref(),
                from.as_ref(),
                through.as_ref(),
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
        ReleaseAuditCommand::Get {
            report_id,
            format,
            output,
            project,
        } => {
            audit_get(
                session,
                report_id,
                *format,
                output.as_deref(),
                project.as_deref(),
            )
            .await
        }
        ReleaseAuditCommand::List {
            release,
            project,
            pagination,
        } => audit_list(session, release.as_deref(), project.as_deref(), pagination).await,
    }
}

pub(crate) fn idem_key(flag: Option<&str>) -> Result<String, CliError> {
    match flag {
        Some(key) => {
            validate_key(key).map_err(|err| CliError::usage(err.to_string()))?;
            Ok(key.to_string())
        }
        None => Ok(generate_key()),
    }
}

pub(crate) fn dash(value: Option<&str>) -> String {
    value.unwrap_or("-").to_string()
}

fn release_row(release: &ReleaseVersion) -> Vec<String> {
    vec![
        release.id.clone(),
        release.name.clone(),
        dash(release.display_version.as_deref()),
        release.state.clone(),
        dash(release.target_date.as_deref()),
        dash(release.release_date.as_deref()),
        release.revision.to_string(),
    ]
}

const RELEASE_COLUMNS: [&str; 7] = [
    "ID", "NAME", "DISPLAY", "STATE", "TARGET", "RELEASED", "REVISION",
];

async fn list(
    session: &mut Session<'_>,
    project_flag: Option<&str>,
    state: Option<&str>,
    include_archived: Option<bool>,
    pagination: &PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;
    let opts = ListReleaseVersionsOptions {
        limit: pagination.directory_page_size(),
        cursor: pagination.cursor.clone(),
        state: state.map(str::to_string),
        include_archived,
    };

    if pagination.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let project = project.clone();
            let opts = ListReleaseVersionsOptions {
                cursor,
                ..opts.clone()
            };
            async move {
                let response = fetch_api
                    .list_release_versions(&org, &project, opts)
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
        finish_release_list(session, &page)
    } else {
        let response = api
            .list_release_versions(&org, &project, opts)
            .await
            .map_err(CliError::from_client)?;
        finish_release_list(
            session,
            &PageItems::new(response.value.items, &response.raw, response.value.page),
        )
    }
}

fn finish_release_list(
    session: &mut Session<'_>,
    page: &PageItems<ReleaseVersion>,
) -> Result<(), CliError> {
    let rows: Vec<Vec<String>> = page.items.iter().map(release_row).collect();
    let json_value = json!({ "items": page.raw_items, "page": page.page });
    check_columns(&RELEASE_COLUMNS, &session.output_options())?;
    render_list(session, &json_value, &RELEASE_COLUMNS, &rows)
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
        .get_release_version(&org, &project, id)
        .await
        .map_err(CliError::from_client)?;
    let release = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_release(session, &release)
    })
}

fn render_release(session: &mut Session<'_>, release: &ReleaseVersion) -> Result<(), CliError> {
    let lines = [
        ("name", release.name.clone()),
        ("id", release.id.clone()),
        ("display", dash(release.display_version.as_deref())),
        ("state", release.state.clone()),
        ("description", dash(release.description.as_deref())),
        (
            "owner",
            release
                .owner_name
                .clone()
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("target", dash(release.target_date.as_deref())),
        ("released", dash(release.release_date.as_deref())),
        ("archived", dash(release.archived_at.as_deref())),
        ("revision", release.revision.to_string()),
        ("created", release.created_at.clone()),
        ("updated", release.updated_at.clone()),
    ];
    render_lines(session, &lines)
}

/// Resolves an optional owner argument (`ME|NONE|ID`) into the body shape.
pub(crate) async fn owner_assignment(
    session: &mut Session<'_>,
    owner: Option<&str>,
) -> Result<Option<Option<String>>, CliError> {
    let Some(owner) = owner else {
        return Ok(None);
    };
    let resolved = super::resolve_user_arg(session, owner).await?;
    if resolved.eq_ignore_ascii_case("none") {
        Ok(Some(None))
    } else {
        Ok(Some(Some(resolved)))
    }
}

/// Reads the description tri-state for create (`Some(Some(text))` or absent).
pub(crate) fn create_description(
    description: Option<&str>,
    description_file: Option<&str>,
    description_editor: bool,
    session: &Session<'_>,
) -> Result<Option<Option<String>>, CliError> {
    if description_editor {
        return Ok(Some(Some(crate::editor::edit_text(
            session.env,
            session.configured_editor()?.as_deref(),
            session.global.no_input,
            "a release description",
        )?)));
    }
    Ok(resolve_text(
        description.map(str::to_string),
        description_file,
        &mut std::io::stdin(),
    )
    .map_err(|err| CliError::general(format!("cannot read text: {err}")))?
    .map(Some))
}

/// Reads the description tri-state for edit, including explicit clears.
#[allow(clippy::too_many_arguments)]
pub(crate) fn edit_description(
    description: Option<&str>,
    description_file: Option<&str>,
    description_editor: bool,
    clear: bool,
    session: &Session<'_>,
    what: &str,
) -> Result<Option<Option<String>>, CliError> {
    if clear {
        return Ok(Some(None));
    }
    if description_editor {
        return Ok(Some(Some(crate::editor::edit_text(
            session.env,
            session.configured_editor()?.as_deref(),
            session.global.no_input,
            what,
        )?)));
    }
    Ok(resolve_text(
        description.map(str::to_string),
        description_file,
        &mut std::io::stdin(),
    )
    .map_err(|err| CliError::general(format!("cannot read text: {err}")))?
    .map(Some))
}

#[allow(clippy::too_many_arguments)]
async fn create(
    session: &mut Session<'_>,
    name: Option<&str>,
    display_version: Option<&str>,
    description: Option<&str>,
    description_file: Option<&str>,
    description_editor: bool,
    owner: Option<&str>,
    target_date: Option<&TimeArg>,
    release_date: Option<&TimeArg>,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    time_arg::report_resolved(
        &mut session.out,
        &[
            ("--target-date", target_date),
            ("--release-date", release_date),
        ],
    );
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;

    let name = match name {
        Some(name) => name.to_string(),
        None if session.can_prompt() => session
            .prompt
            .read_line("Release name: ")
            .map_err(|err| CliError::general(format!("prompt failed: {err}")))?,
        None => return Err(CliError::usage("missing required option --name")),
    };
    if name.trim().is_empty() {
        return Err(CliError::usage("name must not be empty"));
    }

    let owner_id = owner_assignment(session, owner).await?;
    let body = CreateReleaseVersionRequest {
        name,
        display_version: display_version.map(|v| Some(v.to_string())),
        description: create_description(
            description,
            description_file,
            description_editor,
            session,
        )?,
        owner_id,
        target_date: target_date.map(|d| Some(d.to_string())),
        release_date: release_date.map(|d| Some(d.to_string())),
    };
    let idempotency = idem_key(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "release.create",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/releases",
                path: format!("/api/v1/organizations/{org}/projects/{project}/releases"),
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
        .create_release_version(&org, &project, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let release = response.value.clone();
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "release.create",
        &release.id,
        None,
        Some(release.revision),
        response.request_id.as_deref(),
    );
    emit_view(session, &response.raw, &release.id, |session| {
        render_release(session, &release)
    })
}

#[allow(clippy::too_many_arguments)]
async fn edit(
    session: &mut Session<'_>,
    id: &str,
    name: Option<&str>,
    display_version: Option<&str>,
    clear_display_version: bool,
    description: Option<&str>,
    description_file: Option<&str>,
    description_editor: bool,
    clear_description: bool,
    owner: Option<&str>,
    target_date: Option<&TimeArg>,
    clear_target_date: bool,
    release_date: Option<&TimeArg>,
    clear_release_date: bool,
    force: bool,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    time_arg::report_resolved(
        &mut session.out,
        &[
            ("--target-date", target_date),
            ("--release-date", release_date),
        ],
    );
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    let owner_id = owner_assignment(session, owner).await?;
    let body = UpdateReleaseVersionRequest {
        name: name.map(|v| Some(v.to_string())),
        display_version: if clear_display_version {
            Some(None)
        } else {
            display_version.map(|v| Some(v.to_string()))
        },
        description: edit_description(
            description,
            description_file,
            description_editor,
            clear_description,
            session,
            "a release description edit",
        )?,
        owner_id,
        target_date: match (target_date, clear_target_date) {
            (Some(date), _) => Some(Some(date.to_string())),
            (None, true) => Some(None),
            (None, false) => None,
        },
        release_date: match (release_date, clear_release_date) {
            (Some(date), _) => Some(Some(date.to_string())),
            (None, true) => Some(None),
            (None, false) => None,
        },
    };
    if body.is_empty() {
        return Err(CliError::usage("no changes specified"));
    }

    let (if_match, revision_before) = if force {
        ("*".to_string(), None)
    } else {
        let current = api
            .get_release_version(&org, &project, id)
            .await
            .map_err(CliError::from_client)?;
        let etag = current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?;
        (etag, Some(current.value.revision))
    };
    let idempotency = idem_key(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "release.edit",
                method: "PATCH",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/releases/{releaseId}",
                path: format!("/api/v1/organizations/{org}/projects/{project}/releases/{id}"),
                resolved: json!({ "organization": org, "project": project, "release": id }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .update_release_version(&org, &project, id, &body, &if_match, &idempotency)
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
        "release.edit",
        id,
        revision_before,
        Some(response.value.revision),
        response.request_id.as_deref(),
    );
    let release = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_release(session, &release)
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
        .list_release_version_transitions(&org, &project, id)
        .await
        .map_err(CliError::from_client)?;
    let list: ReleaseVersionTransitionList = response.value.clone();
    render_transition_list(session, &response.raw, id, &list)
}

fn render_transition_list(
    session: &mut Session<'_>,
    raw: &Value,
    id: &str,
    list: &ReleaseVersionTransitionList,
) -> Result<(), CliError> {
    emit_view(session, raw, id, |session| {
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

#[allow(clippy::too_many_arguments)]
async fn transition(
    session: &mut Session<'_>,
    id: &str,
    target: &str,
    confirm_incomplete_scope: bool,
    reason: Option<&str>,
    force: bool,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    let (if_match, revision_before) = if force {
        ("*".to_string(), None)
    } else {
        let current = api
            .get_release_version(&org, &project, id)
            .await
            .map_err(CliError::from_client)?;
        let etag = current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?;
        (etag, Some(current.value.revision))
    };

    let body = TransitionReleaseVersionRequest {
        target_state: target.to_string(),
        confirm_incomplete_scope: confirm_incomplete_scope.then_some(true),
        reason: reason.map(str::to_string),
    };
    let idempotency = idem_key(idempotency_key)?;

    // Client-side prevalidation improves UX only; the server remains
    // authoritative for whether the transition is legal (SPEC §48).
    if !force {
        let allowed = api
            .list_release_version_transitions(&org, &project, id)
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
                "release transition to {target} is not allowed from {}; available: {}",
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
                operation: "release.transition",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/releases/{releaseId}/transitions",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/releases/{id}/transitions"
                ),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "release": id,
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
        .transition_release_version(&org, &project, id, &body, &if_match, &idempotency)
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
        "release.transition",
        id,
        revision_before,
        Some(response.value.revision),
        response.request_id.as_deref(),
    );
    let release = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_release(session, &release)
    })
}

/// Archives or restores a Release Version with its ETag.
async fn lifecycle(
    session: &mut Session<'_>,
    id: &str,
    archive: bool,
    reason: Option<&str>,
    force: bool,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    let (if_match, revision_before) = if force {
        ("*".to_string(), None)
    } else {
        let current = api
            .get_release_version(&org, &project, id)
            .await
            .map_err(CliError::from_client)?;
        let etag = current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?;
        (etag, Some(current.value.revision))
    };
    let body = ReleaseVersionLifecycleRequest {
        reason: reason.map(str::to_string),
    };
    let idempotency = idem_key(idempotency_key)?;
    let (operation, suffix) = if archive {
        ("release.archive", "/archive")
    } else {
        ("release.restore", "/restore")
    };

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation,
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/releases/{releaseId}/archive",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/releases/{id}{suffix}"
                ),
                resolved: json!({ "organization": org, "project": project, "release": id }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = if archive {
        api.archive_release_version(&org, &project, id, &body, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?
    } else {
        api.restore_release_version(&org, &project, id, &body, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?
    };
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        operation,
        id,
        revision_before,
        Some(response.value.revision),
        response.request_id.as_deref(),
    );
    let release = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_release(session, &release)
    })
}

#[allow(clippy::too_many_arguments)]
async fn scope(
    session: &mut Session<'_>,
    id: &str,
    search: Option<&str>,
    status: Option<&str>,
    item_type: Option<&str>,
    priority: Option<&str>,
    assignee: Option<&str>,
    sort: Option<&str>,
    direction: Option<&str>,
    project_flag: Option<&str>,
    pagination: &PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;
    let query = ReleaseScopeQuery {
        limit: pagination.directory_page_size(),
        cursor: pagination.cursor.clone(),
        query: search.map(str::to_string),
        status: status.map(str::to_string),
        item_type: item_type.map(str::to_string),
        priority: priority.map(str::to_string),
        assignee_id: assignee.map(str::to_string),
        sort: sort.map(str::to_string),
        direction: direction.map(str::to_string),
    };
    let response = api
        .get_release_version_scope(&org, &project, id, query)
        .await
        .map_err(CliError::from_client)?;

    if pagination.all && response.value.page.has_more {
        let release_json = response.value.release.clone();
        let page = follow_with(follow_policy(pagination), {
            let fetch_api = api.clone();
            let org = org.clone();
            let project = project.clone();
            move |cursor| {
                let fetch_api = fetch_api.clone();
                let org = org.clone();
                let project = project.clone();
                async move {
                    let response = fetch_api
                        .get_release_version_scope(
                            &org,
                            &project,
                            id,
                            ReleaseScopeQuery {
                                cursor,
                                ..ReleaseScopeQuery::default()
                            },
                        )
                        .await?;
                    Ok(PageItems::new(
                        response.value.items,
                        &response.raw,
                        Page {
                            limit: response.value.page.limit,
                            has_more: response.value.page.has_more,
                            next_cursor: response.value.page.next_cursor,
                        },
                    ))
                }
            }
        })
        .await
        .map_err(CliError::from_client)?;
        let mut raw = json!({ "items": page.raw_items, "page": page.page });
        raw["release"] = release_json;
        render_scope(session, &raw, &page.items)
    } else {
        render_scope(session, &response.raw, &response.value.items)
    }
}

const SCOPE_COLUMNS: [&str; 6] = ["KEY", "TITLE", "TYPE", "STATUS", "PRIORITY", "ASSIGNEE"];

fn render_scope(
    session: &mut Session<'_>,
    raw: &Value,
    items: &[hamstik_api_client::ReleaseVersionScopeItem],
) -> Result<(), CliError> {
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            vec![
                item.key.clone(),
                item.title.clone(),
                item.item_type.clone(),
                item.status.clone(),
                item.priority.clone(),
                item.assignee_id.clone().unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();
    check_columns(&SCOPE_COLUMNS, &session.output_options())?;
    render_list(session, raw, &SCOPE_COLUMNS, &rows)
}

async fn item_list(
    session: &mut Session<'_>,
    key: &str,
    project_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;
    let response = api
        .list_work_item_release_versions(&org, &project, key)
        .await
        .map_err(CliError::from_client)?;
    let memberships = response.value.clone();
    emit_view(session, &response.raw, key, |session| {
        session
            .out
            .line(&format!(
                "work item revision: {}",
                memberships.work_item_revision
            ))
            .map_err(CliError::general)?;
        if memberships.release_versions.is_empty() {
            session
                .out
                .line("(no release memberships)")
                .map_err(CliError::general)?;
        }
        for release in &memberships.release_versions {
            session
                .out
                .line(&format!(
                    "  {}  {}  {}  {}",
                    release.id,
                    release.name,
                    release.state,
                    dash(release.display_version.as_deref()),
                ))
                .map_err(CliError::general)?;
        }
        Ok(())
    })
}

/// Resolves the item ETag (or `*`) for a membership mutation.
async fn work_item_if_match(
    api: &std::sync::Arc<dyn hamstik_api_client::HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
    force: bool,
) -> Result<(String, Option<i64>), CliError> {
    if force {
        return Ok(("*".to_string(), None));
    }
    let current = api
        .get_work_item(org, project, key)
        .await
        .map_err(CliError::from_client)?;
    let etag = current
        .etag
        .ok_or_else(|| CliError::protocol("server did not return an ETag; re-run with --force"))?;
    Ok((etag, Some(current.value.revision)))
}

#[allow(clippy::too_many_arguments)]
async fn item_mutate(
    session: &mut Session<'_>,
    key: &str,
    mode: &str,
    release_ids: &[String],
    confirm_released_scope_correction: bool,
    reason: Option<&str>,
    force: bool,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    if release_ids.is_empty() {
        return Err(CliError::usage("at least one release id is required"));
    }
    if let Some(reason) = reason
        && reason.trim().is_empty()
    {
        return Err(CliError::usage("reason must not be empty when given"));
    }
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    let (if_match, revision_before) = work_item_if_match(&api, &org, &project, key, force).await?;
    let body = MutateWorkItemReleaseVersionsRequest {
        mode: mode.to_string(),
        release_version_ids: release_ids.to_vec(),
        confirm_released_scope_correction: confirm_released_scope_correction.then_some(true),
        reason: reason.map(str::to_string),
    };
    let idempotency = idem_key(idempotency_key)?;
    let operation = format!("release.item.{mode}");

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: &operation,
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/releases",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/releases"
                ),
                resolved: json!({ "organization": org, "project": project, "workItem": key }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .mutate_work_item_release_versions(&org, &project, key, &body, &if_match, &idempotency)
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
        key,
        revision_before,
        Some(response.value.work_item_revision),
        response.request_id.as_deref(),
    );
    let memberships = response.value.clone();
    emit_view(session, &response.raw, key, |session| {
        session
            .out
            .line(&format!(
                "work item revision: {}",
                memberships.work_item_revision
            ))
            .map_err(CliError::general)?;
        if memberships.release_versions.is_empty() {
            session
                .out
                .line("(no release memberships)")
                .map_err(CliError::general)?;
        }
        for release in &memberships.release_versions {
            session
                .out
                .line(&format!("  {}  {}", release.id, release.name))
                .map_err(CliError::general)?;
        }
        Ok(())
    })
}

async fn bulk_membership(
    session: &mut Session<'_>,
    file: &str,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;

    let body: BulkReleaseMembershipRequest =
        resolve_json(file, &mut std::io::stdin()).map_err(CliError::usage)?;
    if body.operations.is_empty() || body.operations.len() > 50 {
        return Err(CliError::usage(
            "the bulk envelope accepts between 1 and 50 operations",
        ));
    }
    for operation in &body.operations {
        if operation.release_version_ids.len() > 100 {
            return Err(CliError::usage(
                "each operation accepts at most 100 release ids",
            ));
        }
    }
    let idempotency = idem_key(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "release.bulk-membership",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/release-memberships/bulk",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/release-memberships/bulk"
                ),
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
        .bulk_mutate_release_memberships(&org, &project, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    if response.value.partial {
        session
            .out
            .warn("note: some operations failed; inspect results");
    }
    crate::audit::record(
        &session.config,
        &mut session.out,
        "release.bulk-membership",
        &project,
        response.request_id.as_deref(),
    );
    if session.json() {
        emit_json(session, &response.raw)
    } else {
        for result in &response.value.results {
            let status = result.status;
            let key = result
                .work_item_key
                .clone()
                .unwrap_or_else(|| "-".to_string());
            match (&result.error, &result.release_versions) {
                (Some(error), _) => session
                    .out
                    .line(&format!(
                        "  [{status}] {key}: {} {}",
                        error.code, error.message
                    ))
                    .map_err(CliError::general)?,
                (None, Some(releases)) => session
                    .out
                    .line(&format!(
                        "  [{status}] {key}: {} release(s)",
                        releases.len()
                    ))
                    .map_err(CliError::general)?,
                (None, None) => session
                    .out
                    .line(&format!("  [{status}] {key}"))
                    .map_err(CliError::general)?,
            }
        }
        Ok(())
    }
}

async fn announcement_current(
    session: &mut Session<'_>,
    release_id: &str,
    project_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;
    let response = api
        .get_current_release_announcement(&org, &project, release_id)
        .await
        .map_err(CliError::from_client)?;
    let announcement = response.value.clone();
    emit_view(session, &response.raw, &announcement.id, |session| {
        render_announcement_markdown(session, &announcement.markdown)
    })
}

fn render_announcement_markdown(session: &mut Session<'_>, markdown: &str) -> Result<(), CliError> {
    for line in markdown.lines() {
        session.out.line(line).map_err(CliError::general)?;
    }
    Ok(())
}

async fn announcement_revision(
    session: &mut Session<'_>,
    release_id: &str,
    revision: i64,
    project_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;
    let response = api
        .get_release_announcement_revision(&org, &project, release_id, revision)
        .await
        .map_err(CliError::from_client)?;
    let announcement = response.value.clone();
    emit_view(session, &response.raw, &announcement.id, |session| {
        render_announcement_markdown(session, &announcement.markdown)
    })
}

async fn draft_show(
    session: &mut Session<'_>,
    release_id: &str,
    project_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;
    let response = api
        .get_release_announcement_draft(&org, &project, release_id)
        .await
        .map_err(CliError::from_client)?;
    let draft = response.value.clone();
    emit_view(session, &response.raw, release_id, |session| {
        render_draft(session, &draft)
    })
}

fn render_draft(
    session: &mut Session<'_>,
    draft: &hamstik_api_client::ReleaseAnnouncementDraft,
) -> Result<(), CliError> {
    let lines = [
        ("release", draft.release_version_id.clone()),
        ("revision", draft.revision.to_string()),
        ("source revision", draft.source_release_revision.to_string()),
        ("generated", draft.generated_at.clone()),
        ("updated", draft.updated_at.clone()),
    ];
    render_lines(session, &lines)?;
    session
        .out
        .line(&format!("\nintroduction:\n{}", draft.introduction))
        .map_err(CliError::general)?;
    if !draft.highlights.is_empty() {
        session
            .out
            .line("\nhighlights:")
            .map_err(CliError::general)?;
        for highlight in &draft.highlights {
            session
                .out
                .line(&format!("  - {highlight}"))
                .map_err(CliError::general)?;
        }
    }
    session.out.line("\nitems:").map_err(CliError::general)?;
    for item in &draft.items {
        let included = if item.include { "x" } else { " " };
        session
            .out
            .line(&format!(
                "  [{included}] {} {} ({})",
                item.key, item.title, item.status
            ))
            .map_err(CliError::general)?;
    }
    if let Some(diff) = &draft.diff {
        session
            .out
            .line(&format!(
                "\nscope drift since generation: +{} added, -{} removed, ~{} changed",
                diff.added.len(),
                diff.removed.len(),
                diff.changed.len()
            ))
            .map_err(CliError::general)?;
    }
    Ok(())
}

async fn draft_generate(
    session: &mut Session<'_>,
    release_id: &str,
    revision: Option<i64>,
    confirm_replace_organization: bool,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let body = GenerateReleaseAnnouncementDraftRequest {
        revision,
        confirm_replace_organization: confirm_replace_organization.then_some(true),
    };
    let idempotency = idem_key(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "release.announcement.draft.generate",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/releases/{releaseId}/announcements/draft",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/releases/{release_id}/announcements/draft"
                ),
                resolved: json!({ "organization": org, "project": project, "release": release_id }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let api = session.api(&selection)?;
    let response = api
        .generate_release_announcement_draft(&org, &project, release_id, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    crate::audit::record(
        &session.config,
        &mut session.out,
        "release.announcement.draft.generate",
        release_id,
        response.request_id.as_deref(),
    );
    let draft = response.value.clone();
    emit_view(session, &response.raw, release_id, |session| {
        render_draft(session, &draft)
    })
}

/// The announcement draft edit body as authored in a JSON file.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DraftEditInput {
    introduction: String,
    #[serde(default)]
    highlights: Vec<String>,
    #[serde(default)]
    categories: Vec<hamstik_api_client::ReleaseAnnouncementCategory>,
    #[serde(default)]
    included_work_item_ids: Vec<String>,
}

async fn draft_edit(
    session: &mut Session<'_>,
    release_id: &str,
    file: &str,
    revision: Option<i64>,
    project_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    let draft_revision = match revision {
        Some(revision) => revision,
        None => {
            let current = api
                .get_release_announcement_draft(&org, &project, release_id)
                .await
                .map_err(CliError::from_client)?;
            current.value.revision
        }
    };
    let input: DraftEditInput =
        resolve_json(file, &mut std::io::stdin()).map_err(CliError::usage)?;
    if input.highlights.len() > 50 {
        return Err(CliError::usage("at most 50 highlights are accepted"));
    }
    if input.categories.len() > 50 {
        return Err(CliError::usage("at most 50 categories are accepted"));
    }
    if input.included_work_item_ids.len() > 10_000 {
        return Err(CliError::usage(
            "at most 10,000 included Work Items are accepted",
        ));
    }
    let body = EditReleaseAnnouncementDraftRequest {
        revision: draft_revision,
        introduction: input.introduction,
        highlights: input.highlights,
        categories: input.categories,
        included_work_item_ids: input.included_work_item_ids,
    };

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "release.announcement.draft.edit",
                method: "PATCH",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/releases/{releaseId}/announcements/draft",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/releases/{release_id}/announcements/draft"
                ),
                resolved: json!({ "organization": org, "project": project, "release": release_id }),
                if_match: None,
                idempotency_key: None,
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .edit_release_announcement_draft(&org, &project, release_id, &body)
        .await
        .map_err(CliError::from_client)?;
    crate::audit::record(
        &session.config,
        &mut session.out,
        "release.announcement.draft.edit",
        release_id,
        response.request_id.as_deref(),
    );
    let draft = response.value.clone();
    emit_view(session, &response.raw, release_id, |session| {
        render_draft(session, &draft)
    })
}

async fn draft_discard(
    session: &mut Session<'_>,
    release_id: &str,
    revision: Option<i64>,
    project_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    let draft_revision = match revision {
        Some(revision) => revision,
        None => {
            let current = api
                .get_release_announcement_draft(&org, &project, release_id)
                .await
                .map_err(CliError::from_client)?;
            current.value.revision
        }
    };
    let body = AnnouncementDraftRevisionRequest {
        revision: draft_revision,
    };

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "release.announcement.draft.discard",
                method: "DELETE",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/releases/{releaseId}/announcements/draft",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/releases/{release_id}/announcements/draft"
                ),
                resolved: json!({ "organization": org, "project": project, "release": release_id }),
                if_match: None,
                idempotency_key: None,
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .archive_release_announcement_draft(&org, &project, release_id, &body)
        .await
        .map_err(CliError::from_client)?;
    crate::audit::record(
        &session.config,
        &mut session.out,
        "release.announcement.draft.discard",
        release_id,
        response.request_id.as_deref(),
    );
    session
        .out
        .line(&format!("draft {draft_revision} discarded"))
        .map_err(CliError::general)
}

async fn announcement_publish(
    session: &mut Session<'_>,
    release_id: &str,
    revision: Option<i64>,
    confirm_empty: bool,
    reason: Option<&str>,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    let draft_revision = match revision {
        Some(revision) => revision,
        None => {
            let current = api
                .get_release_announcement_draft(&org, &project, release_id)
                .await
                .map_err(CliError::from_client)?;
            current.value.revision
        }
    };
    let body = PublishReleaseAnnouncementRequest {
        revision: draft_revision,
        confirm_empty: confirm_empty.then_some(true),
        reason: reason.map(str::to_string),
    };
    let idempotency = idem_key(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "release.announcement.publish",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/releases/{releaseId}/announcements/publish",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/releases/{release_id}/announcements/publish"
                ),
                resolved: json!({ "organization": org, "project": project, "release": release_id }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .publish_release_announcement(&org, &project, release_id, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    crate::audit::record(
        &session.config,
        &mut session.out,
        "release.announcement.publish",
        release_id,
        response.request_id.as_deref(),
    );
    emit_view(session, &response.raw, &response.value.id, |session| {
        session
            .out
            .line(&format!(
                "published revision {} ({})",
                response.value.revision, response.value.id
            ))
            .map_err(CliError::general)
    })
}

async fn audit_generate(
    session: &mut Session<'_>,
    kind: ReleaseAuditKindArg,
    release: Option<&str>,
    from: Option<&TimeArg>,
    through: Option<&TimeArg>,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    time_arg::report_resolved(
        &mut session.out,
        &[("--from", from), ("--through", through)],
    );
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;

    let body = match kind {
        ReleaseAuditKindArg::Dossier => {
            let release_id = release.ok_or_else(|| {
                CliError::usage("dossier packages require --release <RELEASE_ID>")
            })?;
            GenerateReleaseAuditReportRequest::Dossier {
                release_version_id: release_id.to_string(),
            }
        }
        ReleaseAuditKindArg::Register => {
            let from = from.ok_or_else(|| CliError::usage("register packages require --from"))?;
            let through =
                through.ok_or_else(|| CliError::usage("register packages require --through"))?;
            GenerateReleaseAuditReportRequest::Register {
                from: from.to_string(),
                through: through.to_string(),
            }
        }
    };
    if kind == ReleaseAuditKindArg::Dossier && (from.is_some() || through.is_some()) {
        return Err(CliError::usage(
            "--from/--through apply to register packages only",
        ));
    }
    let idempotency = idem_key(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "release.audit.generate",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/release-audit-reports",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/release-audit-reports"
                ),
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
        .generate_release_audit_report(&org, &project, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    crate::audit::record(
        &session.config,
        &mut session.out,
        "release.audit.generate",
        &response.value.id,
        response.request_id.as_deref(),
    );
    emit_view(session, &response.raw, &response.value.id, |session| {
        render_audit_summary(session, &response.value)
    })
}

fn render_audit_summary(
    session: &mut Session<'_>,
    summary: &hamstik_api_client::ReleaseAuditReportSummary,
) -> Result<(), CliError> {
    let lines = [
        ("id", summary.id.clone()),
        ("kind", summary.kind.clone()),
        ("sha256", summary.sha256.clone()),
        ("generated", summary.generated_at.clone()),
    ];
    render_lines(session, &lines)
}

async fn audit_get(
    session: &mut Session<'_>,
    report_id: &str,
    format: ReleaseAuditFormatArg,
    output: Option<&std::path::Path>,
    project_flag: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    match format {
        ReleaseAuditFormatArg::Json => {
            if output.is_some() {
                return Err(CliError::usage(
                    "--output applies to --audit-format csv only",
                ));
            }
            let response = api
                .get_release_audit_report(&org, &project, report_id)
                .await
                .map_err(CliError::from_client)?;
            if session.json() {
                emit_json(session, &response.raw)
            } else {
                let pretty =
                    serde_json::to_string_pretty(&response.raw).map_err(CliError::general)?;
                session.out.line(&pretty).map_err(CliError::general)
            }
        }
        ReleaseAuditFormatArg::Csv => {
            let Some(output) = output else {
                return Err(CliError::usage(
                    "--audit-format csv requires --output <PATH> (the package is binary)",
                ));
            };
            let download = api
                .download_release_audit_report(&org, &project, report_id)
                .await
                .map_err(CliError::from_client)?;
            crate::fsutil::write_atomic(output, &download.bytes).map_err(|err| {
                CliError::general(format!("cannot write {}: {err}", output.display()))
            })?;
            session
                .out
                .line(&format!(
                    "wrote {} bytes to {}",
                    download.bytes.len(),
                    output.display()
                ))
                .map_err(CliError::general)
        }
    }
}

const AUDIT_COLUMNS: [&str; 4] = ["ID", "KIND", "SHA256", "GENERATED"];

async fn audit_list(
    session: &mut Session<'_>,
    release: Option<&str>,
    project_flag: Option<&str>,
    pagination: &PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;
    let opts = ListReleaseAuditReportsOptions {
        limit: pagination.directory_page_size(),
        cursor: pagination.cursor.clone(),
        release_version_id: release.map(str::to_string),
    };
    let response = api
        .list_release_audit_reports(&org, &project, opts)
        .await
        .map_err(CliError::from_client)?;
    let rows: Vec<Vec<String>> = response
        .value
        .items
        .iter()
        .map(|report| {
            vec![
                report.id.clone(),
                report.kind.clone(),
                report.sha256.clone(),
                report.generated_at.clone(),
            ]
        })
        .collect();
    check_columns(&AUDIT_COLUMNS, &session.output_options())?;
    render_list(session, &response.raw, &AUDIT_COLUMNS, &rows)
}
