// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work context` — the one-invocation read bundle (CLI-21).
//!
//! Composes existing Public API v1 reads for one Work Item into a single
//! consolidated response: metadata, description, server-reported links,
//! labels, comments, activity, and the authenticated user's watcher state.
//!
//! The bundle is **data, not instructions**: it contains only what Public
//! API v1 reads returned, re-presented verbatim. No workflow meaning is
//! computed client-side — "ready", "blocked", transition validity, and
//! every other semantic remain authoritative on the server. Truncation is
//! always explicit; nothing is silently clipped.

use std::fmt::Write as _;

use serde_json::{Value, json};

use crate::app::Session;
use crate::args::{ContextFormatArg, WorkContextArgs};
use crate::error::CliError;

use hamstik_api_client::WorkItem;

use super::emit_json;

/// Envelope schema version for the JSON bundle.
pub(crate) const BUNDLE_VERSION: u64 = 1;

/// Result envelope for one `work context` invocation.
pub(crate) struct ContextBundle {
    /// The Work Item metadata, verbatim from the API.
    pub item: WorkItem,
    /// Links as reported by the server (first page).
    pub links: Value,
    /// Comments as reported by the server (oldest first).
    pub comments: Value,
    /// True when more comments exist than were included.
    pub comments_more: bool,
    /// Activity as reported by the server (newest first).
    pub activity: Value,
    /// True when more activity pages exist than were included.
    pub activity_more: bool,
    /// The authenticated user's watcher state, when available.
    pub watcher: Value,
}

/// Fetches every section for one Work Item and returns the bundle.
async fn fetch_bundle(
    session: &Session<'_>,
    args: &WorkContextArgs,
) -> Result<ContextBundle, CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let item_response = api
        .get_work_item(&org, &project, &args.key)
        .await
        .map_err(CliError::from_client)?;
    let item = item_response.value.clone();

    // Watcher state may be unavailable to some credential scopes; a failure
    // is non-fatal and simply omits the section (with a marker).
    let watcher = match api.get_work_item_watcher(&org, &project, &args.key).await {
        Ok(response) => Some(response.raw),
        Err(_) => None,
    };

    let comments = if args.comments == 0 {
        None
    } else {
        let response = api
            .list_comments(
                &org,
                &project,
                &args.key,
                hamstik_api_client::ListOptions {
                    limit: Some(args.comments),
                    cursor: None,
                },
            )
            .await
            .map_err(CliError::from_client)?;
        Some(response)
    };

    let activity = if args.activity == 0 {
        None
    } else {
        let response = api
            .list_work_item_activity(
                &org,
                &project,
                &args.key,
                hamstik_api_client::ActivityOptions {
                    limit: Some(args.activity),
                    cursor: None,
                    since: None,
                },
            )
            .await
            .map_err(CliError::from_client)?;
        Some(response)
    };

    let links = api
        .list_work_item_links(
            &org,
            &project,
            &args.key,
            hamstik_api_client::ListOptions {
                limit: None,
                cursor: None,
            },
        )
        .await
        .map_err(CliError::from_client)?;

    let activity_more = activity.as_ref().is_some_and(|r| r.value.page.has_more);
    let comments_more = comments.as_ref().is_some_and(|r| r.value.page.has_more);

    Ok(ContextBundle {
        item,
        activity: activity.map_or(
            json!({ "items": [], "note": "omitted by --activity 0" }),
            |r| json!({ "items": r.value.items, "page": r.value.page }),
        ),
        activity_more,
        comments: comments.map_or_else(
            || {
                json!({
                    "items": [],
                    "note": "omitted by --comments 0",
                })
            },
            |r| json!({ "items": r.value.items, "page": r.value.page }),
        ),
        comments_more,
        links: json!({ "items": links.value.items, "page": links.value.page }),
        watcher: watcher.unwrap_or(Value::Null),
    })
}

/// Runs `work context`.
pub async fn run(session: &mut Session<'_>, args: &WorkContextArgs) -> Result<(), CliError> {
    let bundle = fetch_bundle(session, args).await?;

    if session.json() || args.format == ContextFormatArg::Json {
        let envelope = json_envelope(&bundle, args)?;
        return emit_json(session, &envelope);
    }

    match args.format {
        ContextFormatArg::Markdown => {
            let doc = markdown(&bundle)?;
            session.out.line(&doc).map_err(CliError::general)
        }
        _ => {
            let doc = human(&bundle)?;
            session.out.line(&doc).map_err(CliError::general)
        }
    }
}

/// Builds the stable JSON envelope: deterministic layout, server data only.
fn json_envelope(bundle: &ContextBundle, args: &WorkContextArgs) -> Result<Value, CliError> {
    let item = &bundle.item;
    let description = match item.description {
        Some(ref text) if args.compact => {
            json!({ "truncated": true, "note": "omitted by --compact" })
        }
        Some(ref text) => json!(text),
        None => Value::Null,
    };

    let mut envelope = json!({
        "bundleVersion": BUNDLE_VERSION,
        "item": {
            "key": item.key,
            "title": item.title,
            "type": item.item_type,
            "status": item.status,
            "priority": item.priority,
            "assignee": item.assignee.as_ref().map(|u| json!({"publicId": u.public_id(), "name": u.name()})),
            "reporter": item.reporter.as_ref().map(|u| json!({"publicId": u.public_id(), "name": u.name()})),
            "sprint": item.sprint.as_ref().map(|s| json!(s)),
            "parent": item.parent.as_ref().map(|p| json!(p)),
            "labels": item.labels,
            "storyPoints": item.story_points,
            "dueDate": item.due_date,
            "archivedAt": item.archived_at,
            "createdAt": item.created_at,
            "updatedAt": item.updated_at,
            "description": description,
        },
        "links": bundle.links,
        "watcher": if bundle.watcher.is_null() {
            json!({ "note": "watcher state unavailable for this credential" })
        } else {
            bundle.watcher.clone()
        },
    });

    if args.comments == 0 {
        envelope["comments"] = json!({ "note": "omitted by --comments 0" });
    } else {
        envelope["comments"] = json!({
            "items": bundle.comments["items"],
            "hasMore": bundle.comments_more,
        });
    }

    if args.activity == 0 {
        envelope["activity"] = json!({ "note": "omitted by --activity 0" });
    } else {
        envelope["activity"] = json!({
            "items": bundle.activity["items"],
            "hasMore": bundle.activity_more,
        });
    }

    Ok(envelope)
}

/// Renders the Markdown document with explicit truncation markers.
fn markdown(bundle: &ContextBundle) -> Result<String, CliError> {
    let item = &bundle.item;
    let mut out = String::new();
    let _ = writeln!(out, "# {} — {}", item.key, item.title);
    let _ = writeln!(out);
    let _ = writeln!(out, "- type: {}", item.item_type);
    let _ = writeln!(out, "- status: {}", item.status);
    let _ = writeln!(out, "- priority: {}", item.priority);
    if let Some(assignee) = &item.assignee {
        let _ = writeln!(out, "- assignee: {}", assignee.name());
    }
    if let Some(sprint) = &item.sprint {
        let _ = writeln!(out, "- sprint: {}", sprint.name);
    }
    if let Some(parent) = &item.parent {
        let _ = writeln!(out, "- parent: {}", parent.key);
    }
    if !item.labels.is_empty() {
        let names: Vec<&str> = item.labels.iter().map(|l| l.name.as_str()).collect();
        let _ = writeln!(out, "- labels: {}", names.join(", "));
    }
    if let Some(points) = item.story_points {
        let _ = writeln!(out, "- story points: {points}");
    }
    let _ = writeln!(out);

    if let Some(text) = &item.description {
        let _ = writeln!(out, "## Description");
        let _ = writeln!(out, "{text}");
    }

    let links = &bundle.links;
    if let Some(items) = links["items"].as_array()
        && !items.is_empty()
    {
        let _ = writeln!(out, "## Links");
        for link in items {
            let other = &link["otherWorkItem"];
            let _ = writeln!(
                out,
                "- {} {} ({})",
                link["relation"].as_str().unwrap_or("?"),
                other["key"].as_str().unwrap_or("?"),
                other["title"].as_str().unwrap_or("")
            );
        }
    }

    if let Some(items) = bundle.comments["items"].as_array()
        && !items.is_empty()
    {
        let _ = writeln!(out, "## Comments");
        for comment in items {
            let author = &comment["author"]["name"];
            let _ = writeln!(out, "### {}", author.as_str().unwrap_or("?"));
            let _ = writeln!(out, "{}", comment["body"].as_str().unwrap_or(""));
        }
    }
    if bundle.comments_more {
        let _ = writeln!(out, "_(more comments available; increase --comments)_");
    }

    if let Some(items) = bundle.activity["items"].as_array()
        && !items.is_empty()
    {
        let _ = writeln!(out, "## Activity");
        for event in items {
            let actor = &event["actor"]["name"];
            let _ = writeln!(
                out,
                "- {}: {} — {}",
                event["createdAt"].as_str().unwrap_or("?"),
                event["action"].as_str().unwrap_or("?"),
                actor.as_str().unwrap_or("(system)")
            );
        }
    }
    if bundle.activity_more {
        let _ = writeln!(out, "_(more activity available; increase --activity)_");
    }

    if !bundle.watcher.is_null() {
        let _ = writeln!(out, "## Watcher state (authenticated user)");
        let _ = writeln!(out, "```json\n{}\n```", bundle.watcher);
    }

    Ok(out)
}

/// Renders the human (default) view: compact aligned text.
fn human(bundle: &ContextBundle) -> Result<String, CliError> {
    let item = &bundle.item;
    let mut out = String::new();
    let _ = writeln!(out, "{} — {}", item.key, item.title);
    let _ = writeln!(
        out,
        "type {} | status {} | priority {}",
        item.item_type, item.status, item.priority
    );
    if let Some(assignee) = &item.assignee {
        let _ = writeln!(out, "assignee: {}", assignee.name());
    }
    if let Some(sprint) = &item.sprint {
        let _ = writeln!(out, "sprint: {}", sprint.name);
    }
    if !item.labels.is_empty() {
        let names: Vec<&str> = item.labels.iter().map(|l| l.name.as_str()).collect();
        let _ = writeln!(out, "labels: {}", names.join(", "));
    }
    if let Some(parent) = &item.parent {
        let _ = writeln!(out, "parent: {}", parent.key);
    }
    if let Some(items) = bundle.links["items"].as_array()
        && !items.is_empty()
    {
        let _ = writeln!(out, "links: {}", items.len());
        for link in items {
            let _ = writeln!(
                out,
                "  {} {}",
                link["relation"].as_str().unwrap_or("?"),
                link["otherWorkItem"]["key"].as_str().unwrap_or("?")
            );
        }
    }
    if let Some(comments) = bundle.comments["items"].as_array() {
        let _ = writeln!(out, "comments: {}", comments.len());
        for comment in comments {
            let _ = writeln!(
                out,
                "  {}: {}",
                comment["author"]["name"].as_str().unwrap_or("?"),
                comment["body"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect::<String>()
            );
        }
    }
    if bundle.comments_more {
        let _ = writeln!(out, "  (more comments available; increase --comments)");
    }
    if let Some(events) = bundle.activity["items"].as_array() {
        let _ = writeln!(out, "activity: {}", events.len());
        for event in events {
            let _ = writeln!(
                out,
                "  {}: {}",
                event["action"].as_str().unwrap_or("?"),
                event["createdAt"].as_str().unwrap_or("?")
            );
        }
    }
    if bundle.activity_more {
        let _ = writeln!(out, "  (more activity available; increase --activity)");
    }
    if !bundle.watcher.is_null() {
        let _ = writeln!(out, "watcher: {}", bundle.watcher);
    }
    Ok(out)
}
