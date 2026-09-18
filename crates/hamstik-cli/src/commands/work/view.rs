// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work view` / `work create` / `work edit`.

use std::sync::Arc;

use serde_json::json;

use tokio::sync::Semaphore;

use hamstik_api_client::{
    ActivityOptions, CreateWorkItemRequest, HamstikApi, ListOptions, UpdateWorkItemRequest,
    UserSummary, WorkItem,
};

use crate::app::Session;
use crate::args::{WorkCreateArgs, WorkEditArgs, WorkViewArgs};
use crate::error::CliError;

use super::common::{assignee_fields, idem_key, read_long_text};
use super::dryrun;
use super::emit_json;
use super::emit_view;
use super::render_lines;

const DEFAULT_CONCURRENCY: usize = 8;
const MAX_KEYS: usize = 500;
const TRUNCATED_MARKER: &str = "[description truncated]";

pub(super) async fn view(session: &mut Session<'_>, args: &WorkViewArgs) -> Result<(), CliError> {
    let keys = resolve_keys(args)?;
    if keys.is_empty() {
        return Err(CliError::usage("at least one key is required"));
    }
    if keys.len() > MAX_KEYS {
        return Err(CliError::usage(format!(
            "too many keys: {}/{}",
            keys.len(),
            MAX_KEYS
        )));
    }

    let single = keys.len() == 1;
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    let comments = args.comments;
    let activity = args.activity;
    let compact = args.compact;

    let semaphore = Arc::new(Semaphore::new(DEFAULT_CONCURRENCY));
    let mut tasks = Vec::with_capacity(keys.len());

    for key in keys {
        let api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let sem = semaphore.clone();
        let key_for_task = key.clone();
        let handle = tokio::spawn(async move {
            let _permit = sem
                .acquire()
                .await
                .map_err(|e| CliError::general(format!("concurrency control failed: {e}")))?;
            fetch_one(
                &api,
                &org,
                &project,
                &key_for_task,
                comments,
                activity,
                compact,
            )
            .await
        });
        tasks.push((key, handle));
    }

    let mut results: Vec<ItemResult> = Vec::with_capacity(tasks.len());
    let mut worst: Option<CliError> = None;

    for (key, handle) in tasks {
        match handle
            .await
            .map_err(|e| CliError::general(format!("task failed: {e}")))?
        {
            Ok(item) => {
                results.push(ItemResult::success(key.clone(), item));
            }
            Err(err) => {
                worst = Some(worst.map(|w| worst_exit(w, &err)).unwrap_or(err.clone()));
                results.push(ItemResult::failure(key.clone(), err));
            }
        }
    }

    if single {
        let result = results
            .into_iter()
            .next()
            .ok_or_else(|| CliError::general("no results returned for single key"))?;
        let key = result.key.clone();
        match result.outcome {
            Outcome::Success(item) => emit_view(session, &item.raw_response, &key, |session| {
                render_work_item(session, &item.item, args.compact)
            }),
            Outcome::Failure(err) => Err(err),
        }
    } else {
        let failures = results.iter().filter(|r| r.outcome.is_failure()).count();
        let envelope = json!({
            "items": results.iter().map(|r| r.to_json()).collect::<Vec<_>>(),
            "failures": failures,
            "total": results.len(),
        });
        if let Some(err) = worst {
            // Emit the JSON envelope first, then surface the worst exit code.
            let _ = emit_json(session, &envelope);
            Err(err)
        } else {
            emit_json(session, &envelope)
        }
    }
}

fn worst_exit(current: CliError, contender: &CliError) -> CliError {
    if contender.exit_code() > current.exit_code() {
        contender.clone()
    } else {
        current
    }
}

struct ItemResult {
    key: String,
    outcome: Outcome,
}

impl ItemResult {
    fn success(key: String, item: FetchedItem) -> Self {
        Self {
            key,
            outcome: Outcome::Success(Box::new(item)),
        }
    }

    fn failure(key: String, err: CliError) -> Self {
        Self {
            key,
            outcome: Outcome::Failure(err),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        match &self.outcome {
            Outcome::Success(item) => json!({
                "key": self.key,
                "status": "ok",
                "item": item.to_json(),
            }),
            Outcome::Failure(err) => json!({
                "key": self.key,
                "status": "error",
                "error": err.to_json(),
            }),
        }
    }
}

#[derive(Debug)]
enum Outcome {
    Success(Box<FetchedItem>),
    Failure(CliError),
}

impl Outcome {
    fn is_failure(&self) -> bool {
        matches!(self, Self::Failure(_))
    }
}

#[derive(Debug)]
pub(super) struct FetchedItem {
    item: WorkItem,
    raw_response: serde_json::Value,
}

impl FetchedItem {
    fn to_json(&self) -> serde_json::Value {
        json!({
            "key": self.item.key,
            "title": self.item.title,
            "status": self.item.status,
            "type": self.item.item_type,
            "priority": self.item.priority,
            "assignee": self.item.assignee.as_ref().map(|a| {
                let id = match a {
                    UserSummary::Legacy { id, .. } => id,
                    UserSummary::Public { public_id, .. } => public_id,
                };
                json!({ "id": id, "name": a.name() })
            }),
            "reporter": self.item.reporter.as_ref().map(|a| {
                let id = match a {
                    UserSummary::Legacy { id, .. } => id,
                    UserSummary::Public { public_id, .. } => public_id,
                };
                json!({ "id": id, "name": a.name() })
            }),
            "project": self.item.project_id,
            "sprint": self.item.sprint.as_ref().map(|s| json!({
                "id": s.id,
                "name": s.name,
            })),
            "parent": self.item.parent.as_ref().map(|p| json!({
                "id": p.id,
                "key": p.key,
            })),
            "story_points": self.item.story_points,
            "due_date": self.item.due_date,
            "revision": self.item.revision,
            "created_at": self.item.created_at,
            "updated_at": self.item.updated_at,
        })
    }
}

fn resolve_keys(args: &WorkViewArgs) -> Result<Vec<String>, CliError> {
    if !args.keys.is_empty() {
        return Ok(args.keys.clone());
    }
    let path = args
        .file
        .as_ref()
        .ok_or_else(|| CliError::usage("provide at least one key, or pass --file"))?;
    read_keys_file(path)
}

fn read_keys_file(path: &str) -> Result<Vec<String>, CliError> {
    const MAX_FILE_BYTES: usize = 1024 * 1024;
    let text = if path == "-" {
        let stdin = std::io::stdin();
        crate::input::read_capped(stdin.lock(), MAX_FILE_BYTES)
            .map_err(|e| CliError::usage(format!("cannot read stdin: {e}")))?
    } else {
        let handle = std::fs::File::open(path)
            .map_err(|e| CliError::usage(format!("cannot read {path}: {e}")))?;
        crate::input::read_capped(handle, MAX_FILE_BYTES)
            .map_err(|e| CliError::usage(format!("cannot read {path}: {e}")))?
    };
    let keys: Vec<String> = text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('#'))
        .map(|line| line.trim().to_string())
        .collect();
    if keys.is_empty() {
        return Err(CliError::usage("no keys found in file"));
    }
    Ok(keys)
}

#[allow(unused_variables)]
async fn fetch_one(
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
    comments: u32,
    activity: u32,
    compact: bool,
) -> Result<FetchedItem, CliError> {
    let response = api
        .get_work_item(org, project, key)
        .await
        .map_err(CliError::from_client)?;
    let item = response.value.clone();
    let raw = response.raw.clone();

    // Fetch optional sections concurrently.
    let comment_handle: tokio::task::JoinHandle<Result<serde_json::Value, CliError>> =
        if comments > 0 {
            let api = api.clone();
            let org = org.to_string();
            let project = project.to_string();
            let key = key.to_string();
            tokio::spawn(async move {
                let opts = ListOptions {
                    limit: Some(comments),
                    cursor: None,
                };
                let resp = api
                    .list_comments(&org, &project, &key, opts)
                    .await
                    .map_err(CliError::from_client)?;
                Ok(resp.raw)
            })
        } else {
            tokio::spawn(async { Ok(json!(null)) })
        };

    let activity_handle: tokio::task::JoinHandle<Result<serde_json::Value, CliError>> =
        if activity > 0 {
            let api = api.clone();
            let org = org.to_string();
            let project = project.to_string();
            let key = key.to_string();
            tokio::spawn(async move {
                let opts = ActivityOptions {
                    limit: Some(activity),
                    cursor: None,
                    since: None,
                };
                let resp = api
                    .list_work_item_activity(&org, &project, &key, opts)
                    .await
                    .map_err(CliError::from_client)?;
                Ok(resp.raw)
            })
        } else {
            tokio::spawn(async { Ok(json!(null)) })
        };

    let _ = comment_handle
        .await
        .map_err(|e| CliError::general(format!("comment fetch task failed: {e}")))?;
    let _ = activity_handle
        .await
        .map_err(|e| CliError::general(format!("activity fetch task failed: {e}")))?;

    Ok(FetchedItem {
        item,
        raw_response: raw,
    })
}

pub(super) fn render_work_item(
    session: &mut Session<'_>,
    item: &WorkItem,
    compact: bool,
) -> Result<(), CliError> {
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
    session.out.line("").map_err(CliError::general)?;
    if compact {
        session
            .out
            .line(TRUNCATED_MARKER)
            .map_err(CliError::general)?;
    } else if let Some(description) = &item.description {
        session.out.line(description).map_err(CliError::general)?;
    } else {
        session
            .out
            .line("(no description)")
            .map_err(CliError::general)?;
    }
    Ok(())
}

#[allow(unused_variables)]
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
        render_work_item(session, &item, false)
    })
}

#[allow(unused_variables)]
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
        render_work_item(session, &item, false)
    })
}

/// Builds a tri-state field: `None` (unset), `Some(None)` (clear), or `Some(Some(v))`.
pub(super) fn tri<T>(clear: bool, value: Option<T>) -> Option<Option<T>> {
    if clear { Some(None) } else { value.map(Some) }
}
