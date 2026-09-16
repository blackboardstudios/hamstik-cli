// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work view` / `work create` / `work edit`.

use serde_json::json;

use hamstik_api_client::{CreateWorkItemRequest, UpdateWorkItemRequest, WorkItem};

use crate::app::Session;
use crate::args::{WorkCreateArgs, WorkEditArgs};
use crate::error::CliError;

use super::common::{assignee_fields, idem_key, read_long_text};
use super::dryrun;
use super::emit_view;
use super::render_lines;
pub(super) async fn view(session: &mut Session<'_>, key: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .get_work_item(&org, &project, key)
        .await
        .map_err(CliError::from_client)?;
    let item = response.value.clone();
    emit_view(session, &response.raw, key, |session| {
        render_work_item(session, &item)
    })
}

pub(super) fn render_work_item(session: &mut Session<'_>, item: &WorkItem) -> Result<(), CliError> {
    let lines = [
        ("key", item.key.clone()),
        ("title", item.title.clone()),
        ("type", item.item_type.clone()),
        ("status", item.status.clone()),
        ("priority", item.priority.clone()),
        (
            "assignee",
            item.assignee
                .clone()
                .map(|a| a.name().to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "sprint",
            item.sprint
                .clone()
                .map(|s| s.name)
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "parent",
            item.parent
                .clone()
                .map(|p| p.key)
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "story points",
            item.story_points
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "due date",
            item.due_date.clone().unwrap_or_else(|| "-".to_string()),
        ),
        ("revision", item.revision.to_string()),
    ];
    render_lines(session, &lines)?;
    if let Some(description) = &item.description {
        session.out.line("").map_err(CliError::general)?;
        session.out.line(description).map_err(CliError::general)?;
    }
    Ok(())
}

pub(super) async fn create(
    session: &mut Session<'_>,
    args: &WorkCreateArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;

    let title = match &args.title {
        Some(title) => title.clone(),
        None if session.can_prompt() => session
            .prompt
            .read_line("Title: ")
            .map_err(|err| CliError::general(format!("prompt failed: {err}")))?,
        None => return Err(CliError::usage("missing required option --title")),
    };
    if title.trim().is_empty() {
        return Err(CliError::usage("title must not be empty"));
    }

    let description = read_long_text(
        session,
        args.description.clone(),
        args.description_file.as_deref(),
        args.description_editor,
        "a work item description",
    )?;
    let assignee = match &args.assignee {
        Some(value) => Some(super::super::resolve_user_arg(session, value).await?),
        None => None,
    };
    let (assignee_id, assignee_public_id) = assignee_fields(&assignee);
    let body = CreateWorkItemRequest {
        title,
        description,
        item_type: args.item_type.map(|t| t.as_str().to_string()),
        status: args.status.map(|s| s.as_str().to_string()),
        priority: args.priority.map(|p| p.as_str().to_string()),
        assignee_id,
        assignee_public_id,
        sprint_id: args.sprint.clone(),
        // The current API accepts the parent identifier as a bounded string;
        // forward it unchanged and leave lookup/validation to the server.
        parent_id: args.parent.clone(),
        story_points: args.story_points,
        due_date: args.due_date.clone(),
    };
    let idempotency = idem_key(args.idempotency_key.clone())?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "work.create",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items",
                path: format!("/api/v1/organizations/{org}/projects/{project}/work-items"),
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
        .create_work_item(&org, &project, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let item = response.value.clone();
    emit_view(session, &response.raw, &item.key.clone(), |session| {
        render_work_item(session, &item)
    })
}

pub(super) async fn edit(session: &mut Session<'_>, args: &WorkEditArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let description = if args.clear_description {
        Some(None)
    } else {
        read_long_text(
            session,
            args.description.clone(),
            args.description_file.as_deref(),
            args.description_editor,
            "a work item description edit",
        )?
        .map(Some)
    };
    // A public ID (`usr_...`) must go to `assigneePublicId`; everything else
    // (legacy UUID) goes to `assigneeId`. Clearing targets whichever field the
    // caller named. `me` is resolved to the caller's public ID first.
    let resolved_assignee = match &args.assignee {
        Some(value) if !value.eq_ignore_ascii_case("none") => {
            Some(super::super::resolve_user_arg(session, value).await?)
        }
        other => other.clone(),
    };
    let (assignee_id, assignee_public_id) = if args.clear_assignee {
        match &resolved_assignee {
            Some(id) if id.starts_with("usr_") => (None, Some(None)),
            Some(id) => (Some(Some(id.clone())), None),
            None => (Some(None), None),
        }
    } else {
        match &resolved_assignee {
            Some(id) if id.starts_with("usr_") => (None, Some(Some(id.clone()))),
            Some(id) if id.eq_ignore_ascii_case("none") => (Some(None), None),
            Some(id) => (Some(Some(id.clone())), None),
            None => (None, None),
        }
    };

    let body = UpdateWorkItemRequest {
        title: args.title.clone(),
        description,
        item_type: args.item_type.map(|t| t.as_str().to_string()),
        priority: args.priority.map(|p| p.as_str().to_string()),
        assignee_id,
        assignee_public_id,
        sprint_id: tri(args.clear_sprint, args.sprint.clone()),
        parent_id: tri(args.clear_parent, args.parent.clone()),
        story_points: tri(args.clear_story_points, args.story_points),
        due_date: tri(args.clear_due_date, args.due_date.clone()),
    };
    if body.is_empty() {
        return Err(CliError::usage("no changes specified"));
    }

    // Dry-run may perform the safe ETag read; only the mutation is withheld.
    // With `--force` no request at all is made and the preview shows the
    // explicit `If-Match: *` last-write-wins header.
    let if_match = if args.force {
        "*".to_string()
    } else {
        let current = api
            .get_work_item(&org, &project, &args.key)
            .await
            .map_err(CliError::from_client)?;
        current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?
    };

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "work.edit",
                method: "PATCH",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/work-items/{}",
                    args.key
                ),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "workItem": args.key,
                }),
                if_match: Some(&if_match),
                idempotency_key: None,
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .update_work_item(&org, &project, &args.key, &body, &if_match)
        .await
        .map_err(CliError::from_client)?;
    let item = response.value.clone();
    emit_view(session, &response.raw, &item.key.clone(), |session| {
        render_work_item(session, &item)
    })
}
/// Builds a tri-state field: `None` (unset), `Some(None)` (clear), or `Some(Some(v))`.
/// Builds a tri-state field: `None` (unset), `Some(None)` (clear), or `Some(Some(v))`.
pub(super) fn tri<T>(clear: bool, value: Option<T>) -> Option<Option<T>> {
    if clear { Some(None) } else { value.map(Some) }
}
