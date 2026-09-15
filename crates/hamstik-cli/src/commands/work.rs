// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik work` (list / view / create / edit / transitions / transition /
//! start / close / comment).

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use hamstik_api_client::{
    ActivityOptions, AttachLabelRequest, BulkCreateEnvelope, BulkTransitionEnvelope,
    BulkUpdateEnvelope, CreateCommentRequest, CreateWorkItemLinkRequest, CreateWorkItemRequest,
    ListOptions, ListWorkItemsQuery, PageItems, SqueakQlSearchRequest, TransitionRequest,
    UpdateCommentRequest, UpdateWorkItemRequest, WatcherAction, WorkItem, WorkItemSummary,
    WorkItemWatcher, follow_all, generate_key, validate_key,
};

use crate::app::Session;
use crate::args::{
    CommentArgs, CommentCommand, MyWorkArgs, WorkArgs, WorkAttachmentArgs, WorkAttachmentCommand,
    WorkBulkArgs, WorkBulkCommand, WorkCommand, WorkCreateArgs, WorkEditArgs, WorkLabelArgs,
    WorkLabelCommand, WorkLinkArgs, WorkLinkCommand, WorkListArgs, WorkWatcherArgs,
    WorkWatcherCommand,
};
use crate::error::CliError;
use crate::input::resolve_text;

use super::bulk_preflight;
use super::dryrun;
use super::org::render_lines;
use super::{emit_json, emit_table, emit_view};

/// Runs the `work` subcommands.
pub async fn run(session: &mut Session<'_>, args: &WorkArgs) -> Result<(), CliError> {
    match &args.command {
        WorkCommand::List(list_args) => list(session, list_args).await,
        WorkCommand::Mine(mine_args) => mine(session, mine_args).await,
        WorkCommand::Search {
            query,
            file,
            saved,
            pagination,
        } => search(session, query, file, saved, pagination).await,
        WorkCommand::View { key } => view(session, key).await,
        WorkCommand::Create(create_args) => create(session, create_args).await,
        WorkCommand::Edit(edit_args) => edit(session, edit_args).await,
        WorkCommand::Watcher(watcher_args) => watcher(session, watcher_args).await,
        WorkCommand::Transitions { key } => transitions(session, key).await,
        WorkCommand::Transition { key, target } => {
            transition_to(session, key, target.as_str()).await
        }
        WorkCommand::Start { key } => transition_to(session, key, "in_progress").await,
        WorkCommand::Close { key } => transition_to(session, key, "done").await,
        WorkCommand::Label(label_args) => label(session, label_args).await,
        WorkCommand::Attachment(attachment_args) => attachment(session, attachment_args).await,
        WorkCommand::Comment(comment_args) => comment(session, comment_args).await,
        WorkCommand::Link(link_args) => link(session, link_args).await,
        WorkCommand::Activity {
            key,
            since,
            pagination,
        } => activity(session, key, since.as_deref(), pagination).await,
        WorkCommand::Archive {
            key,
            force,
            idempotency_key,
        } => change_archive(session, key, true, *force, idempotency_key.as_deref()).await,
        WorkCommand::Unarchive {
            key,
            force,
            idempotency_key,
        } => change_archive(session, key, false, *force, idempotency_key.as_deref()).await,
        WorkCommand::Delete {
            key,
            cascade,
            force,
            idempotency_key,
        } => delete(session, key, *cascade, *force, idempotency_key.as_deref()).await,
        WorkCommand::Bulk(bulk_args) => bulk(session, bulk_args).await,
    }
}

fn my_work_query(args: &MyWorkArgs) -> ListWorkItemsQuery {
    ListWorkItemsQuery {
        limit: args.pagination.limit,
        cursor: args.pagination.cursor.clone(),
        projects: args.project.clone(),
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
        label: args.label.clone(),
        label_name: args.label_name.clone(),
        overdue: args.overdue,
        due_before: args.due_before.clone(),
        due_after: args.due_after.clone(),
        sort: args.sort.map(|value| value.as_str().to_string()),
        archived: args.archived,
        fields: args.fields.clone(),
        ..Default::default()
    }
}

async fn mine(session: &mut Session<'_>, args: &MyWorkArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    let api = session.api(&selection)?;
    let base = my_work_query(args);
    let json_value = if args.pagination.all {
        let fetch_api = api.clone();
        let page = follow_all(move |cursor| {
            let fetch_api = fetch_api.clone();
            let mut query = base.clone();
            query.cursor = cursor;
            async move {
                let response = fetch_api.list_my_work(query).await?;
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
        api.list_my_work(base)
            .await
            .map_err(CliError::from_client)?
            .raw
    };
    render_context_work_items(session, &json_value)
}

async fn search(
    session: &mut Session<'_>,
    query: &Option<String>,
    file: &Option<String>,
    saved: &Option<String>,
    pagination: &crate::args::PaginationArgs,
) -> Result<(), CliError> {
    let expression = crate::commands::squeakql::resolve_expression(
        session,
        query.as_deref(),
        file.as_deref(),
        saved.as_deref(),
    )?;
    let query = expression.as_str();
    if query.trim().is_empty() {
        return Err(CliError::usage("SqueakQL query must not be empty"));
    }
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let base = SqueakQlSearchRequest {
        query: query.to_string(),
        limit: pagination.limit,
        cursor: pagination.cursor.clone(),
    };
    let json_value = if pagination.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let page = follow_all(move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let mut body = base.clone();
            body.cursor = cursor;
            async move {
                let response = fetch_api
                    .search_organization_work_items_with_squeakql(&org, &body)
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
        api.search_organization_work_items_with_squeakql(&org, &base)
            .await
            .map_err(CliError::from_client)?
            .raw
    };
    render_context_work_items(session, &json_value)
}

fn render_context_work_items(session: &mut Session<'_>, value: &Value) -> Result<(), CliError> {
    let rows = value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    vec![
                        raw_string(item, "key"),
                        item.get("project")
                            .and_then(|project| project.get("key"))
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        raw_string(item, "title"),
                        raw_string(item, "status"),
                        item.get("assignee")
                            .and_then(|assignee| assignee.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("-")
                            .to_string(),
                    ]
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    emit_table(
        session,
        value,
        &["KEY", "PROJECT", "TITLE", "STATUS", "ASSIGNEE"],
        &rows,
    )
}

fn raw_string(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Applies the shared Work Item filters to a query.
pub(crate) fn apply_filters(query: &mut ListWorkItemsQuery, filters: &crate::args::WorkFilters) {
    query.q = filters.search.clone();
    query.status = filters
        .status
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();
    query.scope = filters.scope.map(|s| s.as_str().to_string());
    query.item_type = filters
        .item_type
        .iter()
        .map(|t| t.as_str().to_string())
        .collect();
    query.priority = filters
        .priority
        .iter()
        .map(|p| p.as_str().to_string())
        .collect();
    query.assignee = filters.assignee_query();
    query.sprint = filters.sprint.clone();
    query.label = filters.label.clone();
    query.label_name = filters.label_name.clone();
    query.parent = filters.parent.clone();
    query.top_level = filters.top_level;
    query.updated_after = filters.updated_after.clone();
    query.overdue = filters.overdue;
    query.due_before = filters.due_before.clone();
    query.due_after = filters.due_after.clone();
    query.sort = filters.sort.map(|s| s.as_str().to_string());
    query.archived = filters.archived;
    query.fields = filters.fields.clone();
}

fn build_query(args: &WorkListArgs) -> ListWorkItemsQuery {
    let mut query = ListWorkItemsQuery {
        limit: args.pagination.limit,
        cursor: args.pagination.cursor.clone(),
        ..Default::default()
    };
    apply_filters(&mut query, &args.filters);
    query
}

async fn list(session: &mut Session<'_>, args: &WorkListArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    let query = build_query(args);

    if args.pagination.all {
        let org = org.clone();
        let project = project.clone();
        let base = query.clone();
        let fetch_api = api.clone();
        let page = follow_all(move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let project = project.clone();
            let mut query = base.clone();
            query.cursor = cursor;
            async move {
                let response = fetch_api.list_work_items(&org, &project, query).await?;
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        })
        .await
        .map_err(CliError::from_client)?;
        let rows: Vec<Vec<String>> = page.items.iter().map(summary_row).collect();
        let json_value = json!({ "items": page.raw_items, "page": page.page });
        emit_table(
            session,
            &json_value,
            &["KEY", "TITLE", "STATUS", "TYPE", "PRIORITY", "ASSIGNEE"],
            &rows,
        )
    } else {
        let response = api
            .list_work_items(&org, &project, query)
            .await
            .map_err(CliError::from_client)?;
        let rows: Vec<Vec<String>> = response.value.items.iter().map(summary_row).collect();
        emit_table(
            session,
            &response.raw,
            &["KEY", "TITLE", "STATUS", "TYPE", "PRIORITY", "ASSIGNEE"],
            &rows,
        )
    }
}

fn idem_key(flag: Option<String>) -> Result<String, CliError> {
    match flag {
        Some(key) => {
            validate_key(&key).map_err(|err| CliError::usage(err.to_string()))?;
            Ok(key)
        }
        None => Ok(generate_key()),
    }
}

fn idem_key_ref(flag: Option<&str>) -> Result<String, CliError> {
    match flag {
        Some(key) => {
            validate_key(key).map_err(|err| CliError::usage(err.to_string()))?;
            Ok(key.to_string())
        }
        None => Ok(generate_key()),
    }
}

fn read_text(inline: Option<String>, file: Option<&str>) -> Result<Option<String>, CliError> {
    let mut stdin = std::io::stdin();
    resolve_text(inline, file, &mut stdin)
        .map_err(|err| CliError::general(format!("cannot read text: {err}")))
}

/// Resolves long-form text from inline, `--*-file`/stdin, or the editor.
///
/// Source conflicts are already rejected at argument-parse time (`conflicts_with`),
/// so at most one source can be present here. `--editor` launches
/// `$VISUAL`/`$EDITOR` on a secure temporary file; the editor's content
/// becomes the value (see [`crate::editor::edit_text`]).
fn read_long_text(
    session: &Session<'_>,
    inline: Option<String>,
    file: Option<&str>,
    editor: bool,
    what: &str,
) -> Result<Option<String>, CliError> {
    if editor {
        return crate::editor::edit_text(session.env, session.global.no_input, what).map(Some);
    }
    read_text(inline, file)
}

/// Resolves the `--assignee` value into the preferred wire form: a `usr_`
/// public ID goes to `assigneePublicId`, everything else to legacy
/// `assigneeId` (UUID, `me`, or `none`).
fn assignee_fields(value: &Option<String>) -> (Option<String>, Option<String>) {
    match value {
        Some(id) if id.starts_with("usr_") => (None, Some(id.clone())),
        other => (other.clone(), None),
    }
}

/// True when `value` parses as a UUID (the wire form for label ids).
fn is_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}

fn summary_row(item: &WorkItemSummary) -> Vec<String> {
    vec![
        item.key.clone(),
        item.title.clone().unwrap_or_default(),
        item.status.clone().unwrap_or_default(),
        item.item_type.clone().unwrap_or_default(),
        item.priority.clone().unwrap_or_default(),
        item.assignee
            .clone()
            .map(|a| a.name().to_string())
            .unwrap_or_else(|| "-".to_string()),
    ]
}

async fn view(session: &mut Session<'_>, key: &str) -> Result<(), CliError> {
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

fn render_work_item(session: &mut Session<'_>, item: &WorkItem) -> Result<(), CliError> {
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

async fn create(session: &mut Session<'_>, args: &WorkCreateArgs) -> Result<(), CliError> {
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
        Some(value) => Some(super::resolve_user_arg(session, value).await?),
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

async fn edit(session: &mut Session<'_>, args: &WorkEditArgs) -> Result<(), CliError> {
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
            Some(super::resolve_user_arg(session, value).await?)
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
fn tri<T>(clear: bool, value: Option<T>) -> Option<Option<T>> {
    if clear { Some(None) } else { value.map(Some) }
}

async fn transitions(session: &mut Session<'_>, key: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .list_transitions(&org, &project, key)
        .await
        .map_err(CliError::from_client)?;
    let list = response.value.clone();
    emit_view(session, &response.raw, key, |session| {
        session
            .out
            .line(&format!("current status: {}", list.current_status))
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
                    .line(&format!("  {}", transition.target_status))
                    .map_err(CliError::general)?;
            }
        }
        Ok(())
    })
}

async fn transition_to(session: &mut Session<'_>, key: &str, target: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let current = api
        .get_work_item(&org, &project, key)
        .await
        .map_err(CliError::from_client)?;
    let current_status = current.value.status.clone();
    let etag = current.etag.clone();

    if current_status == target {
        session.out.warn(&format!("{key} is already {target}"));
        if session.json() {
            return emit_json(session, &current.raw);
        }
        let item = current.value.clone();
        return emit_view(session, &current.raw, key, |session| {
            render_work_item(session, &item)
        });
    }

    let allowed = api
        .list_transitions(&org, &project, key)
        .await
        .map_err(CliError::from_client)?;
    let permitted = allowed
        .value
        .transitions
        .iter()
        .any(|t| t.target_status == target);
    if !permitted {
        let options: Vec<&str> = allowed
            .value
            .transitions
            .iter()
            .map(|t| t.target_status.as_str())
            .collect();
        return Err(CliError::usage(format!(
            "transition to {target} is not allowed from {current_status}; available: {}",
            if options.is_empty() {
                "none".to_string()
            } else {
                options.join(", ")
            }
        )));
    }

    let if_match =
        etag.ok_or_else(|| CliError::protocol("server did not return an ETag for the work item"))?;
    let idempotency = generate_key();

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "work.transition",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/transitions",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/transitions"
                ),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "workItem": key,
                    "currentStatus": current_status,
                    "targetStatus": target,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(json!({ "targetStatus": target })),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .transition_work_item(
            &org,
            &project,
            key,
            &TransitionRequest {
                target_status: target.to_string(),
            },
            &if_match,
            &idempotency,
        )
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let item = response.value.clone();
    emit_view(session, &response.raw, key, |session| {
        render_work_item(session, &item)
    })
}

async fn label(session: &mut Session<'_>, args: &WorkLabelArgs) -> Result<(), CliError> {
    let (command, label, key, force, idempotency_key) = match &args.command {
        WorkLabelCommand::Add {
            key,
            label,
            force,
            idempotency_key,
        } => ("add", label, key, *force, idempotency_key.as_deref()),
        WorkLabelCommand::Remove {
            key,
            label,
            force,
            idempotency_key,
        } => ("remove", label, key, *force, idempotency_key.as_deref()),
    };
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    // Attach accepts either selector directly in the live contract. Detach
    // remains a label-id path operation, so names are resolved only there.
    let attach_body = AttachLabelRequest {
        label_id: is_uuid(label).then(|| label.clone()),
        label: (!is_uuid(label)).then(|| label.trim().to_lowercase()),
    };
    let label_id = if command == "remove" && !is_uuid(label) {
        let name = label.trim().to_lowercase();
        let mut cursor: Option<String> = None;
        let mut resolved: Option<String> = None;
        loop {
            let response = api
                .list_labels(
                    &org,
                    &project,
                    ListOptions {
                        // The label collection caps `limit` at 100.
                        limit: Some(100),
                        cursor: cursor.clone(),
                    },
                )
                .await
                .map_err(CliError::from_client)?;
            if let Some(matched) = response
                .value
                .items
                .iter()
                .find(|label| label.name == name)
                .map(|l| l.id.clone())
            {
                resolved = Some(matched);
                break;
            }
            if !response.value.page.has_more {
                break;
            }
            cursor = response.value.page.next_cursor;
        }
        resolved.ok_or_else(|| {
            CliError::not_found(format!("no label named {label:?} in project {project}"))
        })?
    } else {
        label.clone()
    };

    let if_match = if force {
        "*".to_string()
    } else {
        let current = api
            .get_work_item(&org, &project, key)
            .await
            .map_err(CliError::from_client)?;
        current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?
    };

    let idempotency = match idempotency_key {
        Some(key) => {
            validate_key(key).map_err(|err| CliError::usage(err.to_string()))?;
            key.to_string()
        }
        None => generate_key(),
    };

    if session.global.dry_run {
        let (operation, method, body) = if command == "add" {
            (
                "work.label.add",
                "POST",
                Some(serde_json::to_value(&attach_body).map_err(CliError::general)?),
            )
        } else {
            ("work.label.remove", "DELETE", None)
        };
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation,
                method,
                path_template: if command == "add" {
                    "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/labels"
                } else {
                    "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/labels/{labelId}"
                },
                path: if command == "add" {
                    format!(
                        "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/labels"
                    )
                } else {
                    format!(
                        "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/labels/{label_id}"
                    )
                },
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "workItem": key,
                    "label": label,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body,
                notes: Vec::new(),
            },
        );
    }

    let response = match command {
        "add" => api
            .attach_label(&org, &project, key, &attach_body, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?,
        _ => api
            .detach_label(&org, &project, key, &label_id, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?,
    };
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

async fn attachment(session: &mut Session<'_>, args: &WorkAttachmentArgs) -> Result<(), CliError> {
    match &args.command {
        WorkAttachmentCommand::List { key, pagination } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let api = session.api(&selection)?;
            let response = api
                .list_attachments(
                    &org,
                    &project,
                    key,
                    ListOptions {
                        limit: pagination.limit,
                        cursor: pagination.cursor.clone(),
                    },
                )
                .await
                .map_err(CliError::from_client)?;
            let rows: Vec<Vec<String>> = response
                .value
                .items
                .iter()
                .map(|a| {
                    vec![
                        a.id.clone(),
                        a.file_name.clone(),
                        a.content_type.clone(),
                        a.size.to_string(),
                        a.created_by
                            .clone()
                            .map(|u| u.name().to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        a.created_at.clone(),
                    ]
                })
                .collect();
            emit_table(
                session,
                &response.raw,
                &["ID", "FILE", "TYPE", "SIZE", "BY", "CREATED"],
                &rows,
            )
        }
        WorkAttachmentCommand::Upload {
            key,
            file,
            file_name,
            content_type,
            idempotency_key,
        } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let (bytes, name) = read_upload(file, file_name.as_deref()).map_err(CliError::usage)?;
            let upload = hamstik_api_client::client::MultipartFile {
                file_name: name,
                content_type: content_type.clone(),
                bytes,
            };
            let idempotency = idem_key(idempotency_key.clone())?;

            if session.global.dry_run {
                return dryrun::emit_preview(
                    session,
                    dryrun::PreviewRequest {
                        operation: "work.attachment.upload",
                        method: "POST",
                        path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/attachments",
                        path: format!(
                            "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/attachments"
                        ),
                        resolved: json!({
                            "organization": org,
                            "project": project,
                            "workItem": key,
                            "fileName": upload.file_name,
                            "contentType": upload.content_type,
                            "size": upload.bytes.len(),
                        }),
                        if_match: None,
                        idempotency_key: Some(&idempotency),
                        body: Some(json!({
                            "multipart": "form-data",
                            "part": "file",
                            "fileName": upload.file_name,
                            "contentType": upload.content_type.as_deref().unwrap_or("application/octet-stream"),
                            "size": upload.bytes.len(),
                        })),
                        notes: vec!["file bytes are not shown and will not be uploaded"],
                    },
                );
            }

            let api = session.api(&selection)?;
            let response = api
                .upload_attachment(&org, &project, key, &upload, &idempotency)
                .await
                .map_err(CliError::from_client)?;
            if response.idempotency_replayed {
                session
                    .out
                    .warn("note: request replayed (idempotent duplicate)");
            }
            let attachment_id = response.value.id.clone();
            emit_view(session, &response.raw, &attachment_id.clone(), |session| {
                render_attachment(session, &response.value)
            })
        }
        WorkAttachmentCommand::Download {
            key,
            attachment_id,
            output,
        } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let api = session.api(&selection)?;
            let download = api
                .download_attachment(&org, &project, key, attachment_id)
                .await
                .map_err(CliError::from_client)?;
            let target = match output {
                Some(path) => std::path::PathBuf::from(path),
                None => {
                    let name = download
                        .file_name
                        .clone()
                        .unwrap_or_else(|| format!("{attachment_id}.bin"));
                    // Refuse to write outside the current directory implicitly:
                    // use only the final path component of the server-suggested name.
                    let safe = name.rsplit(['/', '\\']).next().unwrap_or(&name);
                    std::path::PathBuf::from(safe)
                }
            };
            std::fs::write(&target, &download.bytes).map_err(|err| {
                CliError::general(format!("cannot write {}: {err}", target.display()))
            })?;
            if session.json() {
                emit_json(
                    session,
                    &json!({
                        "attachmentId": attachment_id,
                        "path": target.display().to_string(),
                        "size": download.bytes.len(),
                        "fileName": download.file_name,
                        "contentType": download.content_type,
                        "contentLength": download.content_length,
                        "contentDisposition": download.content_disposition,
                        "requestId": download.request_id,
                    }),
                )
            } else {
                session
                    .out
                    .line(&format!(
                        "Downloaded {} ({} bytes) to {}",
                        attachment_id,
                        download.bytes.len(),
                        target.display()
                    ))
                    .map_err(CliError::general)
            }
        }
        WorkAttachmentCommand::Delete { key, attachment_id } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;

            if session.global.dry_run {
                return dryrun::emit_preview(
                    session,
                    dryrun::PreviewRequest {
                        operation: "work.attachment.delete",
                        method: "DELETE",
                        path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/attachments/{attachmentId}",
                        path: format!(
                            "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/attachments/{attachment_id}"
                        ),
                        resolved: json!({
                            "organization": org,
                            "project": project,
                            "workItem": key,
                            "attachmentId": attachment_id,
                        }),
                        if_match: None,
                        idempotency_key: None,
                        body: None,
                        notes: Vec::new(),
                    },
                );
            }

            let api = session.api(&selection)?;
            api.delete_attachment(&org, &project, key, attachment_id)
                .await
                .map_err(CliError::from_client)?;
            if session.json() {
                emit_json(
                    session,
                    &json!({ "deleted": true, "attachmentId": attachment_id }),
                )
            } else {
                session
                    .out
                    .line(&format!("Deleted attachment {attachment_id}"))
                    .map_err(CliError::general)
            }
        }
    }
}

fn render_attachment(
    session: &mut Session<'_>,
    attachment: &hamstik_api_client::Attachment,
) -> Result<(), CliError> {
    let lines = [
        ("id", attachment.id.clone()),
        ("file", attachment.file_name.clone()),
        ("type", attachment.content_type.clone()),
        ("size", attachment.size.to_string()),
        (
            "by",
            attachment
                .created_by
                .clone()
                .map(|u| u.name().to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("created", attachment.created_at.clone()),
    ];
    render_lines(session, &lines)
}

/// Reads upload bytes from a path or stdin (`-`), capping at the response-body
/// budget (10 MiB), and derives a default file name from the path.
///
/// Both sources are streamed through the cap, so an oversized or misdirected
/// stream is rejected at the first excess byte instead of being buffered.
fn read_upload(file: &str, explicit_name: Option<&str>) -> Result<(Vec<u8>, String), String> {
    const MAX_UPLOAD_BYTES: usize = hamstik_api_client::client::MAX_BODY_BYTES;
    let (bytes, default_name) = if file == "-" {
        let stdin = std::io::stdin();
        let bytes = crate::input::read_bytes_capped(stdin.lock(), MAX_UPLOAD_BYTES)
            .map_err(|err| format!("cannot read stdin: {err}"))?;
        (bytes, "attachment".to_string())
    } else {
        let path = std::path::Path::new(file);
        let handle =
            std::fs::File::open(path).map_err(|err| format!("cannot read {file}: {err}"))?;
        let bytes = crate::input::read_bytes_capped(handle, MAX_UPLOAD_BYTES)
            .map_err(|err| format!("cannot read {file}: {err}"))?;
        let name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("attachment")
            .to_string();
        (bytes, name)
    };
    let name = explicit_name.map(str::to_string).unwrap_or(default_name);
    Ok((bytes, name))
}

async fn comment(session: &mut Session<'_>, args: &CommentArgs) -> Result<(), CliError> {
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
            let response = api
                .list_comments(
                    &org,
                    &project,
                    key,
                    ListOptions {
                        limit: pagination.limit,
                        cursor: pagination.cursor.clone(),
                    },
                )
                .await
                .map_err(CliError::from_client)?;
            // Soft-deleted comments stay in the thread to preserve reply
            // structure; `--exclude-deleted` filters them from the rendering.
            let items: Vec<_> = if *exclude_deleted {
                response
                    .value
                    .items
                    .iter()
                    .filter(|comment| !comment.deleted)
                    .collect()
            } else {
                response.value.items.iter().collect()
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
                &response.raw,
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

async fn link(session: &mut Session<'_>, args: &WorkLinkArgs) -> Result<(), CliError> {
    match &args.command {
        WorkLinkCommand::List { key, pagination } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let api = session.api(&selection)?;
            let response = list_links(api, &org, &project, key, pagination).await?;
            let rows: Vec<Vec<String>> = response
                .value
                .items
                .iter()
                .map(|l| {
                    vec![
                        l.id.clone(),
                        l.relation.clone(),
                        l.other_work_item.key.clone(),
                        l.other_work_item.title.clone(),
                        l.created_by
                            .as_ref()
                            .map(|u| u.name().to_string())
                            .unwrap_or_else(|| "-".to_string()),
                    ]
                })
                .collect();
            emit_table(
                session,
                &response.raw,
                &["ID", "RELATION", "OTHER", "TITLE", "BY"],
                &rows,
            )
        }
        WorkLinkCommand::Add {
            key,
            target_key,
            target_id,
            relation,
            idempotency_key,
        } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let api = session.api(&selection)?;
            let idempotency = idem_key(idempotency_key.clone())?;

            if session.global.dry_run {
                return dryrun::emit_preview(
                    session,
                    dryrun::PreviewRequest {
                        operation: "work.link.add",
                        method: "POST",
                        path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/links",
                        path: format!(
                            "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/links"
                        ),
                        resolved: json!({
                            "organization": org,
                            "project": project,
                            "workItem": key,
                            "targetKey": target_key,
                            "targetId": target_id,
                            "relation": relation.as_str(),
                        }),
                        if_match: None,
                        idempotency_key: Some(&idempotency),
                        body: Some(json!({
                            "targetId": target_id,
                            "targetKey": target_key,
                            "relation": relation.as_str(),
                        })),
                        notes: Vec::new(),
                    },
                );
            }

            let response = api
                .create_work_item_link(
                    &org,
                    &project,
                    key,
                    &CreateWorkItemLinkRequest {
                        target_id: target_id.clone(),
                        target_key: target_key.clone(),
                        relation: relation.as_str().to_string(),
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
            let link = response.value.clone();
            emit_view(session, &response.raw, &link.id.clone(), |session| {
                render_link(session, &link)
            })
        }
        WorkLinkCommand::Delete {
            key,
            link_id,
            idempotency_key,
        } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let idempotency = idem_key(idempotency_key.clone())?;

            if session.global.dry_run {
                return dryrun::emit_preview(
                    session,
                    dryrun::PreviewRequest {
                        operation: "work.link.delete",
                        method: "DELETE",
                        path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/links/{linkId}",
                        path: format!(
                            "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/links/{link_id}"
                        ),
                        resolved: json!({
                            "organization": org,
                            "project": project,
                            "workItem": key,
                            "linkId": link_id,
                        }),
                        if_match: None,
                        idempotency_key: Some(&idempotency),
                        body: None,
                        notes: Vec::new(),
                    },
                );
            }

            let api = session.api(&selection)?;
            let response = api
                .delete_work_item_link(&org, &project, key, link_id, &idempotency)
                .await
                .map_err(CliError::from_client)?;
            if response.idempotency_replayed {
                session
                    .out
                    .warn("note: request replayed (idempotent duplicate)");
            }
            if session.json() {
                emit_json(session, &json!({ "deleted": true, "linkId": link_id }))
            } else {
                session
                    .out
                    .line(&format!("Deleted link {link_id}"))
                    .map_err(CliError::general)
            }
        }
    }
}

/// Runs `hamstik work watcher` subcommands (dry-run included).
///
/// The watcher surface carries no revision guard (no `If-Match`): the server
/// resolves the authenticated user's own state, and actions are idempotent.
async fn watcher(session: &mut Session<'_>, args: &WorkWatcherArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let (key, action, idempotency_key) = match &args.command {
        WorkWatcherCommand::Show { key } => (key, None, None),
        WorkWatcherCommand::Watch {
            key,
            idempotency_key,
        } => (key, Some(WatcherAction::Watch), idempotency_key.as_deref()),
        WorkWatcherCommand::Unwatch {
            key,
            idempotency_key,
        } => (
            key,
            Some(WatcherAction::Unwatch),
            idempotency_key.as_deref(),
        ),
        WorkWatcherCommand::Mute {
            key,
            idempotency_key,
        } => (key, Some(WatcherAction::Mute), idempotency_key.as_deref()),
        WorkWatcherCommand::Unmute {
            key,
            idempotency_key,
        } => (key, Some(WatcherAction::Unmute), idempotency_key.as_deref()),
    };
    let idempotency = idem_key(idempotency_key.map(str::to_string))?;

    if let Some(action) = action
        && session.global.dry_run
    {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "work.watcher.action",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/watcher",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/watcher"
                ),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "workItem": key,
                    "action": action.as_str(),
                }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(json!({ "action": action.as_str() })),
                notes: Vec::new(),
            },
        );
    }

    let api = session.api(&selection)?;
    let response = match action {
        None => api
            .get_work_item_watcher(&org, &project, key)
            .await
            .map_err(CliError::from_client)?,
        Some(action) => {
            let response = api
                .update_work_item_watcher(&org, &project, key, action, &idempotency)
                .await
                .map_err(CliError::from_client)?;
            if response.idempotency_replayed {
                session
                    .out
                    .warn("note: request replayed (idempotent duplicate)");
            }
            response
        }
    };
    let watcher = response.value.clone();
    emit_view(session, &response.raw, key, |session| {
        render_watcher(session, &watcher)
    })
}

/// Renders the authenticated user's watcher state.
fn render_watcher(session: &mut Session<'_>, watcher: &WorkItemWatcher) -> Result<(), CliError> {
    let state = if watcher.watched {
        "watching"
    } else {
        "not watching"
    };
    session
        .out
        .line(&format!("Watcher state: {state}"))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("  manual watch:    {}", watcher.manual_watch))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("  from assignment: {}", watcher.assignee_origin))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("  muted:           {}", watcher.muted))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("  assigned:        {}", watcher.assigned))
        .map_err(CliError::general)?;
    Ok(())
}

async fn list_links(
    api: std::sync::Arc<dyn hamstik_api_client::HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
    pagination: &crate::args::PaginationArgs,
) -> Result<hamstik_api_client::ApiResponse<hamstik_api_client::WorkItemLinkList>, CliError> {
    let opts = ListOptions {
        limit: pagination.limit,
        cursor: pagination.cursor.clone(),
    };
    if pagination.all {
        let fetch_api = api.clone();
        let org = org.to_string();
        let project = project.to_string();
        let key = key.to_string();
        let limit = pagination.limit;
        let page = follow_all(move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let project = project.clone();
            let key = key.to_string();
            async move {
                let response = fetch_api
                    .list_work_item_links(&org, &project, &key, ListOptions { limit, cursor })
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
        Ok(hamstik_api_client::ApiResponse {
            value: hamstik_api_client::WorkItemLinkList {
                items: page.items,
                page: page.page.clone(),
            },
            raw: json!({ "items": page.raw_items, "page": page.page }),
            request_id: None,
            etag: None,
            idempotency_replayed: false,
            location: None,
            rate_limit: None,
        })
    } else {
        api.list_work_item_links(org, project, key, opts)
            .await
            .map_err(CliError::from_client)
    }
}

fn render_link(
    session: &mut Session<'_>,
    link: &hamstik_api_client::WorkItemLink,
) -> Result<(), CliError> {
    let lines = [
        ("id", link.id.clone()),
        ("relation", link.relation.clone()),
        ("other", link.other_work_item.key.clone()),
        ("title", link.other_work_item.title.clone()),
        ("project", link.other_work_item.project.key.clone()),
        (
            "created by",
            link.created_by
                .as_ref()
                .map(|u| u.name().to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("created", link.created_at.clone()),
    ];
    render_lines(session, &lines)
}

async fn activity(
    session: &mut Session<'_>,
    key: &str,
    since: Option<&str>,
    pagination: &crate::args::PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    let opts = ActivityOptions {
        limit: pagination.limit,
        cursor: pagination.cursor.clone(),
        since: since.map(str::to_string),
    };
    let json_value: Value = if pagination.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let key = key.to_string();
        let since = opts.since.clone();
        let limit = pagination.limit;
        let page = follow_all(move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let project = project.clone();
            let key = key.clone();
            let since = since.clone();
            async move {
                let opts = ActivityOptions {
                    limit,
                    cursor,
                    since: since.clone(),
                };
                let response = fetch_api
                    .list_work_item_activity(&org, &project, &key, opts)
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
            .list_work_item_activity(&org, &project, key, opts)
            .await
            .map_err(CliError::from_client)?;
        response.raw
    };
    let rows: Vec<Vec<String>> = json_value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(activity_row_from_raw).collect())
        .unwrap_or_default();
    emit_table(
        session,
        &json_value,
        &["ID", "ACTION", "ACTOR", "DETAIL", "CREATED"],
        &rows,
    )
}

fn activity_row_from_raw(raw: &Value) -> Vec<String> {
    let detail = raw
        .get("detail")
        .filter(|d| !d.is_null())
        .map(Value::to_string)
        .unwrap_or_else(|| "-".to_string());
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
        detail,
        raw.get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    ]
}

/// Archives (or unarchives) a Work Item with the Work Item ETag.
async fn change_archive(
    session: &mut Session<'_>,
    key: &str,
    archived: bool,
    force: bool,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let if_match = if force {
        "*".to_string()
    } else {
        let current = api
            .get_work_item(&org, &project, key)
            .await
            .map_err(CliError::from_client)?;
        current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?
    };
    let idempotency = idem_key_ref(idempotency_key)?;

    if session.global.dry_run {
        let (operation, suffix) = if archived {
            ("work.archive", "/archive")
        } else {
            ("work.unarchive", "/unarchive")
        };
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation,
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/archive",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/work-items/{key}{suffix}"
                ),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "workItem": key,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(json!({})),
                notes: Vec::new(),
            },
        );
    }

    let response = if archived {
        api.archive_work_item(&org, &project, key, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?
    } else {
        api.unarchive_work_item(&org, &project, key, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?
    };
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

/// Soft-deletes a Work Item (Organization owner only).
async fn delete(
    session: &mut Session<'_>,
    key: &str,
    cascade: bool,
    force: bool,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let if_match = if force {
        "*".to_string()
    } else {
        let current = api
            .get_work_item(&org, &project, key)
            .await
            .map_err(CliError::from_client)?;
        current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?
    };
    let idempotency = idem_key_ref(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "work.delete",
                method: "DELETE",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}",
                path: format!("/api/v1/organizations/{org}/projects/{project}/work-items/{key}"),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "workItem": key,
                    "cascade": cascade,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(json!({ "cascade": cascade })),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .delete_work_item(&org, &project, key, cascade, &if_match, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    if session.json() {
        emit_json(
            session,
            &json!({ "deleted": true, "key": key, "cascade": cascade }),
        )
    } else {
        session
            .out
            .line(&format!("Deleted work item {key}"))
            .map_err(CliError::general)
    }
}

/// Reads and preflights the bulk operations payload (path or `-` for stdin).
///
/// The JSON syntax, envelope shape, per-operation required fields, unknown
/// fields, enum spellings, and revision constraints are validated locally
/// against the checked-in Public API schema-derived rules before any HTTP
/// request. Malformed input fails with the failing operation index and field
/// path and never echoes unrelated payload content.
fn read_operations<T: DeserializeOwned>(
    path: &str,
    kind: bulk_preflight::PreflightKind,
) -> Result<Vec<T>, CliError> {
    const MAX_OPERATIONS_BYTES: usize = 1024 * 1024;
    let text = if path == "-" {
        let stdin = std::io::stdin();
        crate::input::read_capped(stdin.lock(), MAX_OPERATIONS_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read stdin: {err}")))?
    } else {
        let handle = std::fs::File::open(path)
            .map_err(|err| CliError::usage(format!("cannot read {path}: {err}")))?;
        crate::input::read_capped(handle, MAX_OPERATIONS_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read {path}: {err}")))?
    };
    let source = if path == "-" { "stdin" } else { path };
    let items: Vec<T> = serde_json::from_str(
        &serde_json::to_string(&bulk_preflight::preflight(&text, source, kind)?)
            .map_err(CliError::general)?,
    )
    .map_err(|err| {
        CliError::usage(format!(
            "operations must match the Public API JSON array schema: {err}"
        ))
    })?;
    Ok(items)
}

async fn bulk(session: &mut Session<'_>, args: &WorkBulkArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let idempotency = idem_key(args_idempotency(&args.command))?;

    // Bulk previews read and shape-validate the operations file exactly as a
    // real invocation would, then emit the request the CLI would send.
    if session.global.dry_run {
        let (operation, method, path, path_template, body): (&str, &str, String, &str, Value) =
            match &args.command {
                WorkBulkCommand::Create {
                    operations_file, ..
                } => {
                    let body = BulkCreateEnvelope {
                        operations: read_operations(
                            operations_file,
                            bulk_preflight::PreflightKind::Create,
                        )?,
                    };
                    (
                        "work.bulk.create",
                        "POST",
                        format!("/api/v1/organizations/{org}/bulk-work-items"),
                        "/api/v1/organizations/{organization}/bulk-work-items",
                        serde_json::to_value(&body).map_err(CliError::general)?,
                    )
                }
                WorkBulkCommand::Update {
                    operations_file,
                    concurrency,
                    ..
                } => {
                    let body = BulkUpdateEnvelope {
                        concurrency: concurrency
                            .unwrap_or(crate::args::ConcurrencyArg::RequireRevision)
                            .as_str()
                            .to_string(),
                        operations: read_operations(
                            operations_file,
                            bulk_preflight::PreflightKind::Update,
                        )?,
                    };
                    (
                        "work.bulk.update",
                        "PATCH",
                        format!("/api/v1/organizations/{org}/bulk-work-items"),
                        "/api/v1/organizations/{organization}/bulk-work-items",
                        serde_json::to_value(&body).map_err(CliError::general)?,
                    )
                }
                WorkBulkCommand::Transition {
                    operations_file,
                    concurrency,
                    ..
                } => {
                    let body = BulkTransitionEnvelope {
                        concurrency: concurrency.map(|c| c.as_str().to_string()),
                        operations: read_operations(
                            operations_file,
                            bulk_preflight::PreflightKind::Transition,
                        )?,
                    };
                    (
                        "work.bulk.transition",
                        "POST",
                        format!("/api/v1/organizations/{org}/bulk-work-item-transitions"),
                        "/api/v1/organizations/{organization}/bulk-work-item-transitions",
                        serde_json::to_value(&body).map_err(CliError::general)?,
                    )
                }
            };
        let count = body
            .get("operations")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation,
                method,
                path_template,
                path,
                resolved: json!({
                    "organization": org,
                    "operationCount": count,
                }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(body),
                notes: Vec::new(),
            },
        );
    }

    let response = match &args.command {
        WorkBulkCommand::Create {
            operations_file, ..
        } => {
            let body = BulkCreateEnvelope {
                operations: read_operations(
                    operations_file,
                    bulk_preflight::PreflightKind::Create,
                )?,
            };
            api.bulk_create_work_items(&org, &body, &idempotency)
                .await
                .map_err(CliError::from_client)?
        }
        WorkBulkCommand::Update {
            operations_file,
            concurrency,
            ..
        } => {
            let body = BulkUpdateEnvelope {
                concurrency: concurrency
                    .unwrap_or(crate::args::ConcurrencyArg::RequireRevision)
                    .as_str()
                    .to_string(),
                operations: read_operations(
                    operations_file,
                    bulk_preflight::PreflightKind::Update,
                )?,
            };
            api.bulk_update_work_items(&org, &body, &idempotency)
                .await
                .map_err(CliError::from_client)?
        }
        WorkBulkCommand::Transition {
            operations_file,
            concurrency,
            ..
        } => {
            let body = BulkTransitionEnvelope {
                concurrency: concurrency.map(|c| c.as_str().to_string()),
                operations: read_operations(
                    operations_file,
                    bulk_preflight::PreflightKind::Transition,
                )?,
            };
            api.bulk_transition_work_items(&org, &body, &idempotency)
                .await
                .map_err(CliError::from_client)?
        }
    };
    let concurrency_mode = match &args.command {
        WorkBulkCommand::Create { .. } => None,
        WorkBulkCommand::Update { concurrency, .. }
        | WorkBulkCommand::Transition { concurrency, .. } => Some(
            concurrency
                .unwrap_or(crate::args::ConcurrencyArg::RequireRevision)
                .as_str(),
        ),
    };
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    render_bulk(session, &response.raw, concurrency_mode)
}

fn args_idempotency(command: &WorkBulkCommand) -> Option<String> {
    match command {
        WorkBulkCommand::Create {
            idempotency_key, ..
        }
        | WorkBulkCommand::Update {
            idempotency_key, ..
        }
        | WorkBulkCommand::Transition {
            idempotency_key, ..
        } => idempotency_key.clone(),
    }
}

/// Renders bulk results: the raw `{results: [...]}` body in JSON mode, a
/// per-item table with a total/succeeded/failed summary and actionable
/// failure details in human mode, or a compact success/error listing in quiet.
fn render_bulk(
    session: &mut Session<'_>,
    raw: &Value,
    concurrency_mode: Option<&str>,
) -> Result<(), CliError> {
    if session.json() {
        // JSON preserves every documented per-operation field exactly as the
        // server returned it (index, status, workItem, error).
        return emit_json(session, raw);
    }
    // State the selected concurrency mode explicitly so last-write-wins is
    // always a visible, deliberate choice in the transcript.
    if let Some(mode) = concurrency_mode {
        let label = bulk_preflight::concurrency_label(Some(mode));
        session
            .out
            .line(&format!(
                "concurrency: {} ({})",
                label["mode"].as_str().unwrap_or(mode),
                label["note"].as_str().unwrap_or_default()
            ))
            .map_err(CliError::general)?;
    }
    let results = raw
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let failed: Vec<&Value> = results
        .iter()
        .filter(|result| result.get("error").is_some_and(|e| !e.is_null()))
        .collect();
    let total = results.len();
    let succeeded = total - failed.len();
    if session.out.is_quiet() {
        for result in &results {
            let index = result.get("index").and_then(Value::as_i64).unwrap_or(0);
            let status = result.get("status").and_then(Value::as_i64).unwrap_or(0);
            if let Some(error) = result.get("error").filter(|e| !e.is_null()) {
                let code = error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("UNKNOWN");
                session
                    .out
                    .line(&format!("{index}\t{status}\t{code}"))
                    .map_err(CliError::general)?;
            } else {
                let key = result
                    .get("workItem")
                    .and_then(|w| w.get("key"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                session
                    .out
                    .line(&format!("{index}\t{status}\t{key}"))
                    .map_err(CliError::general)?;
            }
        }
        return Ok(());
    }
    let rows: Vec<Vec<String>> = results
        .iter()
        .map(|result| {
            let index = result.get("index").and_then(Value::as_i64).unwrap_or(0);
            let status = result.get("status").and_then(Value::as_i64).unwrap_or(0);
            let outcome = if let Some(error) = result.get("error").filter(|e| !e.is_null()) {
                let code = error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("UNKNOWN");
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                format!("{code}: {message}")
            } else {
                result
                    .get("workItem")
                    .and_then(|w| w.get("key"))
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string()
            };
            vec![index.to_string(), status.to_string(), outcome]
        })
        .collect();
    // A leading summary line states the mixed-outcome counts up front.
    let summary = if failed.is_empty() {
        format!("bulk results: {total}/{total} succeeded")
    } else {
        format!(
            "bulk results: {succeeded}/{total} succeeded, {} failed",
            failed.len()
        )
    };
    session.out.line(&summary).map_err(CliError::general)?;
    if !failed.is_empty() {
        session
            .out
            .line("failed operations (actionable details):")
            .map_err(CliError::general)?;
        for result in &failed {
            let index = result.get("index").and_then(Value::as_i64).unwrap_or(0);
            let status = result.get("status").and_then(Value::as_i64).unwrap_or(0);
            let error = result.get("error").unwrap_or(&Value::Null);
            let code = error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN");
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the server did not provide a message");
            let detail = match (code, status) {
                ("REVISION_CONFLICT", _) => Some(
                    "re-read the Work Item (work view) and either set the new revision on the \
                     operation or re-run with --concurrency last-write-wins to accept any prior state"
                        .to_string(),
                ),
                ("INVALID_TRANSITION", _) => Some(format!(
                    "list the allowed transitions with: hamstik work transitions (operation index {index})"
                )),
                ("NOT_FOUND", 404) => Some(format!(
                    "the Work Item targeted by operations[{index}] does not exist in this project; \
                     verify the key"
                )),
                ("ARCHIVED", _) | ("CONFLICT", _) if status == 409 || status == 404 => Some(
                    "the target Work Item may be archived; list archived items with \
                     `work list --archived true` and unarchive it first"
                        .to_string(),
                ),
                _ => None,
            };
            session
                .out
                .line(&format!(
                    "  - operations[{index}] (HTTP {status}) {code}: {message}"
                ))
                .map_err(CliError::general)?;
            if let Some(detail) = detail {
                session
                    .out
                    .line(&format!("    fix: {detail}"))
                    .map_err(CliError::general)?;
            }
            if let Some(request_id) = error.get("requestId").and_then(Value::as_str) {
                session
                    .out
                    .line(&format!("    request id: {request_id}"))
                    .map_err(CliError::general)?;
            }
        }
    }
    emit_table(session, raw, &["INDEX", "STATUS", "OUTCOME"], &rows)
}

fn render_comment(session: &mut Session<'_>, raw: &Value) -> Result<(), CliError> {
    let body = raw
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or("(no body)");
    session.out.line(body).map_err(CliError::general)?;
    Ok(())
}
