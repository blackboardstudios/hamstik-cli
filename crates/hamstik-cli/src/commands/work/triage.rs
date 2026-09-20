// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work triage` — a composed "what needs my attention" view.
//!
//! One invocation merges three existing Public API v1 reads for the resolved
//! Organization context:
//!
//! * `assignedOpen` — open Work Items assigned to the authenticated user, the
//!   `work mine --scope open` read (`GET /my/work`);
//! * `overdue` — overdue Work Items assigned to the authenticated user, the
//!   `work mine --overdue true` read (`GET /my/work`);
//! * `activity` — the newest Project activity events, the `project activity`
//!   read (`GET .../projects/{key}/activity`), included only when a Project is
//!   resolved.
//!
//! This is a **client-side composition of already-shipped reads**. It computes
//! no new semantics: every field is forwarded verbatim from the server, and the
//! only client-side judgment is which existing read failed. Section failures
//! are surfaced per-section instead of aborting the whole command; the exit
//! code stays successful so a partial snapshot is still usable.
//!
//! Public API v1 does not expose an "unread", "mention", or "watched items"
//! read, so the activity section is the Project feed (the only recent-activity
//! source the documented contract provides). No local "seen" state is kept.

use std::sync::Arc;

use serde_json::{Value, json};

use hamstik_api_client::{ActivityOptions, HamstikApi, ListWorkItemsQuery, PageItems, follow_with};

use crate::app::Session;
use crate::args::{PaginationArgs, TriageArgs};
use crate::error::CliError;

use super::{emit_json, emit_table, follow_policy};

/// Envelope schema version for the JSON triage document.
pub(crate) const TRIAGE_VERSION: u64 = 1;

/// Human/quiet headers for the Work Item sections.
const WORK_HEADERS: &[&str] = &[
    "KEY", "PROJECT", "TITLE", "STATUS", "PRIORITY", "DUE", "ASSIGNEE",
];

/// Human/quiet headers for the activity section.
const ACTIVITY_HEADERS: &[&str] = &["ID", "CREATED", "ACTION", "WORK ITEM", "ACTOR"];

/// One section's outcome: a fetched page, a recorded failure, or a reason the
/// section was not applicable to this invocation.
enum Section {
    /// The read succeeded; the value is the raw `{items, page}` collection.
    Ok(Value),
    /// The read failed; the error is reported, never swallowed.
    Failed(CliError),
    /// The section was not applicable (no Project resolved, or disabled).
    Skipped(String),
}

impl Section {
    /// Wraps a fetched value or its failure.
    fn from_result(result: Result<Value, CliError>) -> Self {
        match result {
            Ok(value) => Self::Ok(value),
            Err(error) => Self::Failed(error),
        }
    }

    /// True when this section failed to load.
    fn is_failure(&self) -> bool {
        matches!(self, Self::Failed(_))
    }

    /// Renders the stable per-section JSON object.
    fn to_json(&self) -> Value {
        match self {
            Self::Ok(value) => json!({
                "status": "ok",
                "items": value.get("items").cloned().unwrap_or_else(|| json!([])),
                "page": value.get("page").cloned().unwrap_or(Value::Null),
            }),
            Self::Failed(error) => {
                let mut doc = json!({ "status": "error" });
                doc["error"] = error.to_json()["error"].clone();
                doc
            }
            Self::Skipped(reason) => json!({ "status": "skipped", "reason": reason }),
        }
    }

    /// The section's items array (empty when not loaded).
    fn items(&self) -> &[Value] {
        match self {
            Self::Ok(value) => value
                .get("items")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            _ => &[],
        }
    }
}

/// Runs `work triage`.
pub(crate) async fn triage(session: &mut Session<'_>, args: &TriageArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    crate::time_arg::report_resolved(&mut session.out, &[("--since", args.since.as_ref())]);

    // The Project is optional: only the activity section needs it, and its
    // absence degrades that one section rather than the whole command.
    let project = selection.project.value.clone();

    let assigned_query = ListWorkItemsQuery {
        limit: args.pagination.page_size(),
        cursor: args.pagination.cursor.clone(),
        scope: Some("open".to_string()),
        ..Default::default()
    };
    let overdue_query = ListWorkItemsQuery {
        limit: args.pagination.page_size(),
        cursor: args.pagination.cursor.clone(),
        overdue: Some(true),
        ..Default::default()
    };

    // The three reads are independent; run them concurrently.
    let (assigned, overdue, activity) = tokio::join!(
        fetch_my_work(api.clone(), assigned_query, &args.pagination),
        fetch_my_work(api.clone(), overdue_query, &args.pagination),
        fetch_project_activity(api, &org, project.as_deref(), args),
    );
    let assigned = Section::from_result(assigned);
    let overdue = Section::from_result(overdue);

    if session.json() {
        let envelope = envelope(&assigned, &overdue, &activity);
        return emit_json(session, &envelope);
    }

    render_human(session, &assigned, &overdue, &activity)
}

/// Fetches one `/my/work` section, honoring the shared pagination flags.
async fn fetch_my_work(
    api: Arc<dyn HamstikApi>,
    query: ListWorkItemsQuery,
    pagination: &PaginationArgs,
) -> Result<Value, CliError> {
    if pagination.all {
        let base = query;
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let api = api.clone();
            let mut query = base.clone();
            query.cursor = cursor;
            async move {
                let response = api.list_my_work(query).await?;
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        })
        .await
        .map_err(CliError::from_client)?;
        Ok(json!({ "items": page.raw_items, "page": page.page }))
    } else {
        let response = api
            .list_my_work(query)
            .await
            .map_err(CliError::from_client)?;
        Ok(response.raw)
    }
}

/// Fetches the recent Project activity section, or reports why it is skipped.
async fn fetch_project_activity(
    api: Arc<dyn HamstikApi>,
    org: &str,
    project: Option<&str>,
    args: &TriageArgs,
) -> Section {
    if args.activity == 0 {
        return Section::Skipped("disabled by --activity 0".to_string());
    }
    let Some(project) = project else {
        return Section::Skipped(
            "no project selected; pass --project, set HAMSTIK_PROJECT, or run `hamstik project use <key>`"
                .to_string(),
        );
    };
    let opts = ActivityOptions {
        limit: Some(args.activity),
        cursor: None,
        since: args.since.as_ref().map(|d| d.to_string()),
    };
    match api.list_project_activity(org, project, opts).await {
        Ok(response) => Section::Ok(response.raw),
        Err(error) => Section::Failed(CliError::from_client(error)),
    }
}

/// Builds the stable JSON document with every section present and failures
/// marked (never silently omitted).
fn envelope(assigned: &Section, overdue: &Section, activity: &Section) -> Value {
    let mut failed = Vec::new();
    if assigned.is_failure() {
        failed.push("assignedOpen");
    }
    if overdue.is_failure() {
        failed.push("overdue");
    }
    if activity.is_failure() {
        failed.push("activity");
    }
    json!({
        "triageVersion": TRIAGE_VERSION,
        "sections": {
            "assignedOpen": assigned.to_json(),
            "overdue": overdue.to_json(),
            "activity": activity.to_json(),
        },
        "failedSections": failed,
    })
}

/// Renders the human (or quiet) view, reporting failures on stderr.
fn render_human(
    session: &mut Session<'_>,
    assigned: &Section,
    overdue: &Section,
    activity: &Section,
) -> Result<(), CliError> {
    let quiet = session.out.is_quiet();

    if !quiet {
        session.out.line("Work triage").map_err(CliError::general)?;
    }

    render_work_section(session, "Assigned (open)", assigned, quiet)?;
    render_work_section(session, "Overdue", overdue, quiet)?;
    render_activity_section(session, activity, quiet)?;

    // A failed section is reported, but never aborts the others.
    for (label, section) in [
        ("assignedOpen", assigned),
        ("overdue", overdue),
        ("activity", activity),
    ] {
        if let Section::Failed(error) = section {
            session.out.warn(&format!(
                "warning: {label} section failed: {}",
                error.message
            ));
            if !quiet {
                session
                    .out
                    .line(&format!(
                        "  {label}: failed ({}): {}",
                        error.code, error.message
                    ))
                    .map_err(CliError::general)?;
            }
        }
    }

    Ok(())
}

/// Renders one Work Item section as a table (or identifiers in quiet mode).
fn render_work_section(
    session: &mut Session<'_>,
    label: &str,
    section: &Section,
    quiet: bool,
) -> Result<(), CliError> {
    if !quiet {
        let heading = match section {
            Section::Ok(_) => format!("\n{label} — {} item(s)", section.items().len()),
            Section::Failed(_) => format!("\n{label} — unavailable"),
            Section::Skipped(_) => format!("\n{label} — skipped"),
        };
        session.out.line(&heading).map_err(CliError::general)?;
    }
    let Section::Ok(value) = section else {
        return Ok(());
    };
    let rows: Vec<Vec<String>> = section
        .items()
        .iter()
        .map(|item| {
            vec![
                raw_string(item, "key"),
                item.get("project")
                    .and_then(|project| project.get("key"))
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
                raw_string(item, "title"),
                raw_string(item, "status"),
                raw_string(item, "priority"),
                raw_string(item, "dueDate"),
                item.get("assignee")
                    .and_then(|assignee| assignee.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
            ]
        })
        .collect();
    emit_table(session, value, WORK_HEADERS, &rows)
}

/// Renders the activity section, or the reason it is unavailable.
fn render_activity_section(
    session: &mut Session<'_>,
    section: &Section,
    quiet: bool,
) -> Result<(), CliError> {
    match section {
        Section::Ok(value) => {
            if !quiet {
                session
                    .out
                    .line(&format!(
                        "\nRecent project activity — {} event(s)",
                        section.items().len()
                    ))
                    .map_err(CliError::general)?;
            }
            let rows: Vec<Vec<String>> = section
                .items()
                .iter()
                .map(|event| {
                    let work_item = event.get("workItem");
                    vec![
                        raw_string(event, "id"),
                        raw_string(event, "createdAt"),
                        raw_string(event, "action"),
                        work_item
                            .and_then(|item| item.get("key"))
                            .and_then(Value::as_str)
                            .unwrap_or("-")
                            .to_string(),
                        event
                            .get("actor")
                            .and_then(|actor| actor.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("(system)")
                            .to_string(),
                    ]
                })
                .collect();
            emit_table(session, value, ACTIVITY_HEADERS, &rows)
        }
        Section::Skipped(reason) => {
            if !quiet {
                session
                    .out
                    .line(&format!("\nRecent project activity — skipped ({reason})"))
                    .map_err(CliError::general)?;
            }
            Ok(())
        }
        Section::Failed(_) => Ok(()),
    }
}

/// A raw string field, or `-` when absent/null.
fn raw_string(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .unwrap_or("-")
        .to_string()
}
