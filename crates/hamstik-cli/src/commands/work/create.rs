// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work create`.

use hamstik_api_client::CreateWorkItemRequest;

use crate::app::Session;
use crate::args::WorkCreateArgs;
use crate::error::CliError;
use crate::time_arg;
use serde_json::json;

use super::common::{assignee_fields, idem_key, read_long_text};
use super::dryrun;
use super::emit_view;

pub(super) async fn create(
    session: &mut Session<'_>,
    args: &WorkCreateArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    time_arg::report_resolved(&mut session.out, &[("--due-date", args.due_date.as_ref())]);
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
        parent_id: args.parent.clone(),
        story_points: args.story_points,
        due_date: args.due_date.map(|d| d.to_string()),
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
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "work.create",
        &item.key.clone(),
        None,
        Some(item.revision),
        response.request_id.as_deref(),
    );
    emit_view(session, &response.raw, &item.key.clone(), |session| {
        super::view::render_work_item(session, &item, false)
    })
}
