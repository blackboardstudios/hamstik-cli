// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work edit`.

use hamstik_api_client::UpdateWorkItemRequest;

use crate::app::Session;
use crate::args::WorkEditArgs;
use crate::error::CliError;
use crate::time_arg;

use super::common::read_long_text;
use super::dryrun;
use super::emit_view;

pub(super) async fn edit(session: &mut Session<'_>, args: &WorkEditArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    time_arg::report_resolved(&mut session.out, &[("--due-date", args.due_date.as_ref())]);
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
        due_date: tri(args.clear_due_date, args.due_date.map(|d| d.to_string())),
    };
    if body.is_empty() {
        return Err(CliError::usage("no changes specified"));
    }

    let (if_match, revision_before) = if args.force {
        ("*".to_string(), None)
    } else {
        let current = api
            .get_work_item(&org, &project, &args.key)
            .await
            .map_err(CliError::from_client)?;
        let etag = current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?;
        (etag, Some(current.value.revision))
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
                resolved: serde_json::json!({
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
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "work.edit",
        &args.key,
        revision_before,
        Some(response.value.revision),
        response.request_id.as_deref(),
    );
    let item = response.value.clone();
    emit_view(session, &response.raw, &item.key.clone(), |session| {
        super::view::render_work_item(session, &item, false)
    })
}

/// Builds a tri-state field: `None` (unset), `Some(None)` (clear), or `Some(Some(v))`.
pub(super) fn tri<T>(clear: bool, value: Option<T>) -> Option<Option<T>> {
    if clear { Some(None) } else { value.map(Some) }
}
