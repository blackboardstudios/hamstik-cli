// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work dashboard` — a read-only, multi-project work overview.
//!
//! One invocation composes the existing Public API v1 reads for the resolved
//! Organization context:
//!
//! * a My Work section (`GET /my/work`, the `work mine` read), restricted to
//!   the configured Project set;
//! * one section per configured Project (`GET
//!   .../projects/{key}/work-items`, the `work list` read).
//!
//! The command is strictly read-only and computes no new semantics: every
//! field is forwarded verbatim from the server. Project fetches run with
//! bounded concurrency, and a single Project failure is recorded per section
//! instead of aborting the whole command, so one failing Project never hides
//! the others' results. Like `work triage`, the exit code stays successful so
//! a partial snapshot is still usable; failures are explicit in `--json`
//! (`failedSections` and per-section `status`) and on stderr.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};

use tokio::sync::Semaphore;

use hamstik_api_client::{HamstikApi, ListWorkItemsQuery, PageItems, follow_with};

use crate::app::{Selection, Session};
use crate::args::{PaginationArgs, WorkDashboardArgs};
use crate::error::CliError;

use super::{emit_json, emit_table, follow_policy};
use crate::commands::validate_work_item_fields;

/// Envelope schema version for the stable `--json` dashboard document.
pub(crate) const DASHBOARD_VERSION: u64 = 1;

/// Maximum in-flight section requests.
const DEFAULT_CONCURRENCY: usize = 4;

/// Human/quiet headers for every Work Item section.
const WORK_HEADERS: &[&str] = &[
    "KEY", "PROJECT", "TITLE", "STATUS", "PRIORITY", "DUE", "ASSIGNEE",
];

/// Which read produced a section.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SectionKind {
    /// Work assigned to the authenticated user across the Project set.
    Mine,
    /// One Project's Work Item list.
    Project(String),
}

impl SectionKind {
    /// Stable identifier used in JSON and diagnostics.
    fn id(&self) -> String {
        match self {
            Self::Mine => "mine".to_string(),
            Self::Project(project) => format!("project:{project}"),
        }
    }

    /// Human label for the section heading.
    fn label(&self) -> String {
        match self {
            Self::Mine => "My Work".to_string(),
            Self::Project(project) => format!("Project {project}"),
        }
    }

    /// The Project key for a Project section, or `None` for My Work.
    fn project(&self) -> Option<&str> {
        match self {
            Self::Mine => None,
            Self::Project(project) => Some(project.as_str()),
        }
    }
}

/// One section's outcome.
struct Section {
    kind: SectionKind,
    outcome: Result<Value, CliError>,
}

/// One pending section fetch.
struct FetchJob {
    kind: SectionKind,
    query: ListWorkItemsQuery,
}

/// Runs `work dashboard`.
pub(crate) async fn dashboard(
    session: &mut Session<'_>,
    args: &WorkDashboardArgs,
) -> Result<(), CliError> {
    validate_work_item_fields(&session.global.fields)?;
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let projects = resolve_projects(&selection, args)?;
    let api = session.api(&selection)?;
    let include_mine = args.mine.unwrap_or(true);

    let mut jobs: Vec<FetchJob> = Vec::new();
    if include_mine {
        jobs.push(FetchJob {
            kind: SectionKind::Mine,
            query: ListWorkItemsQuery {
                limit: args.pagination.page_size(),
                cursor: args.pagination.cursor.clone(),
                scope: Some(args.scope.as_str().to_string()),
                projects: projects.clone(),
                fields: session.global.fields.clone(),
                ..Default::default()
            },
        });
    }
    for project in &projects {
        jobs.push(FetchJob {
            kind: SectionKind::Project(project.clone()),
            query: ListWorkItemsQuery {
                limit: args.pagination.page_size(),
                cursor: args.pagination.cursor.clone(),
                scope: Some(args.scope.as_str().to_string()),
                fields: session.global.fields.clone(),
                ..Default::default()
            },
        });
    }

    let sections = fetch_sections(api, org.clone(), jobs, args.pagination.clone()).await;

    if session.json() {
        let envelope = envelope(&org, &projects, include_mine, &sections);
        return emit_json(session, &envelope);
    }
    render_human(session, &org, &sections)
}

/// Resolves the Project set: explicit flags, then `.hamstik.toml`, then the
/// resolved single Project.
fn resolve_projects(
    selection: &Selection,
    args: &WorkDashboardArgs,
) -> Result<Vec<String>, CliError> {
    let mut projects = dedup_projects(args.project.iter().map(String::as_str));
    if !projects.is_empty() {
        return Ok(projects);
    }
    if let Some(path) = selection.context_path.as_deref() {
        let context = crate::context::load(path)?;
        if let Some(configured) = context.dashboard_projects.as_deref() {
            projects = dedup_projects(configured.iter().map(String::as_str));
            if !projects.is_empty() {
                return Ok(projects);
            }
        }
    }
    if let Some(project) = selection.project.value.clone() {
        return Ok(vec![project]);
    }
    Err(CliError::usage(
        "no projects configured for the dashboard; pass --project <KEY> (repeatable) or set \
         dashboard_projects in .hamstik.toml",
    ))
}

/// Trims, drops blanks, and de-duplicates Project keys, preserving order.
fn dedup_projects<'a>(keys: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut projects = Vec::new();
    for key in keys {
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        if seen.insert(key.to_string()) {
            projects.push(key.to_string());
        }
    }
    projects
}

/// Fetches every section with bounded concurrency, preserving job order.
async fn fetch_sections(
    api: Arc<dyn HamstikApi>,
    org: String,
    jobs: Vec<FetchJob>,
    pagination: PaginationArgs,
) -> Vec<Section> {
    let semaphore = Arc::new(Semaphore::new(DEFAULT_CONCURRENCY));
    let mut handles = Vec::with_capacity(jobs.len());
    for job in jobs {
        let kind = job.kind.clone();
        let api = api.clone();
        let org = org.clone();
        let sem = semaphore.clone();
        let pagination = pagination.clone();
        let handle = tokio::spawn(async move {
            // One permit covers the whole section (including `--all` paging),
            // bounding in-flight section requests to DEFAULT_CONCURRENCY.
            match sem.acquire().await {
                Ok(_permit) => match &job.kind {
                    SectionKind::Mine => fetch_mine(api, job.query, &pagination).await,
                    SectionKind::Project(project) => {
                        fetch_project(api, org, project.clone(), job.query, &pagination).await
                    }
                },
                Err(err) => Err(CliError::general(format!(
                    "concurrency control failed: {err}"
                ))),
            }
        });
        handles.push((kind, handle));
    }

    let mut sections = Vec::with_capacity(handles.len());
    for (kind, handle) in handles {
        let outcome = match handle.await {
            Ok(outcome) => outcome,
            Err(err) => Err(CliError::general(format!("task failed: {err}"))),
        };
        sections.push(Section { kind, outcome });
    }
    sections
}

/// Fetches the My Work section, honoring the shared pagination flags.
async fn fetch_mine(
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

/// Fetches one Project's Work Item list, honoring the shared pagination flags.
async fn fetch_project(
    api: Arc<dyn HamstikApi>,
    org: String,
    project: String,
    query: ListWorkItemsQuery,
    pagination: &PaginationArgs,
) -> Result<Value, CliError> {
    if pagination.all {
        let base = query;
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let api = api.clone();
            let org = org.clone();
            let project = project.clone();
            let mut query = base.clone();
            query.cursor = cursor;
            async move {
                let response = api.list_work_items(&org, &project, query).await?;
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
            .list_work_items(&org, &project, query)
            .await
            .map_err(CliError::from_client)?;
        Ok(response.raw)
    }
}

/// Builds the stable JSON document with every section present and failures
/// marked (never silently omitted).
fn envelope(org: &str, projects: &[String], include_mine: bool, sections: &[Section]) -> Value {
    let failed: Vec<String> = sections
        .iter()
        .filter(|section| section.outcome.is_err())
        .map(|section| section.kind.id())
        .collect();
    json!({
        "dashboardVersion": DASHBOARD_VERSION,
        "organization": org,
        "projects": projects,
        "mineIncluded": include_mine,
        "sections": sections.iter().map(section_json).collect::<Vec<_>>(),
        "failedSections": failed,
    })
}

/// Renders one section as a stable JSON object.
fn section_json(section: &Section) -> Value {
    let mut doc = json!({
        "id": section.kind.id(),
        "kind": if section.kind.project().is_some() { "project" } else { "mine" },
        "project": section.kind.project(),
    });
    match &section.outcome {
        Ok(value) => {
            doc["status"] = json!("ok");
            doc["items"] = value.get("items").cloned().unwrap_or_else(|| json!([]));
            doc["page"] = value.get("page").cloned().unwrap_or(Value::Null);
        }
        Err(error) => {
            doc["status"] = json!("error");
            doc["error"] = error.to_json()["error"].clone();
        }
    }
    doc
}

/// Renders the human (or quiet) view, reporting failures on stderr.
fn render_human(
    session: &mut Session<'_>,
    org: &str,
    sections: &[Section],
) -> Result<(), CliError> {
    let quiet = session.out.is_quiet();
    if !quiet {
        let project_count = sections
            .iter()
            .filter(|section| section.kind.project().is_some())
            .count();
        session
            .out
            .line(&format!(
                "Work dashboard {} {org} ({project_count} project(s))",
                session.glyphs().em_dash()
            ))
            .map_err(CliError::general)?;
    }

    for section in sections {
        render_section(session, section, quiet)?;
    }

    // A failed section is reported, but never aborts the others.
    for section in sections {
        if let Err(error) = &section.outcome {
            session.out.warn(&format!(
                "warning: {} section failed: {}",
                section.kind.id(),
                error.message
            ));
            if !quiet {
                session
                    .out
                    .line(&format!(
                        "  {}: failed ({}): {}",
                        section.kind.id(),
                        error.code,
                        error.message
                    ))
                    .map_err(CliError::general)?;
            }
        }
    }
    Ok(())
}

/// Renders one Work Item section as a table (or identifiers in quiet mode).
fn render_section(
    session: &mut Session<'_>,
    section: &Section,
    quiet: bool,
) -> Result<(), CliError> {
    let label = section.kind.label();
    match &section.outcome {
        Ok(value) => {
            if !quiet {
                session
                    .out
                    .line(&format!(
                        "\n{label} {} {} item(s)",
                        session.glyphs().em_dash(),
                        section_item_count(value)
                    ))
                    .map_err(CliError::general)?;
            }
            let rows = work_rows(value, section.kind.project());
            emit_table(session, value, WORK_HEADERS, &rows)
        }
        Err(_) => {
            if !quiet {
                session
                    .out
                    .line(&format!(
                        "\n{label} {} unavailable",
                        session.glyphs().em_dash()
                    ))
                    .map_err(CliError::general)?;
            }
            Ok(())
        }
    }
}

/// The number of items in a fetched section.
fn section_item_count(value: &Value) -> usize {
    value
        .get("items")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

/// Table rows for a section, always attributing each item to its Project.
fn work_rows(value: &Value, fallback_project: Option<&str>) -> Vec<Vec<String>> {
    value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    vec![
                        raw_string(item, "key"),
                        project_of(item, fallback_project),
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
                .collect()
        })
        .unwrap_or_default()
}

/// The item's Project key, falling back to the section's Project, or `-`.
fn project_of(item: &Value, fallback: Option<&str>) -> String {
    item.get("project")
        .and_then(|project| project.get("key"))
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .or(fallback)
        .unwrap_or("-")
        .to_string()
}

/// A raw string field, or `-` when absent/null/empty.
fn raw_string(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .unwrap_or("-")
        .to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn dedup_projects_trims_drops_blanks_and_preserves_order() {
        let projects = dedup_projects([" HAM ", "", "WEB", "HAM", "  "].into_iter());
        assert_eq!(projects, vec!["HAM".to_string(), "WEB".to_string()]);
    }

    #[test]
    fn project_of_prefers_item_project_then_section_fallback() {
        let item = json!({"project": {"key": "HAM"}});
        assert_eq!(project_of(&item, Some("WEB")), "HAM");
        let blank = json!({"project": {"key": ""}});
        assert_eq!(project_of(&blank, Some("WEB")), "WEB");
        let absent = json!({});
        assert_eq!(project_of(&absent, Some("WEB")), "WEB");
        assert_eq!(project_of(&absent, None), "-");
    }
}
