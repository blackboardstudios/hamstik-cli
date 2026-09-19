// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik report`: saved Advanced Reports and captured selection items.

use std::sync::{Arc, Mutex};

use hamstik_api_client::{
    AdvancedReport, AdvancedReportInput, AdvancedReportRunRequest, ListAdvancedOptions,
    ListOptions, PageItems, follow_with, generate_key, validate_key,
};
use serde_json::{Value, json};

use crate::app::Session;
use crate::args::{AdvancedReportArgs, AdvancedReportCommand, PaginationArgs};
use crate::error::CliError;
use crate::input::resolve_json;

use super::dryrun;
use super::org::render_lines;
use super::{check_columns, emit_json, emit_view, follow_policy, render_list};

/// Runs an Advanced Report subcommand.
pub async fn run(session: &mut Session<'_>, args: &AdvancedReportArgs) -> Result<(), CliError> {
    match &args.command {
        AdvancedReportCommand::List {
            visibility,
            pagination,
        } => list(session, visibility, pagination).await,
        AdvancedReportCommand::View { id } => view(session, id).await,
        AdvancedReportCommand::Create {
            file,
            idempotency_key,
        } => create(session, file, idempotency_key.as_deref()).await,
        AdvancedReportCommand::Edit {
            id,
            file,
            force,
            idempotency_key,
        } => edit(session, id, file, *force, idempotency_key.as_deref()).await,
        AdvancedReportCommand::Delete {
            id,
            force,
            idempotency_key,
        } => delete(session, id, *force, idempotency_key.as_deref()).await,
        AdvancedReportCommand::Run { id } => evaluate(session, id).await,
        AdvancedReportCommand::SelectionItems {
            run_id,
            cell_id,
            pagination,
        } => selection_items(session, run_id, cell_id, pagination).await,
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
                    .list_advanced_reports(
                        &org,
                        ListAdvancedOptions {
                            limit: PaginationArgs::DIRECTORY_PAGE_SIZE.into(),
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
        api.list_advanced_reports(&org, base)
            .await
            .map_err(CliError::from_client)?
            .raw
    };
    let rows = report_rows(&json_value);
    let headers = ["ID", "NAME", "VISIBILITY", "REVISION", "SOURCE", "UPDATED"];
    check_columns(&headers, &session.output_options())?;
    render_list(session, &json_value, &headers, &rows)
}

fn report_rows(value: &Value) -> Vec<Vec<String>> {
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
        .get_advanced_report(&org, id)
        .await
        .map_err(CliError::from_client)?;
    let report = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_report(session, &report)
    })
}

async fn create(
    session: &mut Session<'_>,
    file: &str,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let body: AdvancedReportInput =
        resolve_json(file, &mut std::io::stdin()).map_err(CliError::usage)?;
    let idempotency = idempotency(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "report.create",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/advanced-reports",
                path: format!("/api/v1/organizations/{org}/advanced-reports"),
                resolved: json!({ "organization": org }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = session
        .api(&selection)?
        .create_advanced_report(&org, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    replay_note(session, response.idempotency_replayed);
    let report = response.value.clone();
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "report.create",
        &report.id,
        None,
        Some(report.revision),
        response.request_id.as_deref(),
    );
    emit_view(session, &response.raw, &report.id.clone(), |session| {
        render_report(session, &report)
    })
}

async fn edit(
    session: &mut Session<'_>,
    id: &str,
    file: &str,
    force: bool,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let body: AdvancedReportInput =
        resolve_json(file, &mut std::io::stdin()).map_err(CliError::usage)?;
    let api = session.api(&selection)?;
    let (if_match, revision_before) = if force {
        ("*".to_string(), None)
    } else {
        let current = api
            .get_advanced_report(&org, id)
            .await
            .map_err(CliError::from_client)?;
        let etag = current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?;
        (etag, Some(current.value.revision))
    };
    let idempotency = idempotency(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "report.edit",
                method: "PATCH",
                path_template: "/api/v1/organizations/{organization}/advanced-reports/{id}",
                path: format!("/api/v1/organizations/{org}/advanced-reports/{id}"),
                resolved: json!({ "organization": org, "report": id }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .update_advanced_report(&org, id, &body, &if_match, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    replay_note(session, response.idempotency_replayed);
    let report = response.value.clone();
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "report.edit",
        id,
        revision_before,
        Some(report.revision),
        response.request_id.as_deref(),
    );
    emit_view(session, &response.raw, id, |session| {
        render_report(session, &report)
    })
}

async fn delete(
    session: &mut Session<'_>,
    id: &str,
    force: bool,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let (if_match, revision_before) = if force {
        ("*".to_string(), None)
    } else {
        let current = api
            .get_advanced_report(&org, id)
            .await
            .map_err(CliError::from_client)?;
        let etag = current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?;
        (etag, Some(current.value.revision))
    };
    let idempotency = idempotency(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "report.delete",
                method: "DELETE",
                path_template: "/api/v1/organizations/{organization}/advanced-reports/{id}",
                path: format!("/api/v1/organizations/{org}/advanced-reports/{id}"),
                resolved: json!({ "organization": org, "report": id }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(json!({})),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .delete_advanced_report(&org, id, &if_match, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    replay_note(session, response.idempotency_replayed);
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "report.delete",
        id,
        revision_before,
        None,
        response.request_id.as_deref(),
    );
    if session.json() {
        emit_json(session, &json!({ "deleted": true, "id": id }))
    } else {
        session
            .out
            .line(&format!("Deleted Advanced Report {id}"))
            .map_err(CliError::general)
    }
}

async fn evaluate(session: &mut Session<'_>, id: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let current = api
        .get_advanced_report(&org, id)
        .await
        .map_err(CliError::from_client)?;
    let response = api
        .run_advanced_report(
            &org,
            id,
            &AdvancedReportRunRequest {
                expected_revision: current.value.revision,
            },
        )
        .await
        .map_err(CliError::from_client)?;
    let result = response.value.clone();
    emit_view(session, &response.raw, id, |session| {
        render_lines(
            session,
            &[
                ("report", id.to_string()),
                ("source availability", result.source_availability.clone()),
                (
                    "dataset",
                    if result.dataset.is_some() {
                        "available".to_string()
                    } else {
                        "unavailable".to_string()
                    },
                ),
                ("message", result.message.clone().unwrap_or_default()),
            ],
        )
    })
}

async fn selection_items(
    session: &mut Session<'_>,
    run_id: &str,
    cell_id: &str,
    pagination: &PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let opts = ListOptions {
        limit: pagination.directory_page_size(),
        cursor: pagination.cursor.clone(),
    };
    let json_value = if pagination.all {
        let metadata = Arc::new(Mutex::new(None::<Value>));
        let capture = metadata.clone();
        let fetch_api = api.clone();
        let fetch_org = org.clone();
        let fetch_run = run_id.to_string();
        let fetch_cell = cell_id.to_string();
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let api = fetch_api.clone();
            let org = fetch_org.clone();
            let run_id = fetch_run.clone();
            let cell_id = fetch_cell.clone();
            let capture = capture.clone();
            async move {
                let response = api
                    .list_advanced_selection_items(
                        &org,
                        &run_id,
                        &cell_id,
                        ListOptions {
                            limit: Some(PaginationArgs::DIRECTORY_PAGE_SIZE),
                            cursor,
                        },
                    )
                    .await?;
                let mut slot = capture
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if slot.is_none() {
                    *slot = Some(response.raw.clone());
                }
                drop(slot);
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        })
        .await
        .map_err(CliError::from_client)?;
        let mut value = metadata
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .unwrap_or_else(|| json!({}));
        value["items"] = Value::Array(page.raw_items);
        value["page"] = serde_json::to_value(page.page).map_err(CliError::general)?;
        value
    } else {
        api.list_advanced_selection_items(&org, run_id, cell_id, opts)
            .await
            .map_err(CliError::from_client)?
            .raw
    };
    let rows = selection_rows(&json_value);
    let headers = [
        "KEY",
        "PROJECT",
        "STATUS",
        "CONTRIBUTION",
        "ARCHIVED",
        "DELETED",
        "TITLE",
    ];
    check_columns(&headers, &session.output_options())?;
    render_list(session, &json_value, &headers, &rows)
}

fn selection_rows(value: &Value) -> Vec<Vec<String>> {
    value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    vec![
                        text(item, "key"),
                        item.pointer("/project/key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        text(item, "status"),
                        number(item, "capturedContribution"),
                        boolean(item, "archived"),
                        boolean(item, "deleted"),
                        text(item, "title"),
                    ]
                })
                .collect()
        })
        .unwrap_or_default()
}

fn render_report(session: &mut Session<'_>, report: &AdvancedReport) -> Result<(), CliError> {
    render_lines(
        session,
        &[
            ("id", report.id.clone()),
            ("name", report.name.clone()),
            ("description", report.description.clone()),
            ("visibility", report.visibility.clone()),
            ("owner", report.owner.public_id.clone()),
            ("revision", report.revision.to_string()),
            ("source availability", report.source_availability.clone()),
            ("updated", report.updated_at.clone()),
            (
                "permissions",
                format!(
                    "edit={}, delete={}, duplicate={}",
                    report.permissions.edit,
                    report.permissions.delete,
                    report.permissions.duplicate
                ),
            ),
            (
                "definition",
                if report.definition.is_some() {
                    "available".to_string()
                } else {
                    "withheld".to_string()
                },
            ),
        ],
    )
}

fn idempotency(explicit: Option<&str>) -> Result<String, CliError> {
    match explicit {
        Some(key) => {
            validate_key(key).map_err(|err| CliError::usage(err.to_string()))?;
            Ok(key.to_string())
        }
        None => Ok(generate_key()),
    }
}

fn replay_note(session: &mut Session<'_>, replayed: bool) {
    if replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
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

fn boolean(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_bool)
        .map(|value| value.to_string())
        .unwrap_or_default()
}
