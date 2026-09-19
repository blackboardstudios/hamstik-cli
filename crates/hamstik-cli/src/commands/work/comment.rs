// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work comment add|edit|delete`.

use serde_json::{Value, json};

use hamstik_api_client::{
    CreateCommentRequest, ListOptions, PageItems, UpdateCommentRequest, follow_with,
};

use crate::app::Session;
use crate::args::{CommentArgs, CommentCommand};
use crate::error::CliError;

use super::common::{idem_key, read_long_text};
use super::dryrun;
use super::emit_json;
use super::emit_table;
use super::emit_view;
use super::follow_policy;
pub(super) async fn comment(session: &mut Session<'_>, args: &CommentArgs) -> Result<(), CliError> {
    match &args.command {
        CommentCommand::List {
            key,
            exclude_deleted,
            pagination,
        } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let api = session.api(&selection)?;
            let base = ListOptions {
                limit: pagination.page_size(),
                cursor: pagination.cursor.clone(),
            };
            let (comments, json_value) = if pagination.all {
                let fetch_api = api.clone();
                let org = org.clone();
                let project = project.clone();
                let key = key.clone();
                let page = follow_with(follow_policy(pagination), move |cursor| {
                    let fetch_api = fetch_api.clone();
                    let org = org.clone();
                    let project = project.clone();
                    let key = key.clone();
                    let mut opts = base.clone();
                    opts.cursor = cursor;
                    async move {
                        let response = fetch_api.list_comments(&org, &project, &key, opts).await?;
                        Ok(PageItems::new(
                            response.value.items,
                            &response.raw,
                            response.value.page,
                        ))
                    }
                })
                .await
                .map_err(CliError::from_client)?;
                let json_value = json!({ "items": page.raw_items, "page": page.page });
                (page.items, json_value)
            } else {
                let response = api
                    .list_comments(&org, &project, key, base)
                    .await
                    .map_err(CliError::from_client)?;
                (response.value.items, response.raw)
            };
            // Soft-deleted comments stay in the thread to preserve reply
            // structure; `--exclude-deleted` filters them from the rendering.
            // The filter runs after `--limit`, which counts what the API
            // returned rather than what survives the filter.
            let items: Vec<_> = if *exclude_deleted {
                comments.iter().filter(|comment| !comment.deleted).collect()
            } else {
                comments.iter().collect()
            };
            let rows: Vec<Vec<String>> = items
                .iter()
                .map(|c| {
                    let body = c.body.clone().unwrap_or_else(|| "(deleted)".to_string());
                    let preview: String = body.chars().take(60).collect();
                    vec![
                        c.id.clone(),
                        c.parent_comment_id
                            .clone()
                            .unwrap_or_else(|| "-".to_string()),
                        c.author.name().to_string(),
                        c.created_at.clone(),
                        preview,
                    ]
                })
                .collect();
            emit_table(
                session,
                &json_value,
                &["ID", "PARENT", "AUTHOR", "CREATED", "BODY"],
                &rows,
            )
        }
        CommentCommand::Add {
            key,
            body,
            body_file,
            body_editor,
            parent,
            idempotency_key,
        } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let text = read_long_text(
                session,
                body.clone(),
                body_file.as_deref(),
                *body_editor,
                "a comment",
            )?
            .ok_or_else(|| {
                CliError::usage("missing comment body; use --body, --body-file, or --body-editor")
            })?;
            if text.trim().is_empty() {
                return Err(CliError::usage("comment body must not be empty"));
            }
            let idempotency = idem_key(idempotency_key.clone())?;

            if session.global.dry_run {
                return dryrun::emit_preview(
                    session,
                    dryrun::PreviewRequest {
                        operation: "work.comment.add",
                        method: "POST",
                        path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/comments",
                        path: format!(
                            "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/comments"
                        ),
                        resolved: json!({
                            "organization": org,
                            "project": project,
                            "workItem": key,
                            "parentCommentId": parent,
                        }),
                        if_match: None,
                        idempotency_key: Some(&idempotency),
                        body: Some(json!({ "body": text, "parentCommentId": parent })),
                        notes: Vec::new(),
                    },
                );
            }

            let api = session.api(&selection)?;
            let response = api
                .create_comment(
                    &org,
                    &project,
                    key,
                    &CreateCommentRequest {
                        body: text,
                        parent_comment_id: parent.clone(),
                    },
                    &idempotency,
                )
                .await
                .map_err(CliError::from_client)?;
            if response.idempotency_replayed {
                session
                    .out
                    .warn("note: request replayed (idempotent duplicate)");
            }
            let comment_id = response.value.id.clone();
            emit_view(session, &response.raw, &comment_id.clone(), |session| {
                render_comment(session, &response.raw)
            })
        }
        CommentCommand::Edit {
            key,
            comment_id,
            body,
            body_file,
            body_editor,
            idempotency_key,
        } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let text = read_long_text(
                session,
                body.clone(),
                body_file.as_deref(),
                *body_editor,
                "a comment edit",
            )?
            .ok_or_else(|| {
                CliError::usage("missing comment body; use --body, --body-file, or --body-editor")
            })?;
            if text.trim().is_empty() {
                return Err(CliError::usage("comment body must not be empty"));
            }
            let idempotency = idem_key(idempotency_key.clone())?;

            if session.global.dry_run {
                return dryrun::emit_preview(
                    session,
                    dryrun::PreviewRequest {
                        operation: "work.comment.edit",
                        method: "PATCH",
                        path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/comments/{commentId}",
                        path: format!(
                            "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/comments/{comment_id}"
                        ),
                        resolved: json!({
                            "organization": org,
                            "project": project,
                            "workItem": key,
                            "commentId": comment_id,
                        }),
                        if_match: None,
                        idempotency_key: Some(&idempotency),
                        body: Some(json!({ "body": text })),
                        notes: Vec::new(),
                    },
                );
            }

            let api = session.api(&selection)?;
            let response = api
                .update_comment(
                    &org,
                    &project,
                    key,
                    comment_id,
                    &UpdateCommentRequest { body: text },
                    &idempotency,
                )
                .await
                .map_err(CliError::from_client)?;
            if response.idempotency_replayed {
                session
                    .out
                    .warn("note: request replayed (idempotent duplicate)");
            }
            emit_view(session, &response.raw, comment_id, |session| {
                render_comment(session, &response.raw)
            })
        }
        CommentCommand::Delete { key, comment_id } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;

            if session.global.dry_run {
                return dryrun::emit_preview(
                    session,
                    dryrun::PreviewRequest {
                        operation: "work.comment.delete",
                        method: "DELETE",
                        path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/comments/{commentId}",
                        path: format!(
                            "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/comments/{comment_id}"
                        ),
                        resolved: json!({
                            "organization": org,
                            "project": project,
                            "workItem": key,
                            "commentId": comment_id,
                        }),
                        if_match: None,
                        idempotency_key: None,
                        body: None,
                        notes: Vec::new(),
                    },
                );
            }

            let api = session.api(&selection)?;
            api.delete_comment(&org, &project, key, comment_id)
                .await
                .map_err(CliError::from_client)?;
            if session.json() {
                emit_json(
                    session,
                    &json!({ "deleted": true, "commentId": comment_id }),
                )
            } else {
                session
                    .out
                    .line(&format!("Deleted comment {comment_id}"))
                    .map_err(CliError::general)
            }
        }
    }
}

fn render_comment(session: &mut Session<'_>, raw: &Value) -> Result<(), CliError> {
    let body = raw
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or("(no body)");
    session.out.line(body).map_err(CliError::general)?;
    Ok(())
}
