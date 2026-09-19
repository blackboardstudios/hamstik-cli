// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik dashboard`: list, view, and evaluate Advanced Dashboards.

use hamstik_api_client::{
    AdvancedDashboard, AdvancedDashboardFilters, AdvancedDashboardRunRequest, ListAdvancedOptions,
    PageItems, follow_with,
};
use serde_json::{Value, json};

use crate::app::Session;
use crate::args::{AdvancedDashboardArgs, AdvancedDashboardCommand, PaginationArgs};
use crate::error::CliError;
use crate::input::resolve_json;

use super::org::render_lines;
use super::{check_columns, emit_view, follow_policy, render_list};

/// Runs an Advanced Dashboard subcommand.
pub async fn run(session: &mut Session<'_>, args: &AdvancedDashboardArgs) -> Result<(), CliError> {
    match &args.command {
        AdvancedDashboardCommand::List {
            visibility,
            pagination,
        } => list(session, visibility, pagination).await,
        AdvancedDashboardCommand::View { id } => view(session, id).await,
        AdvancedDashboardCommand::Run { id, filters_file } => {
            evaluate(session, id, filters_file.as_deref()).await
        }
    }
}

async fn list(
    session: &mut Session<'_>,
    visibility: &str,
    pagination: &PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let base = ListAdvancedOptions {
        limit: pagination.directory_page_size(),
        cursor: pagination.cursor.clone(),
        visibility: Some(visibility.to_string()),
    };
    let json_value = if pagination.all {
        let fetch_api = api.clone();
        let fetch_org = org.clone();
        let fetch_visibility = visibility.to_string();
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let api = fetch_api.clone();
            let org = fetch_org.clone();
            let visibility = fetch_visibility.clone();
            async move {
                let response = api
                    .list_advanced_dashboards(
                        &org,
                        ListAdvancedOptions {
                            limit: Some(PaginationArgs::DIRECTORY_PAGE_SIZE),
                            cursor,
                            visibility: Some(visibility),
                        },
                    )
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
        api.list_advanced_dashboards(&org, base)
            .await
            .map_err(CliError::from_client)?
            .raw
    };
    let rows = dashboard_rows(&json_value);
    let headers = [
        "ID",
        "NAME",
        "VISIBILITY",
        "REVISION",
        "WIDGETS",
        "SOURCE",
        "UPDATED",
    ];
    check_columns(&headers, &session.output_options())?;
    render_list(session, &json_value, &headers, &rows)
}

fn dashboard_rows(value: &Value) -> Vec<Vec<String>> {
    value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    vec![
                        text(item, "id"),
                        text(item, "name"),
                        text(item, "visibility"),
                        number(item, "revision"),
                        item.get("widgets")
                            .and_then(Value::as_array)
                            .map_or_else(String::new, |widgets| widgets.len().to_string()),
                        text(item, "sourceAvailability"),
                        text(item, "updatedAt"),
                    ]
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn view(session: &mut Session<'_>, id: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let response = session
        .api(&selection)?
        .get_advanced_dashboard(&org, id)
        .await
        .map_err(CliError::from_client)?;
    let dashboard = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_dashboard(session, &dashboard)
    })
}

async fn evaluate(
    session: &mut Session<'_>,
    id: &str,
    filters_file: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let filters: Option<AdvancedDashboardFilters> = filters_file
        .map(|file| resolve_json(file, &mut std::io::stdin()))
        .transpose()
        .map_err(CliError::usage)?;
    let api = session.api(&selection)?;
    let current = api
        .get_advanced_dashboard(&org, id)
        .await
        .map_err(CliError::from_client)?;
    let response = api
        .run_advanced_dashboard(
            &org,
            id,
            &AdvancedDashboardRunRequest {
                expected_revision: current.value.revision,
                filters,
            },
        )
        .await
        .map_err(CliError::from_client)?;
    let result = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_lines(
            session,
            &[
                ("dashboard", result.dashboard_id.clone()),
                ("revision", result.dashboard_revision.to_string()),
                ("evaluated", result.evaluated_at.clone()),
                ("source availability", result.source_availability.clone()),
                ("widgets", result.widgets.len().to_string()),
            ],
        )
    })
}

fn render_dashboard(
    session: &mut Session<'_>,
    dashboard: &AdvancedDashboard,
) -> Result<(), CliError> {
    render_lines(
        session,
        &[
            ("id", dashboard.id.clone()),
            ("name", dashboard.name.clone()),
            ("description", dashboard.description.clone()),
            ("visibility", dashboard.visibility.clone()),
            ("owner", dashboard.owner.public_id.clone()),
            ("revision", dashboard.revision.to_string()),
            ("source availability", dashboard.source_availability.clone()),
            ("widgets", dashboard.widgets.len().to_string()),
            ("updated", dashboard.updated_at.clone()),
            (
                "permissions",
                format!(
                    "edit={}, delete={}, duplicate={}",
                    dashboard.permissions.edit,
                    dashboard.permissions.delete,
                    dashboard.permissions.duplicate
                ),
            ),
        ],
    )
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number(value: &Value, key: &str) -> String {
    value
        .get(key)
        .filter(|value| value.is_number())
        .map(Value::to_string)
        .unwrap_or_default()
}
