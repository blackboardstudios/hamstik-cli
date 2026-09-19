// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik user` (view / work / activity / avatar) — authenticated profile
//! reads over `profile:read`.

use serde_json::{Value, json};

use hamstik_api_client::{
    ActivityOptions, AvatarOptions, ListWorkItemsQuery, PageItems, follow_with,
};

use crate::app::Session;
use crate::args::{UserArgs, UserCommand, UserWorkArgs};
use crate::error::CliError;
use crate::time_arg::{self, TimeArg};

use super::org::render_lines;
use super::work::sort::apply_sort;
use super::{check_columns, emit_json, emit_view, follow_policy, render_list};

/// Runs the `user` subcommands.
pub async fn run(session: &mut Session<'_>, args: &UserArgs) -> Result<(), CliError> {
    match &args.command {
        UserCommand::View { public_id } => view(session, public_id).await,
        UserCommand::Work(work_args) => work(session, work_args).await,
        UserCommand::Activity {
            public_id,
            since,
            pagination,
        } => activity(session, public_id, since.as_ref(), pagination).await,
        UserCommand::Avatar {
            public_id,
            output,
            avatar_version,
            format,
            revision,
        } => {
            avatar(
                session,
                public_id,
                output.as_deref(),
                AvatarOptions {
                    version: avatar_version.clone(),
                    format: format.clone(),
                    revision: revision.clone(),
                },
            )
            .await
        }
    }
}

/// Resolves a profile target: `me` becomes the caller's public ID via `GET /me`.
async fn resolve_target(session: &Session<'_>, public_id: &str) -> Result<String, CliError> {
    if public_id.starts_with("usr_") {
        return Ok(public_id.to_string());
    }
    if public_id.eq_ignore_ascii_case("me") {
        return super::resolve_user_arg(session, "me").await;
    }
    Err(CliError::usage(format!(
        "invalid user argument {public_id:?}; expected `me` or a `usr_` public ID"
    )))
}

async fn view(session: &mut Session<'_>, public_id: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    let api = session.api(&selection)?;
    let target = resolve_target(session, public_id).await?;
    let response = api
        .get_user_profile(&target)
        .await
        .map_err(CliError::from_client)?;
    let profile = response.value.clone();
    emit_view(session, &response.raw, &target, |session| {
        let mut lines = vec![
            ("public id", profile.public_id.clone()),
            ("name", profile.name.clone()),
            ("joined", profile.joined_at.clone()),
            ("you", profile.is_current_user.to_string()),
            (
                "avatar",
                profile
                    .avatar_url
                    .clone()
                    .unwrap_or_else(|| "-".to_string()),
            ),
        ];
        for shared in &profile.shared_organizations {
            let username = shared.username.clone().unwrap_or_else(|| "-".to_string());
            lines.push((
                Box::leak(format!("user @ {}", shared.organization.slug).into_boxed_str()),
                username,
            ));
        }
        lines.push(("projects", profile.stats.projects.to_string()));
        lines.push(("assigned", profile.stats.work_items_assigned.to_string()));
        lines.push(("created", profile.stats.work_items_created.to_string()));
        lines.push(("completed", profile.stats.work_items_completed.to_string()));
        lines.push(("comments", profile.stats.comments.to_string()));
        render_lines(session, &lines)
    })
}

async fn work(session: &mut Session<'_>, args: &UserWorkArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    time_arg::report_resolved(
        &mut session.out,
        &[
            ("--updated-after", args.updated_after.as_ref()),
            ("--due-before", args.due_before.as_ref()),
            ("--due-after", args.due_after.as_ref()),
        ],
    );
    let api = session.api(&selection)?;
    let target = resolve_target(session, &args.public_id).await?;
    let base = ListWorkItemsQuery {
        limit: args.pagination.page_size(),
        cursor: args.pagination.cursor.clone(),
        involvement: args
            .involvement
            .iter()
            .map(|value| value.as_str().to_string())
            .collect(),
        organizations: args.org.clone(),
        projects: args.project.clone(),
        q: args.search.clone(),
        status: args
            .status
            .iter()
            .map(|value| value.as_str().to_string())
            .collect(),
        scope: args.scope.map(|value| value.as_str().to_string()),
        item_type: args
            .item_type
            .iter()
            .map(|value| value.as_str().to_string())
            .collect(),
        priority: args
            .priority
            .iter()
            .map(|value| value.as_str().to_string())
            .collect(),
        sprint: args.sprint.clone(),
        label: args.label.clone(),
        label_name: args.label_name.clone(),
        parent: args.parent.clone(),
        top_level: args.top_level,
        updated_after: args.updated_after.map(|v| v.to_string()),
        overdue: args.overdue,
        due_before: args.due_before.map(|v| v.to_string()),
        due_after: args.due_after.map(|v| v.to_string()),
        sort: args.sort.map(|value| value.as_str().to_string()),
        archived: args.archived,
        fields: args.fields.clone(),
        // Profile Work fixes the target as the assignee; the caller cannot
        // choose one.
        ..Default::default()
    };
    let mut json_value: Value = if args.pagination.all {
        let fetch_api = api.clone();
        let public_id = target.clone();
        let page = follow_with(follow_policy(&args.pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            let public_id = public_id.clone();
            let mut query = base.clone();
            query.cursor = cursor;
            async move {
                let response = fetch_api.list_user_profile_work(&public_id, query).await?;
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        })
        .await
        .map_err(CliError::from_client)?;
        json!({ "items": page.raw_items, "page": page.page })
    } else {
        let response = api
            .list_user_profile_work(&target, base)
            .await
            .map_err(CliError::from_client)?;
        response.raw
    };
    if let Some(sort) = args.sort {
        apply_sort(&mut json_value, sort);
    }
    let rows: Vec<Vec<String>> = json_value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(work_row_from_raw).collect())
        .unwrap_or_default();
    check_columns(
        &["KEY", "PROJECT", "TITLE", "STATUS", "ASSIGNEE", "REPORTER"],
        &session.output_options(),
    )?;
    render_list(
        session,
        &json_value,
        &["KEY", "PROJECT", "TITLE", "STATUS", "ASSIGNEE", "REPORTER"],
        &rows,
    )
}

fn work_row_from_raw(raw: &Value) -> Vec<String> {
    let project_key = raw
        .get("project")
        .and_then(|p| p.get("key"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let user_name = |value: Option<&Value>| -> String {
        value
            .and_then(|a| a.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string()
    };
    vec![
        raw.get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        project_key.to_string(),
        raw.get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        raw.get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        user_name(raw.get("assignee").filter(|a| !a.is_null())),
        user_name(raw.get("reporter").filter(|r| !r.is_null())),
    ]
}

async fn activity(
    session: &mut Session<'_>,
    public_id: &str,
    since: Option<&TimeArg>,
    pagination: &crate::args::PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let api = session.api(&selection)?;
    let target = resolve_target(session, public_id).await?;
    time_arg::report_resolved(&mut session.out, &[("--since", since)]);
    let base = ActivityOptions {
        limit: pagination.page_size(),
        cursor: pagination.cursor.clone(),
        since: since.map(|d| d.to_string()),
    };
    let json_value: Value = if pagination.all {
        let fetch_api = api.clone();
        let public_id = target.clone();
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            let public_id = public_id.clone();
            let mut opts = base.clone();
            opts.cursor = cursor;
            async move {
                let response = fetch_api
                    .list_user_profile_activity(&public_id, opts)
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
        json!({ "items": page.raw_items, "page": page.page })
    } else {
        let response = api
            .list_user_profile_activity(&target, base)
            .await
            .map_err(CliError::from_client)?;
        response.raw
    };
    let rows: Vec<Vec<String>> = json_value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(activity_row_from_raw).collect())
        .unwrap_or_default();
    check_columns(
        &[
            "ID",
            "ACTION",
            "ACTOR",
            "ORG",
            "PROJECT",
            "WORK ITEM",
            "CREATED",
        ],
        &session.output_options(),
    )?;
    render_list(
        session,
        &json_value,
        &[
            "ID",
            "ACTION",
            "ACTOR",
            "ORG",
            "PROJECT",
            "WORK ITEM",
            "CREATED",
        ],
        &rows,
    )
}

fn activity_row_from_raw(raw: &Value) -> Vec<String> {
    vec![
        raw.get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        raw.get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        raw.get("actor")
            .and_then(|a| a.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string(),
        raw.get("organization")
            .and_then(|o| o.get("slug"))
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string(),
        raw.get("project")
            .and_then(|p| p.get("key"))
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string(),
        raw.get("workItem")
            .and_then(|w| w.get("key"))
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string(),
        raw.get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    ]
}

async fn avatar(
    session: &mut Session<'_>,
    public_id: &str,
    output: Option<&str>,
    opts: AvatarOptions,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let api = session.api(&selection)?;
    let target_user = resolve_target(session, public_id).await?;
    let download = api
        .get_user_profile_avatar(&target_user, opts)
        .await
        .map_err(CliError::from_client)?;
    let target = match output {
        Some(path) => std::path::PathBuf::from(path),
        None => {
            let extension = match download.content_type.as_deref() {
                Some("image/png") => "png",
                Some("image/jpeg") => "jpg",
                Some("image/webp") => "webp",
                _ => "bin",
            };
            // Refuse to write outside the current directory implicitly: use
            // only the final path component of the derived name.
            let name = format!("{target_user}.{extension}");
            let safe = name.rsplit(['/', '\\']).next().unwrap_or(&name);
            std::path::PathBuf::from(safe)
        }
    };
    std::fs::write(&target, &download.bytes)
        .map_err(|err| CliError::general(format!("cannot write {}: {err}", target.display())))?;
    if session.json() {
        emit_json(
            session,
            &json!({
                "publicId": target_user,
                "path": target.display().to_string(),
                "size": download.bytes.len(),
                "contentType": download.content_type,
                "contentLength": download.content_length,
                "contentDisposition": download.content_disposition,
                "cacheControl": download.cache_control,
                "requestId": download.request_id,
            }),
        )
    } else {
        session
            .out
            .line(&format!(
                "Downloaded avatar for {target_user} ({} bytes) to {}",
                download.bytes.len(),
                target.display()
            ))
            .map_err(CliError::general)
    }
}
