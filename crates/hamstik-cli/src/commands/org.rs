// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik org` (list / view / members / work / use).

use serde_json::{Value, json};

use hamstik_api_client::{
    ListOptions, ListOrganizationUsersOptions, ListWorkItemsQuery, OrganizationListItem, PageItems,
    follow_with,
};

use crate::app::Session;
use crate::args::{OrgArgs, OrgCommand, OrgWorkListArgs, PaginationArgs};
use crate::error::CliError;

use super::ensure_profile_for_default;
use super::work::apply_filters as apply_work_filters;
use super::work::sort::apply_sort;
use super::{
    check_columns, emit_json, emit_view, follow_policy, render_list, validate_work_item_fields,
};

/// Runs the `org` subcommands.
pub async fn run(session: &mut Session<'_>, args: &OrgArgs) -> Result<(), CliError> {
    match &args.command {
        OrgCommand::List(pagination) => list(session, pagination).await,
        OrgCommand::View { slug } => view(session, slug).await,
        OrgCommand::Members {
            slug,
            search,
            pagination,
        } => members(session, slug, search.as_deref(), pagination).await,
        OrgCommand::Work(work_args) => work(session, work_args).await,
        OrgCommand::Use { slug } => use_org(session, slug).await,
    }
}

async fn list(session: &mut Session<'_>, pagination: &PaginationArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    let api = session.api(&selection)?;

    let (rows, json_value) = if pagination.all {
        let limit = pagination.directory_page_size();
        let fetch_api = api.clone();
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            async move {
                let response = fetch_api
                    .list_organizations(ListOptions { limit, cursor })
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
        let rows: Vec<Vec<String>> = page.items.iter().map(org_row).collect();
        let json_value = json!({ "items": page.raw_items, "page": page.page });
        (rows, json_value)
    } else {
        let response = api
            .list_organizations(ListOptions {
                limit: pagination.directory_page_size(),
                cursor: pagination.cursor.clone(),
            })
            .await
            .map_err(CliError::from_client)?;
        let rows: Vec<Vec<String>> = response.value.items.iter().map(org_row).collect();
        (rows, response.raw)
    };

    check_columns(
        &["SLUG", "NAME", "PLAN", "STATE"],
        &session.output_options(),
    )?;
    render_list(
        session,
        &json_value,
        &["SLUG", "NAME", "PLAN", "STATE"],
        &rows,
    )
}

fn org_row(item: &OrganizationListItem) -> Vec<String> {
    let state = if item.suspended {
        "suspended"
    } else {
        "active"
    };
    vec![
        item.slug.clone(),
        item.name.clone(),
        item.plan.clone().unwrap_or_default(),
        state.to_string(),
    ]
}

async fn view(session: &mut Session<'_>, slug: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    let api = session.api(&selection)?;
    let response = api
        .get_organization(slug)
        .await
        .map_err(CliError::from_client)?;
    let org = response.value.clone();
    emit_view(session, &response.raw, slug, |session| {
        let lines = [
            ("name", org.name.clone()),
            ("slug", org.slug.clone()),
            ("plan", org.plan.clone()),
            ("role", org.role.clone()),
            ("default", org.is_default.to_string()),
            ("suspended", org.suspended.to_string()),
        ];
        render_lines(session, &lines)
    })
}

async fn use_org(session: &mut Session<'_>, slug: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    // SPEC §35: validate the organization through the Public API before
    // persisting it, so a typo never lands in the config.
    let api = session.api(&selection)?;
    let response = api
        .get_organization(slug)
        .await
        .map_err(CliError::from_client)?;
    let org = response.value;

    let profile_name = ensure_profile_for_default(session, &selection).await?;

    let mut config = session.config.load()?;
    let profile = config
        .profiles
        .get_mut(&profile_name)
        .ok_or_else(|| CliError::config(format!("no such profile: {profile_name}")))?;
    profile.default_organization = Some(org.slug);
    session.config.save(&config)?;
    crate::audit::record(
        &session.config,
        &mut session.out,
        "org.use",
        &profile_name,
        None,
    );

    if session.json() {
        emit_json(
            session,
            &json!({ "profile": profile_name, "defaultOrganization": slug }),
        )
    } else {
        session
            .out
            .line(&format!(
                "Default organization for profile {profile_name}: {slug}"
            ))
            .map_err(CliError::general)
    }
}

async fn members(
    session: &mut Session<'_>,
    slug: &str,
    search: Option<&str>,
    pagination: &PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let api = session.api(&selection)?;
    let base = ListOrganizationUsersOptions {
        limit: pagination.directory_page_size(),
        cursor: pagination.cursor.clone(),
        q: search.map(str::to_string),
    };
    let json_value: Value = if pagination.all {
        let fetch_api = api.clone();
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            let mut opts = base.clone();
            opts.cursor = cursor;
            async move {
                let response = fetch_api.list_organization_users(slug, opts).await?;
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
            .list_organization_users(slug, base)
            .await
            .map_err(CliError::from_client)?;
        response.raw
    };
    let rows: Vec<Vec<String>> = json_value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    vec![
                        item.get("publicId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        item.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        item.get("username")
                            .and_then(Value::as_str)
                            .unwrap_or("-")
                            .to_string(),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    check_columns(
        &["PUBLIC ID", "NAME", "USERNAME"],
        &session.output_options(),
    )?;
    render_list(
        session,
        &json_value,
        &["PUBLIC ID", "NAME", "USERNAME"],
        &rows,
    )
}

async fn work(session: &mut Session<'_>, args: &OrgWorkListArgs) -> Result<(), CliError> {
    validate_work_item_fields(&args.filters.fields)?;
    let selection = session.selection()?;
    super::work::report_filter_dates(&mut session.out, &args.filters);
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let mut query = ListWorkItemsQuery {
        limit: args.pagination.directory_page_size(),
        cursor: args.pagination.cursor.clone(),
        ..Default::default()
    };
    apply_work_filters(&mut query, &args.filters);
    query.projects = args.project.clone();

    let mut json_value: Value = if args.pagination.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let base = query.clone();
        let page = follow_with(follow_policy(&args.pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let mut query = base.clone();
            query.cursor = cursor;
            async move {
                let response = fetch_api.list_organization_work_items(&org, query).await?;
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
            .list_organization_work_items(&org, query)
            .await
            .map_err(CliError::from_client)?;
        response.raw
    };
    // `--sort KEY:DIR` carries the direction the REST parameter cannot.
    if let Some(sort) = args.filters.sort {
        apply_sort(&mut json_value, sort);
    }
    let color = session.color_enabled();
    let truecolor = session.truecolor_enabled();
    let rows: Vec<Vec<String>> = json_value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let key = item.get("key").and_then(Value::as_str).unwrap_or_default();
                    let title = item
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let status = item
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let project_key = item
                        .get("project")
                        .and_then(|p| p.get("key"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let assignee = item
                        .get("assignee")
                        .filter(|a| !a.is_null())
                        .and_then(|a| a.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("-");
                    let project_color = item
                        .get("project")
                        .and_then(|p| p.get("color"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    vec![
                        key.to_string(),
                        project_key.to_string(),
                        title.to_string(),
                        status.to_string(),
                        assignee.to_string(),
                        crate::palette::color_cell(color, project_color, truecolor),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    check_columns(
        &["KEY", "PROJECT", "TITLE", "STATUS", "ASSIGNEE", "COLOR"],
        &session.output_options(),
    )?;
    render_list(
        session,
        &json_value,
        &["KEY", "PROJECT", "TITLE", "STATUS", "ASSIGNEE", "COLOR"],
        &rows,
    )
}

/// Renders a decoded context row (kept for potential reuse by `user work`).
#[allow(dead_code)]
pub(crate) fn context_row(
    item: &hamstik_api_client::WorkItemContextSummary,
    color: bool,
    truecolor: bool,
) -> Vec<String> {
    vec![
        item.summary.key.clone(),
        item.project.key.clone(),
        item.summary.title.clone().unwrap_or_default(),
        item.summary.status.clone().unwrap_or_default(),
        item.summary
            .assignee
            .clone()
            .map(|a| a.name().to_string())
            .unwrap_or_else(|| "-".to_string()),
        crate::palette::color_cell(color, &item.project.color, truecolor),
    ]
}

pub(crate) fn render_lines(
    session: &mut Session<'_>,
    lines: &[(&str, String)],
) -> Result<(), CliError> {
    let width = lines.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (key, value) in lines {
        session
            .out
            .line(&format!("{key:<width$}  {value}"))
            .map_err(CliError::general)?;
    }
    Ok(())
}
