// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work diff` (CLI-56): what changed on a Work Item since a reference point.
//!
//! Two comparison modes, both server-authoritative:
//!
//! - **Baseline mode** (default): compares the current item against a local
//!   baseline recorded with `--save-baseline`. The baseline is a per-user
//!   file under the config directory (`baselines/<org>/<project>/<KEY>.json`),
//!   contains the observed item state and no credentials, and is replaced
//!   only by another `--save-baseline`. This produces a complete field-level
//!   before/after diff.
//!
//! - **Revision mode** (`--since <revision>`): the Public API v1 contract
//!   exposes no Work Item revision snapshots, so this mode reports the
//!   revision delta plus the field changes the server records in the item's
//!   activity feed, with explicit partial coverage — it never fabricates a
//!   diff. The item's revision doubles as the `ETag` value.
//!
//! Unchanged comparisons exit 0 with an explicit no-changes result; with
//! `--fail-on-change` a changed comparison exits with the conflict exit code.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};

use hamstik_api_client::{ActivityOptions, HamstikApi, UserSummary, WorkItem};

use crate::app::Session;
use crate::args::WorkDiffArgs;
use crate::error::CliError;
use crate::exit;
use crate::fsutil;

use super::emit_json;
use crate::commands::render_list;

/// Schema version of the diff document; bump only on a breaking layout change.
const DIFF_VERSION: u64 = 1;

/// Schema version of the stored baseline file.
const BASELINE_VERSION: u64 = 1;

/// Upper bound on activity pages fetched in `--since` mode.
const MAX_ACTIVITY_PAGES: usize = 20;

/// The stable public-id projection of a user summary, for comparison.
fn user_value(user: &UserSummary) -> Value {
    let public_id = user.public_id().or_else(|| user.legacy_id());
    json!({
        "publicId": public_id,
        "name": user.name(),
    })
}

/// Fields compared between baseline and current state, in this order.
///
/// Everything else in the item body is deliberately excluded and named in
/// the diff document: identity fields (`id`, `key`, `projectId`), volatile
/// bookkeeping (`revision`, `createdAt`, `updatedAt`, `voteCount`,
/// `hasVoted`), server-computed state (`effortRisk`, `effortRiskReason`),
/// and social state (`reactions`).
fn field_values(item: &WorkItem) -> Vec<(&'static str, Value)> {
    fn named<T>(
        value: &Option<T>,
        id: impl Fn(&T) -> &String,
        name: impl Fn(&T) -> &String,
    ) -> Value {
        value.as_ref().map_or(
            Value::Null,
            |inner| json!({ "id": id(inner), "name": name(inner) }),
        )
    }
    vec![
        ("title", json!(item.title)),
        ("description", json!(item.description)),
        ("type", json!(item.item_type)),
        ("status", json!(item.status)),
        ("priority", json!(item.priority)),
        ("storyPoints", json!(item.story_points)),
        ("dueDate", json!(item.due_date)),
        (
            "assignee",
            item.assignee.as_ref().map_or(Value::Null, user_value),
        ),
        (
            "reporter",
            item.reporter.as_ref().map_or(Value::Null, user_value),
        ),
        ("sprint", named(&item.sprint, |s| &s.id, |s| &s.name)),
        (
            "parent",
            item.parent.as_ref().map_or(Value::Null, |p| json!(p)),
        ),
        (
            "labels",
            json!(
                item.labels
                    .iter()
                    .map(|label| json!({ "id": label.id, "name": label.name }))
                    .collect::<Vec<_>>()
            ),
        ),
        (
            "attributes",
            json!(item
                .attributes
                .iter()
                .map(|attribute| json!({
                    "key": attribute.key,
                    "name": attribute.name,
                    "booleanValue": attribute.boolean_value,
                    "options": attribute.options.iter().map(|o| o.key.clone()).collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>()),
        ),
        ("archivedAt", json!(item.archived_at)),
    ]
}

/// One changed field with before/after values.
struct FieldChange {
    field: &'static str,
    before: Value,
    after: Value,
}

/// Compares the field projections; order follows the projection list.
fn diff_items(old: &WorkItem, new: &WorkItem) -> Vec<FieldChange> {
    let old_values = field_values(old);
    let new_values = field_values(new);
    old_values
        .into_iter()
        .zip(new_values)
        .filter_map(|((field, before), (_, after))| {
            (before != after).then_some(FieldChange {
                field,
                before,
                after,
            })
        })
        .collect()
}

/// Single-line rendering of a diff value for the human table.
fn render_value(value: &Value) -> String {
    match value {
        Value::Null => "(null)".to_string(),
        Value::String(text) => text.replace('\n', "\\n"),
        other => serde_json::to_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// The per-user baseline file path for one item.
fn baseline_path(session: &Session<'_>, org: &str, project: &str, key: &str) -> PathBuf {
    session
        .config
        .path()
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("baselines")
        .join(org)
        .join(project)
        .join(format!("{key}.json"))
}

/// Writes the baseline atomically with owner-restricted permissions.
fn write_baseline(
    path: &std::path::Path,
    org: &str,
    project: &str,
    key: &str,
    item: &WorkItem,
) -> Result<(), CliError> {
    let document = json!({
        "baselineVersion": BASELINE_VERSION,
        "organization": org,
        "project": project,
        "key": key,
        "savedAt": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "revision": item.revision,
        "item": item,
    });
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            CliError::general(format!("cannot create baseline directory ({err})"))
        })?;
    }
    let serialized = serde_json::to_vec_pretty(&document)
        .map_err(|err| CliError::general(format!("cannot serialize baseline ({err})")))?;
    fsutil::write_atomic(path, &serialized)
        .and_then(|()| fsutil::restrict_permissions(path))
        .map_err(|err| CliError::general(format!("cannot write baseline file ({err})")))?;
    Ok(())
}

/// Loads and validates the stored baseline, returning its document.
fn load_baseline(
    path: &std::path::Path,
    org: &str,
    project: &str,
    key: &str,
) -> Result<Value, CliError> {
    let bytes = fs::read(path).map_err(|_| {
        CliError::not_found(format!(
            "no baseline stored for {key}; record one with `hamstik work diff {key} --save-baseline`"
        ))
    })?;
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|err| CliError::general(format!("baseline file is corrupt ({err})")))?;
    if document["baselineVersion"].as_u64() != Some(BASELINE_VERSION) {
        return Err(CliError::general(format!(
            "baseline for {key} uses an unsupported schema version; \
             re-save it with `hamstik work diff {key} --save-baseline`"
        )));
    }
    let matches = document["organization"].as_str() == Some(org)
        && document["project"].as_str() == Some(project)
        && document["key"].as_str() == Some(key);
    if !matches {
        return Err(CliError::usage(
            "stored baseline belongs to a different Organization/Project/key; \
             re-save it in this context with `hamstik work diff <KEY> --save-baseline`",
        ));
    }
    Ok(document)
}

/// Applies `--fail-on-change` after rendering.
fn apply_fail_on_change(session: &mut Session<'_>, args: &WorkDiffArgs, has_changes: bool) {
    if args.fail_on_change && has_changes {
        session.exit_code = exit::CONFLICT;
    }
}

/// The resolved request context for one diff invocation.
struct Context {
    org: String,
    project: String,
    api: Arc<dyn HamstikApi>,
}

/// Resolves Organization/Project and the API client once per invocation.
fn context(session: &Session<'_>) -> Result<Context, CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    Ok(Context { org, project, api })
}

/// Runs `hamstik work diff`.
pub(super) async fn diff(session: &mut Session<'_>, args: &WorkDiffArgs) -> Result<(), CliError> {
    let Context { org, project, api } = context(session)?;

    if args.save_baseline {
        let item = api
            .get_work_item(&org, &project, &args.key)
            .await
            .map_err(CliError::from_client)?
            .value;
        let path = baseline_path(session, &org, &project, &args.key);
        write_baseline(&path, &org, &project, &args.key, &item)?;
        if session.out.is_json() {
            emit_json(
                session,
                &json!({
                    "baselineVersion": BASELINE_VERSION,
                    "saved": true,
                    "item": args.key,
                    "path": path.display().to_string(),
                    "revision": item.revision,
                }),
            )?;
        } else {
            session
                .out
                .human(&format!(
                    "baseline saved for {} (revision {}): {}",
                    args.key,
                    item.revision,
                    path.display()
                ))
                .map_err(CliError::general)?;
        }
        return Ok(());
    }

    let current = api
        .get_work_item(&org, &project, &args.key)
        .await
        .map_err(CliError::from_client)?
        .value;

    if let Some(since) = args.since {
        return diff_since(session, args, &org, &project, &api, &current, since).await;
    }

    let path = baseline_path(session, &org, &project, &args.key);
    let baseline = load_baseline(&path, &org, &project, &args.key)?;
    let recorded: WorkItem = serde_json::from_value(baseline["item"].clone()).map_err(|err| {
        CliError::general(format!(
            "baseline for {} is unreadable ({err}); re-save with `hamstik work diff {} --save-baseline`",
            args.key, args.key
        ))
    })?;

    let changes = diff_items(&recorded, &current);
    let has_changes = !changes.is_empty();
    let document = json!({
        "diffVersion": DIFF_VERSION,
        "item": args.key,
        "reference": {
            "kind": "baseline",
            "savedAt": baseline["savedAt"],
            "revision": baseline["revision"],
            "path": path.display().to_string(),
        },
        "currentRevision": current.revision,
        "hasChanges": has_changes,
        "changes": changes.iter().map(|change| json!({
            "field": change.field,
            "before": change.before,
            "after": change.after,
        })).collect::<Vec<_>>(),
        "excludedFields": [
            "id", "key", "projectId", "revision", "createdAt", "updatedAt",
            "voteCount", "hasVoted", "effortRisk", "effortRiskReason", "reactions",
        ],
        "note": "Links, comments, attachments, and watchers are separate resources and are not part of this diff.",
    });

    render_changes(session, &document, &changes, has_changes, &args.key)?;
    apply_fail_on_change(session, args, has_changes);
    Ok(())
}

/// Renders the baseline diff: human table, or the JSON envelope verbatim.
///
/// Unchanged comparisons get an explicit no-changes result in every mode.
fn render_changes(
    session: &mut Session<'_>,
    document: &Value,
    changes: &[FieldChange],
    has_changes: bool,
    key: &str,
) -> Result<(), CliError> {
    if !has_changes {
        if session.out.is_json() {
            emit_json(session, document)?;
        } else {
            let reference = &document["reference"];
            session
                .out
                .human(&format!(
                    "no changes for {key} (baseline revision {}, current revision {})",
                    reference["revision"], document["currentRevision"],
                ))
                .map_err(CliError::general)?;
        }
        return Ok(());
    }
    let rows: Vec<Vec<String>> = changes
        .iter()
        .map(|change| {
            vec![
                change.field.to_string(),
                render_value(&change.before),
                render_value(&change.after),
            ]
        })
        .collect();
    render_list(session, document, &["field", "before", "after"], &rows).map_err(CliError::general)
}

/// `--since <revision>` mode: revision delta plus the activity-feed evidence.
async fn diff_since(
    session: &mut Session<'_>,
    args: &WorkDiffArgs,
    org: &str,
    project: &str,
    api: &Arc<dyn HamstikApi>,
    current: &WorkItem,
    since: i64,
) -> Result<(), CliError> {
    if since > current.revision {
        return Err(CliError::usage(format!(
            "reference revision {since} is newer than the current revision {}",
            current.revision
        )));
    }
    let has_changes = since < current.revision;
    let mut changes: Vec<Value> = Vec::new();
    if has_changes {
        // The Public API provides no revision snapshots, so the events below
        // are the item's server-recorded field changes (newest first), not a
        // reconstruction of revision `since`. The coverage note says so.
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_ACTIVITY_PAGES {
            let opts = ActivityOptions {
                limit: Some(100),
                cursor: cursor.clone(),
                since: None,
            };
            let response = api
                .list_work_item_activity(org, project, &args.key, opts)
                .await
                .map_err(CliError::from_client)?;
            for event in &response.value.items {
                if let Some(detail) = event
                    .detail
                    .as_ref()
                    .filter(|detail| detail.get("from").is_some() && detail.get("to").is_some())
                {
                    changes.push(json!({
                        "field": field_name_for_action(&event.action),
                        "action": event.action,
                        "before": detail["from"],
                        "after": detail["to"],
                        "at": event.created_at,
                        "actor": event.actor.as_ref().map(user_value),
                    }));
                }
            }
            if response.value.page.has_more {
                cursor = response.value.page.next_cursor.clone();
                if cursor.is_none() {
                    break;
                }
            } else {
                break;
            }
        }
    }

    let mut document = json!({
        "diffVersion": DIFF_VERSION,
        "item": args.key,
        "reference": { "kind": "revision", "revision": since },
        "currentRevision": current.revision,
        "revisionDelta": { "from": since, "to": current.revision },
        "hasChanges": has_changes,
        "historySupported": false,
        "coverage": "partial",
        "changes": changes,
        "note": "The Public API v1 contract exposes no Work Item revision snapshots, so this comparison lists the server-recorded field changes visible in the item's activity feed rather than a reconstruction of the reference revision. Fields without activity events (description, story points, sprint, labels, attributes) may have changed without appearing here. For a complete before/after comparison, record a baseline with `hamstik work diff KEY --save-baseline`.",
    });
    if !has_changes {
        document["note"] = json!("no revision delta between the reference and the current state");
    }

    if !has_changes {
        if session.out.is_json() {
            emit_json(session, &document)?;
        } else {
            session
                .out
                .human(&format!(
                    "no changes for {} since revision {since} (current revision {})",
                    args.key, current.revision
                ))
                .map_err(CliError::general)?;
        }
    } else if session.out.is_json() {
        emit_json(session, &document)?;
    } else {
        let rows: Vec<Vec<String>> = document["changes"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        vec![
                            entry["field"].as_str().unwrap_or_default().to_string(),
                            render_value(&entry["before"]),
                            render_value(&entry["after"]),
                            entry["at"].as_str().unwrap_or_default().to_string(),
                        ]
                    })
                    .collect()
            })
            .unwrap_or_default();
        render_list(
            session,
            &document,
            &["field", "before", "after", "at"],
            &rows,
        )
        .map_err(CliError::general)?;
    }
    apply_fail_on_change(session, args, has_changes);
    Ok(())
}

/// Maps an activity action name onto the diffed field name.
fn field_name_for_action(action: &str) -> String {
    match action {
        "assigned" | "unassigned" => "assignee".to_string(),
        other => other.strip_suffix("_changed").unwrap_or(other).to_string(),
    }
}
