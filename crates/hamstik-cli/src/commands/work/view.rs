// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work view`.

use std::sync::Arc;

use serde_json::json;

use tokio::sync::Semaphore;

use hamstik_api_client::{ActivityOptions, HamstikApi, ListOptions, WorkItem};

use crate::app::Session;
use crate::args::WorkViewArgs;
use crate::error::CliError;

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
            "too many keys: {}/{} (maximum {MAX_KEYS} per invocation)",
            keys.len(),
            MAX_KEYS
        )));
    }

    let single = keys.len() == 1;
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    // Section selection applies to batch reads; the single-item bundle with
    // sections is `work context`, not `work view`.
    if single && (args.comments > 0 || args.activity > 0 || args.links > 0) {
        return Err(CliError::usage(
            "--comments/--activity/--links apply to batch reads (two or more keys); \
             for one item use `hamstik work context KEY`",
        ));
    }

    let semaphore = Arc::new(Semaphore::new(DEFAULT_CONCURRENCY));
    let mut tasks = Vec::with_capacity(keys.len());
    let spec = FetchSpec::from(args);

    for key in keys {
        let api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let sem = semaphore.clone();
        let handle = tokio::spawn(async move {
            // One permit covers the whole item (item + requested sections),
            // bounding in-flight requests to DEFAULT_CONCURRENCY.
            match sem.acquire().await {
                Ok(_permit) => fetch_one(&api, &org, &project, &key, spec).await,
                Err(e) => FetchedKey {
                    key,
                    result: Err(CliError::general(format!(
                        "concurrency control failed: {e}"
                    ))),
                },
            }
        });
        tasks.push(handle);
    }

    if single {
        let handle = tasks
            .into_iter()
            .next()
            .ok_or_else(|| CliError::general("no result for single key"))?;
        let fetched = handle
            .await
            .map_err(|e| CliError::general(format!("task failed: {e}")))?;
        let key = &fetched.key;
        match fetched.result {
            Ok(item) => emit_view(session, &item.raw, key, |session| {
                render_work_item(session, &item.item, args.compact)
            }),
            Err(err) => Err(err),
        }
    } else {
        let mut results: Vec<ItemResult> = Vec::with_capacity(tasks.len());
        let mut worst: Option<CliError> = None;

        for handle in tasks {
            let fetched = handle
                .await
                .map_err(|e| CliError::general(format!("task failed: {e}")))?;
            match fetched.result {
                Ok(item) => results.push(ItemResult::success(fetched.key, item)),
                Err(err) => {
                    worst = Some(
                        worst
                            .map(|w| worst_exit(w, &err))
                            .unwrap_or_else(|| err.clone()),
                    );
                    results.push(ItemResult::failure(fetched.key, err));
                }
            }
        }

        for result in &mut results {
            if let Outcome::Success(item) = &mut result.outcome
                && args.compact
            {
                apply_compact(&mut item.raw);
            }
        }
        let failures = results.iter().filter(|r| r.outcome.is_failure()).count();
        let envelope = json!({
            "items": results.iter().map(ItemResult::to_json).collect::<Vec<_>>(),
            "failures": failures,
            "total": results.len(),
        });
        // The envelope is always the single stdout document; partial-failure
        // detail rides on the stderr error envelope and the exit code (the
        // most severe per-item code, 0 when every item succeeded).
        emit_json(session, &envelope)?;
        match worst {
            Some(err) => Err(err),
            None => Ok(()),
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

struct FetchedItem {
    /// Parsed item, for the single-key human rendering.
    item: WorkItem,
    /// Server item JSON verbatim (compacted when `--compact` in batch mode).
    raw: serde_json::Value,
    /// Present only when the section was requested (`--comments N`, N > 0).
    comments: Option<serde_json::Value>,
    /// Present only when the section was requested (`--activity N`, N > 0).
    activity: Option<serde_json::Value>,
    /// Present only when the section was requested (`--links N`, N > 0).
    links: Option<serde_json::Value>,
}

struct FetchedKey {
    key: String,
    result: Result<FetchedItem, CliError>,
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
            Outcome::Success(item) => {
                let mut value = json!({
                    "key": self.key,
                    "status": "ok",
                    "item": item.raw,
                });
                if let Some(comments) = &item.comments {
                    value["comments"] = comments.clone();
                }
                if let Some(activity) = &item.activity {
                    value["activity"] = activity.clone();
                }
                if let Some(links) = &item.links {
                    value["links"] = links.clone();
                }
                value
            }
            Outcome::Failure(err) => json!({
                "key": self.key,
                // CliError::to_json wraps in an outer "error" object; the
                // per-item entry carries that object directly.
                "status": "error",
                "error": err.to_json()["error"].clone(),
            }),
        }
    }
}

enum Outcome {
    Success(Box<FetchedItem>),
    Failure(CliError),
}

impl Outcome {
    fn is_failure(&self) -> bool {
        matches!(self, Self::Failure(_))
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

/// Per-item fetch options resolved once from parsed arguments.
#[derive(Clone, Copy)]
struct FetchSpec {
    comments: u32,
    activity: u32,
    links: u32,
    compact: bool,
}

impl From<&WorkViewArgs> for FetchSpec {
    fn from(args: &WorkViewArgs) -> Self {
        Self {
            comments: args.comments,
            activity: args.activity,
            links: args.links,
            compact: args.compact,
        }
    }
}

async fn fetch_one(
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
    spec: FetchSpec,
) -> FetchedKey {
    let result = fetch_item(api, org, project, key, spec).await;
    FetchedKey {
        key: key.to_string(),
        result,
    }
}

async fn fetch_item(
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
    spec: FetchSpec,
) -> Result<FetchedItem, CliError> {
    let response = api
        .get_work_item(org, project, key)
        .await
        .map_err(CliError::from_client)?;
    let item = response.value.clone();
    let raw = response.raw.clone();

    let mut comments = None;
    if spec.comments > 0 {
        let resp = api
            .list_comments(
                org,
                project,
                key,
                ListOptions {
                    limit: Some(spec.comments),
                    cursor: None,
                },
            )
            .await
            .map_err(CliError::from_client)?;
        let mut section = json!({ "items": resp.value.items, "page": resp.value.page });
        if spec.compact {
            truncate_comment_bodies(&mut section);
        }
        comments = Some(section);
    }

    let mut activity = None;
    if spec.activity > 0 {
        let resp = api
            .list_work_item_activity(
                org,
                project,
                key,
                ActivityOptions {
                    limit: Some(spec.activity),
                    cursor: None,
                    since: None,
                },
            )
            .await
            .map_err(CliError::from_client)?;
        activity = Some(json!({ "items": resp.value.items, "page": resp.value.page }));
    }

    let mut links = None;
    if spec.links > 0 {
        let resp = api
            .list_work_item_links(
                org,
                project,
                key,
                ListOptions {
                    limit: Some(spec.links),
                    cursor: None,
                },
            )
            .await
            .map_err(CliError::from_client)?;
        links = Some(json!({ "items": resp.value.items, "page": resp.value.page }));
    }

    Ok(FetchedItem {
        item,
        raw,
        comments,
        activity,
        links,
    })
}

fn apply_compact(raw: &mut serde_json::Value) {
    if raw.get("description").is_some() {
        raw["description"] = json!({ "truncated": true, "note": "omitted by --compact" });
    }
}

fn truncate_comment_bodies(section: &mut serde_json::Value) {
    if let Some(items) = section.get_mut("items").and_then(|v| v.as_array_mut()) {
        for item in items {
            if item.get("body").is_some() {
                item["body"] = json!({ "truncated": true, "note": "omitted by --compact" });
            }
        }
    }
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
