// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work create`.

use hamstik_api_client::{AttachLabelRequest, CreateWorkItemRequest};

use crate::app::{Selection, Session};
use crate::args::WorkCreateArgs;
use crate::error::CliError;
use crate::time_arg;
use serde_json::json;

use super::common::{assignee_fields, attribute_changes, idem_key, read_long_text};
use super::dryrun;
use super::emit_view;
use super::template::{self, LabelRef, StartPoint};

pub(super) async fn create(
    session: &mut Session<'_>,
    args: &WorkCreateArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    time_arg::report_resolved(&mut session.out, &[("--due-date", args.due_date.as_ref())]);
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;

    // Resolve the convenience source before prompting so its values can fill in
    // fields the user did not pass explicitly. `--from` performs a safe read;
    // `--template` is entirely local. The two flags are mutually exclusive and
    // `clap` rejects the combination before this function runs, so no network
    // call can happen for an invalid invocation.
    let start = resolve_start_point(session, &selection, &org, &project, args).await?;

    let title = match args
        .title
        .clone()
        .or_else(|| start.as_ref().and_then(|start| start.title.clone()))
    {
        Some(title) => title,
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
    )?
    .or_else(|| start.as_ref().and_then(|start| start.description.clone()));

    // Only the documented allow-list is copied; identity, ownership, workflow
    // state, revision, and scheduling fields are never taken from the source.
    let item_type = args
        .item_type
        .map(|item_type| item_type.as_str().to_string())
        .or_else(|| start.as_ref().and_then(|start| start.item_type.clone()));
    let priority = args
        .priority
        .map(|priority| priority.as_str().to_string())
        .or_else(|| start.as_ref().and_then(|start| start.priority.clone()));
    let labels: Vec<LabelRef> = start
        .as_ref()
        .map(|start| start.labels.clone())
        .unwrap_or_default();

    let assignee = match &args.assignee {
        Some(value) => Some(super::super::resolve_user_arg(session, value).await?),
        None => None,
    };
    let (assignee_id, assignee_public_id) = assignee_fields(&assignee);
    let attributes = attribute_changes(&args.attribute_options, &args.attribute_booleans, &[])?;
    let body = CreateWorkItemRequest {
        title,
        description,
        item_type,
        status: args.status.map(|status| status.as_str().to_string()),
        priority,
        assignee_id,
        assignee_public_id,
        sprint_id: args.sprint.clone(),
        parent_id: args.parent.clone(),
        story_points: args.story_points,
        due_date: args.due_date.map(|date| date.to_string()),
        attributes,
    };
    let idempotency = idem_key(args.idempotency_key.clone())?;

    if session.global.dry_run {
        // The create body cannot carry labels (the frozen Public API v1
        // contract forbids unknown properties), so a preview names the labels
        // that a real invocation attaches after the create succeeds.
        let resolved_labels: Vec<&str> = labels.iter().map(LabelRef::display).collect();
        let mut resolved = json!({ "organization": org, "project": project });
        if start.is_some() {
            resolved["labels"] = json!(resolved_labels);
        }
        let label_note = (start.is_some() && !labels.is_empty()).then(|| {
            format!(
                "labels resolved from the convenience source are attached after create: {}",
                resolved_labels.join(", ")
            )
        });
        let notes = label_note.iter().map(String::as_str).collect();
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "work.create",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items",
                path: format!("/api/v1/organizations/{org}/projects/{project}/work-items"),
                resolved,
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes,
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
    let mut item = response.value.clone();
    let mut raw = response.raw.clone();
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "work.create",
        &item.key.clone(),
        None,
        Some(item.revision),
        response.request_id.as_deref(),
    );

    // Labels are the one allow-listed field the create body cannot express.
    // Attach them through the existing label endpoint, chaining the ETag from
    // each response so concurrency protection stays intact.
    let mut etag = response.etag.clone();
    let mut label_error: Option<CliError> = None;
    for label in &labels {
        let current_etag = match etag.clone() {
            Some(value) => value,
            None => api
                .get_work_item(&org, &project, &item.key)
                .await
                .map_err(CliError::from_client)?
                .etag
                .ok_or_else(|| {
                    CliError::protocol(
                        "server did not return an ETag for the new work item; labels were not attached",
                    )
                })?,
        };
        let attach_body = AttachLabelRequest {
            label_id: label.id.clone(),
            label: label.id.is_none().then(|| label.name.clone()),
        };
        let attach_key = idem_key(None)?;
        match api
            .attach_label(
                &org,
                &project,
                &item.key,
                &attach_body,
                &current_etag,
                &attach_key,
            )
            .await
        {
            Ok(attached) => {
                crate::audit::record_with_revisions(
                    &session.config,
                    &mut session.out,
                    "work.label.add",
                    &item.key.clone(),
                    Some(item.revision),
                    Some(attached.value.revision),
                    attached.request_id.as_deref(),
                );
                item = attached.value;
                raw = attached.raw;
                etag = attached.etag;
            }
            Err(err) => {
                label_error = Some(CliError::general(format!(
                    "created {} but could not attach label {}: {}",
                    item.key,
                    label.display(),
                    CliError::from_client(err)
                )));
                break;
            }
        }
    }

    emit_view(session, &raw, &item.key.clone(), |session| {
        super::view::render_work_item(session, &item, false)
    })?;
    if let Some(err) = label_error {
        return Err(err);
    }
    Ok(())
}

/// Resolves `--from`/`--template` into a [`StartPoint`], if either was given.
async fn resolve_start_point(
    session: &Session<'_>,
    selection: &Selection,
    org: &str,
    project: &str,
    args: &WorkCreateArgs,
) -> Result<Option<StartPoint>, CliError> {
    if let Some(key) = &args.from {
        let api = session.api(selection)?;
        let response = api
            .get_work_item(org, project, key)
            .await
            .map_err(CliError::from_client)?;
        Ok(Some(template::from_work_item(&response.value)))
    } else if let Some(file) = &args.template {
        Ok(Some(template::from_file(file)?))
    } else {
        Ok(None)
    }
}
