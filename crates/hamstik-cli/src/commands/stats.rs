// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Client-side aggregate summary of Work Items.
//!
//! Both `hamstik project stats` and `hamstik sprint stats` page through the
//! `work list` endpoint (respecting the same filters) and compute counts
//! client-side purely from returned, authoritative fields (status/type/priority/
//! assignee/storyPoints).
//!
//! This is a **client-side computed summary** of already-fetched data, not a
//! new server-reported metric.

use serde_json::{Value, json};

use hamstik_api_client::{ListWorkItemsQuery, PageItems, follow_with};

use crate::app::Session;
use crate::args::{PaginationArgs, ProjectStatsArgs, SprintStatsArgs};
use crate::error::CliError;

use super::{emit_json, follow_policy};

/// Aggregate statistics computed from a collection of Work Items.
#[derive(Debug, Clone)]
pub(crate) struct Stats {
    /// Total number of items in scope.
    pub total: u64,
    /// Counts grouped by status.
    pub by_status: std::collections::BTreeMap<String, u64>,
    /// Counts grouped by type.
    pub by_type: std::collections::BTreeMap<String, u64>,
    /// Counts grouped by priority.
    pub by_priority: std::collections::BTreeMap<String, u64>,
    /// Counts grouped by assignee name.
    pub by_assignee: std::collections::BTreeMap<String, u64>,
    /// Sum of story points for items that have a non-null value.
    pub total_story_points: i64,
    /// Number of items with a non-null story point value.
    pub estimated_items: u64,
    /// Total items fetched from the server (for truncation reporting).
    pub items_fetched: u64,
}

impl Stats {
    /// Compute aggregate statistics from a JSON payload containing an `items` array.
    pub(crate) fn from_items(items: &[Value]) -> Self {
        let mut by_status = std::collections::BTreeMap::new();
        let mut by_type = std::collections::BTreeMap::new();
        let mut by_priority = std::collections::BTreeMap::new();
        let mut by_assignee = std::collections::BTreeMap::new();
        let mut total_story_points: i64 = 0;
        let mut estimated_items: u64 = 0;

        for item in items {
            // Status
            if let Some(status) = item.get("status").and_then(Value::as_str) {
                *by_status.entry(status.to_string()).or_insert(0) += 1;
            }

            // Type
            if let Some(item_type) = item.get("type").and_then(Value::as_str) {
                *by_type.entry(item_type.to_string()).or_insert(0) += 1;
            }

            // Priority
            if let Some(priority) = item.get("priority").and_then(Value::as_str) {
                *by_priority.entry(priority.to_string()).or_insert(0) += 1;
            }

            // Assignee
            if let Some(assignee) = item.get("assignee")
                && let Some(name) = assignee.get("name").and_then(Value::as_str)
            {
                *by_assignee.entry(name.to_string()).or_insert(0) += 1;
            }

            // Story points
            if let Some(sp) = item.get("storyPoints").and_then(Value::as_i64) {
                total_story_points += sp;
                estimated_items += 1;
            }
        }

        Stats {
            total: items.len() as u64,
            by_status,
            by_type,
            by_priority,
            by_assignee,
            total_story_points,
            estimated_items,
            items_fetched: items.len() as u64,
        }
    }

    /// Serialize this aggregate into a JSON value suitable for `--json` output.
    pub(crate) fn to_json(&self, title: &str, truncated: bool) -> Value {
        let mut doc = json!({
            "summary": {
                "title": title,
                "total": self.total,
                "byStatus": self.by_status,
                "byType": self.by_type,
                "byPriority": self.by_priority,
                "byAssignee": self.by_assignee,
                "totalStoryPoints": self.total_story_points,
                "estimatedItems": self.estimated_items,
            },
            "computed": "This is a client-side computed summary of already-fetched data, not a server-reported metric.",
            "itemsFetched": self.items_fetched,
        });
        // Always present so `--json` consumers see a stable schema.
        doc["summary"]["truncated"] = json!(truncated);
        doc
    }
}

/// Build a `ListWorkItemsQuery` from `ProjectStatsArgs`.
///
/// `page_size` is the per-request page size derived from `--limit` (clamped
/// to the endpoint maximum), mirroring `work list`; `--limit` additionally
/// acts as the total result cap through `follow_policy`.
fn build_query(args: &ProjectStatsArgs, page_size: Option<u32>) -> ListWorkItemsQuery {
    let mut query = ListWorkItemsQuery {
        limit: page_size,
        cursor: args.cursor.clone(),
        ..Default::default()
    };
    apply_filters_to_query(&mut query, args);
    query
}

/// Build a `ListWorkItemsQuery` from `SprintStatsArgs`.
fn build_sprint_query(args: &SprintStatsArgs, page_size: Option<u32>) -> ListWorkItemsQuery {
    let mut query = ListWorkItemsQuery {
        limit: page_size,
        cursor: args.cursor.clone(),
        ..Default::default()
    };
    query.sprint = Some(args.sprint.clone());
    query
}

/// Apply the shared Work Item filters to a query.
fn apply_filters_to_query(query: &mut ListWorkItemsQuery, args: &ProjectStatsArgs) {
    query.q = args.search.clone();
    query.status = args.status.iter().map(|s| s.as_str().to_string()).collect();
    query.scope = args.scope.map(|s| s.as_str().to_string());
    query.item_type = args
        .item_type
        .iter()
        .map(|t| t.as_str().to_string())
        .collect();
    query.priority = args
        .priority
        .iter()
        .map(|p| p.as_str().to_string())
        .collect();
    // Forward the assignee filter verbatim — the API accepts `me`, `none`, a
    // user id, or a public id such as `usr_...`. This mirrors `work list`; a
    // split via assignee_fields that drops the public id would silently ignore
    // `usr_...` filters.
    query.assignee = args.assignee.clone();
    query.sprint = args.sprint.clone();
    query.label = args.label.clone();
    query.label_name = args.label_name.clone();
    query.parent = args.parent.clone();
    query.top_level = args.top_level;
    query.updated_after = args.updated_after.map(|d| d.to_string());
    query.overdue = args.overdue;
    query.due_before = args.due_before.map(|d| d.to_string());
    query.due_after = args.due_after.map(|d| d.to_string());
    query.archived = args.archived;
}

/// Run `hamstik project stats <KEY>`.
pub(crate) async fn project_stats(
    session: &mut Session<'_>,
    args: &ProjectStatsArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = args.project.clone();
    let api = session.api(&selection)?;

    let pagination = PaginationArgs {
        all: args.all,
        limit: args.limit,
        cursor: args.cursor.clone(),
    };
    let query = build_query(args, pagination.page_size());

    let (all_items, page_has_more) = if args.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let base = query.clone();
        let page = follow_with(follow_policy(&pagination), move |cursor| {
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
        // `page.has_more` is the authoritative truncation signal: it stays
        // `true` even when `--limit` cut the traversal through a page, so
        // we can distinguish "more data exists" from "exhausted".
        (page.raw_items, page.page.has_more)
    } else {
        let response = api
            .list_work_items(&org, &project, query)
            .await
            .map_err(CliError::from_client)?;
        // The raw response contains the items array; extract it
        let items = response
            .raw
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // Without `--all` exactly one page is fetched; the server's `hasMore`
        // is the truncation signal so a partial aggregate is never presented
        // as complete.
        (items, response.value.page.has_more)
    };

    let stats = Stats::from_items(&all_items);
    // Truncated whenever the server reports more pages beyond what this run
    // aggregated — either a single page without `--all`, or a `--limit` cap
    // that cut a `--all` traversal short.
    let truncated = page_has_more;

    if session.json() {
        let doc = stats.to_json(&format!("Project: {project}"), truncated);
        emit_json(session, &doc)
    } else {
        render_stats_human(session, &stats, &project, truncated)
    }
}

/// Run `hamstik sprint stats <SPRINT_ID>`.
pub(crate) async fn sprint_stats(
    session: &mut Session<'_>,
    args: &SprintStatsArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = super::sprint::require_project(session, args.project.as_deref())?;
    let api = session.api(&selection)?;

    let pagination = PaginationArgs {
        all: args.all,
        limit: args.limit,
        cursor: args.cursor.clone(),
    };
    let query = build_sprint_query(args, pagination.page_size());

    let (all_items, page_has_more) = if args.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let base = query.clone();
        let page = follow_with(follow_policy(&pagination), move |cursor| {
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
        // `page.has_more` is the authoritative truncation signal: it stays
        // `true` even when `--limit` cut the traversal through a page, so
        // we can distinguish "more data exists" from "exhausted".
        (page.raw_items, page.page.has_more)
    } else {
        let response = api
            .list_work_items(&org, &project, query)
            .await
            .map_err(CliError::from_client)?;
        let items = response
            .raw
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // Without `--all` exactly one page is fetched; the server's `hasMore`
        // is the truncation signal so a partial aggregate is never presented
        // as complete.
        (items, response.value.page.has_more)
    };

    let stats = Stats::from_items(&all_items);
    // Truncated whenever the server reports more pages beyond what this run
    // aggregated — either a single page without `--all`, or a `--limit` cap
    // that cut a `--all` traversal short.
    let truncated = page_has_more;

    if session.json() {
        let doc = stats.to_json(&format!("Sprint: {}", args.sprint), truncated);
        emit_json(session, &doc)
    } else {
        render_stats_human(session, &stats, &args.sprint, truncated)
    }
}

/// Render stats in human-readable table format.
fn render_stats_human(
    session: &mut Session<'_>,
    stats: &Stats,
    scope: &str,
    truncated: bool,
) -> Result<(), CliError> {
    // Header
    session
        .out
        .line(&format!("Aggregate summary for: {scope}"))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("Total items: {}", stats.total))
        .map_err(CliError::general)?;

    if truncated {
        session.out.warn(
            "note: results truncated: the server reported more pages; rerun with --all (optionally --limit) to aggregate the complete collection",
        );
    }

    // By Status
    if !stats.by_status.is_empty() {
        session
            .out
            .line("\nBy status:")
            .map_err(CliError::general)?;
        let rows: Vec<Vec<String>> = stats
            .by_status
            .iter()
            .map(|(status, count)| vec![status.clone(), count.to_string()])
            .collect();
        session
            .out
            .table(&["STATUS", "COUNT"], &rows)
            .map_err(CliError::general)?;
    }

    // By Type
    if !stats.by_type.is_empty() {
        session.out.line("\nBy type:").map_err(CliError::general)?;
        let rows: Vec<Vec<String>> = stats
            .by_type
            .iter()
            .map(|(item_type, count)| vec![item_type.clone(), count.to_string()])
            .collect();
        session
            .out
            .table(&["TYPE", "COUNT"], &rows)
            .map_err(CliError::general)?;
    }

    // By Priority
    if !stats.by_priority.is_empty() {
        session
            .out
            .line("\nBy priority:")
            .map_err(CliError::general)?;
        let rows: Vec<Vec<String>> = stats
            .by_priority
            .iter()
            .map(|(priority, count)| vec![priority.clone(), count.to_string()])
            .collect();
        session
            .out
            .table(&["PRIORITY", "COUNT"], &rows)
            .map_err(CliError::general)?;
    }

    // By Assignee
    if !stats.by_assignee.is_empty() {
        session
            .out
            .line("\nBy assignee:")
            .map_err(CliError::general)?;
        let rows: Vec<Vec<String>> = stats
            .by_assignee
            .iter()
            .map(|(assignee, count)| vec![assignee.clone(), count.to_string()])
            .collect();
        session
            .out
            .table(&["ASSIGNEE", "COUNT"], &rows)
            .map_err(CliError::general)?;
    }

    // Story points
    if stats.estimated_items > 0 {
        session
            .out
            .line(&format!(
                "\nStory points: {} total across {} estimated item(s)",
                stats.total_story_points, stats.estimated_items
            ))
            .map_err(CliError::general)?;
    }

    // Disclaimer
    session
        .out
        .line(
            "\nNote: this is a client-side computed summary of already-fetched data, \
             not a server-reported metric.",
        )
        .map_err(CliError::general)?;

    Ok(())
}
