// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work list` / `work mine` / `work search`.

use serde_json::{Value, json};

use hamstik_api_client::{ListWorkItemsQuery, PageItems, SqueakQlSearchRequest, follow_with};

use crate::app::Session;
use crate::args::{MyWorkArgs, WorkListArgs};
use crate::error::CliError;

use super::sort::apply_sort;
use super::{emit_table, follow_policy};
fn my_work_query(args: &MyWorkArgs) -> ListWorkItemsQuery {
    ListWorkItemsQuery {
        limit: args.pagination.page_size(),
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

pub(super) async fn mine(session: &mut Session<'_>, args: &MyWorkArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    let api = session.api(&selection)?;
    let base = my_work_query(args);
    let mut json_value = if args.pagination.all {
        let fetch_api = api.clone();
        let page = follow_with(follow_policy(&args.pagination), move |cursor| {
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
    if let Some(sort) = args.sort {
        apply_sort(&mut json_value, sort);
    }
    render_context_work_items(session, &json_value)
}

pub(super) async fn search(
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
        limit: pagination.page_size(),
        cursor: pagination.cursor.clone(),
    };
    let json_value = if pagination.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let page = follow_with(follow_policy(pagination), move |cursor| {
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
fn build_query(args: &WorkListArgs) -> ListWorkItemsQuery {
    let mut query = ListWorkItemsQuery {
        limit: args.pagination.page_size(),
        cursor: args.pagination.cursor.clone(),
        ..Default::default()
    };
    super::apply_filters(&mut query, &args.filters);
    query
}

pub(super) async fn list(session: &mut Session<'_>, args: &WorkListArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    let query = build_query(args);

    let mut json_value = if args.pagination.all {
        let org = org.clone();
        let project = project.clone();
        let base = query.clone();
        let fetch_api = api.clone();
        let page = follow_with(follow_policy(&args.pagination), move |cursor| {
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
        json!({ "items": page.raw_items, "page": page.page })
    } else {
        api.list_work_items(&org, &project, query)
            .await
            .map_err(CliError::from_client)?
            .raw
    };
    if let Some(sort) = args.filters.sort {
        apply_sort(&mut json_value, sort);
    }
    let rows = summary_rows(&json_value);
    emit_table(
        session,
        &json_value,
        &["KEY", "TITLE", "STATUS", "TYPE", "PRIORITY", "ASSIGNEE"],
        &rows,
    )
}

/// Table rows for a Work Item collection payload, in the order the payload is
/// emitted so a table never disagrees with `--json`.
fn summary_rows(value: &Value) -> Vec<Vec<String>> {
    let Some(items) = value.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| {
            vec![
                raw_string(item, "key"),
                raw_string(item, "title"),
                raw_string(item, "status"),
                raw_string(item, "type"),
                raw_string(item, "priority"),
                item.get("assignee")
                    .and_then(|assignee| assignee.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
            ]
        })
        .collect()
}
