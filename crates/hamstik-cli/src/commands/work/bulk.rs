// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work bulk create|update|transition`.

use serde_json::{Value, json};

use hamstik_api_client::{BulkCreateEnvelope, BulkTransitionEnvelope, BulkUpdateEnvelope};

use crate::app::Session;
use crate::args::{WorkBulkArgs, WorkBulkCommand};
use crate::error::CliError;

use super::archive::read_operations;
use super::bulk_csv;
use super::bulk_preflight;
use super::common::idem_key;
use super::dryrun;
use super::emit_json;
use super::emit_table;
pub(super) async fn bulk(session: &mut Session<'_>, args: &WorkBulkArgs) -> Result<(), CliError> {
    // `from-csv` is a local conversion: it needs no Organization, credential,
    // or network access, so handle it before resolving any API context.
    if let WorkBulkCommand::FromCsv {
        file,
        op,
        project,
        output,
    } = &args.command
    {
        return bulk_csv::from_csv(session, file, *op, project.as_deref(), output.as_deref());
    }

    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let idempotency = idem_key(args_idempotency(&args.command))?;

    // Bulk previews read and shape-validate the operations file exactly as a
    // real invocation would, then emit the request the CLI would send.
    if session.global.dry_run {
        let (operation, method, path, path_template, body): (&str, &str, String, &str, Value) =
            match &args.command {
                WorkBulkCommand::Create {
                    operations_file, ..
                } => {
                    let body = BulkCreateEnvelope {
                        operations: read_operations(
                            operations_file,
                            bulk_preflight::PreflightKind::Create,
                        )?,
                    };
                    (
                        "work.bulk.create",
                        "POST",
                        format!("/api/v1/organizations/{org}/bulk-work-items"),
                        "/api/v1/organizations/{organization}/bulk-work-items",
                        serde_json::to_value(&body).map_err(CliError::general)?,
                    )
                }
                WorkBulkCommand::Update {
                    operations_file,
                    concurrency,
                    ..
                } => {
                    let body = BulkUpdateEnvelope {
                        concurrency: concurrency
                            .unwrap_or(crate::args::ConcurrencyArg::RequireRevision)
                            .as_str()
                            .to_string(),
                        operations: read_operations(
                            operations_file,
                            bulk_preflight::PreflightKind::Update,
                        )?,
                    };
                    (
                        "work.bulk.update",
                        "PATCH",
                        format!("/api/v1/organizations/{org}/bulk-work-items"),
                        "/api/v1/organizations/{organization}/bulk-work-items",
                        serde_json::to_value(&body).map_err(CliError::general)?,
                    )
                }
                WorkBulkCommand::Transition {
                    operations_file,
                    concurrency,
                    ..
                } => {
                    let body = BulkTransitionEnvelope {
                        concurrency: concurrency.map(|c| c.as_str().to_string()),
                        operations: read_operations(
                            operations_file,
                            bulk_preflight::PreflightKind::Transition,
                        )?,
                    };
                    (
                        "work.bulk.transition",
                        "POST",
                        format!("/api/v1/organizations/{org}/bulk-work-item-transitions"),
                        "/api/v1/organizations/{organization}/bulk-work-item-transitions",
                        serde_json::to_value(&body).map_err(CliError::general)?,
                    )
                }
                WorkBulkCommand::FromCsv { .. } => {
                    unreachable!("from-csv is handled before any request")
                }
            };
        let count = body
            .get("operations")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation,
                method,
                path_template,
                path,
                resolved: json!({
                    "organization": org,
                    "operationCount": count,
                }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(body),
                notes: Vec::new(),
            },
        );
    }

    let response = match &args.command {
        WorkBulkCommand::Create {
            operations_file, ..
        } => {
            let body = BulkCreateEnvelope {
                operations: read_operations(
                    operations_file,
                    bulk_preflight::PreflightKind::Create,
                )?,
            };
            api.bulk_create_work_items(&org, &body, &idempotency)
                .await
                .map_err(CliError::from_client)?
        }
        WorkBulkCommand::Update {
            operations_file,
            concurrency,
            ..
        } => {
            let body = BulkUpdateEnvelope {
                concurrency: concurrency
                    .unwrap_or(crate::args::ConcurrencyArg::RequireRevision)
                    .as_str()
                    .to_string(),
                operations: read_operations(
                    operations_file,
                    bulk_preflight::PreflightKind::Update,
                )?,
            };
            api.bulk_update_work_items(&org, &body, &idempotency)
                .await
                .map_err(CliError::from_client)?
        }
        WorkBulkCommand::Transition {
            operations_file,
            concurrency,
            ..
        } => {
            let body = BulkTransitionEnvelope {
                concurrency: concurrency.map(|c| c.as_str().to_string()),
                operations: read_operations(
                    operations_file,
                    bulk_preflight::PreflightKind::Transition,
                )?,
            };
            api.bulk_transition_work_items(&org, &body, &idempotency)
                .await
                .map_err(CliError::from_client)?
        }
        WorkBulkCommand::FromCsv { .. } => {
            unreachable!("from-csv is handled before any request")
        }
    };
    let concurrency_mode = match &args.command {
        WorkBulkCommand::Create { .. } => None,
        WorkBulkCommand::Update { concurrency, .. }
        | WorkBulkCommand::Transition { concurrency, .. } => Some(
            concurrency
                .unwrap_or(crate::args::ConcurrencyArg::RequireRevision)
                .as_str(),
        ),
        WorkBulkCommand::FromCsv { .. } => {
            unreachable!("from-csv is handled before any request")
        }
    };
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let command_name = match &args.command {
        WorkBulkCommand::Create { .. } => "work.bulk.create",
        WorkBulkCommand::Update { .. } => "work.bulk.update",
        WorkBulkCommand::Transition { .. } => "work.bulk.transition",
        WorkBulkCommand::FromCsv { .. } => {
            unreachable!("from-csv is handled before any request")
        }
    };
    // A batch has no single target: record the batch size, which is all the
    // record can state without touching operation payloads.
    let target = match response.raw.get("results").and_then(Value::as_array) {
        Some(results) => format!("bulk:{} operations", results.len()),
        None => "bulk".to_string(),
    };
    crate::audit::record(
        &session.config,
        &mut session.out,
        command_name,
        &target,
        response.request_id.as_deref(),
    );
    render_bulk(session, &response.raw, concurrency_mode)
}

fn args_idempotency(command: &WorkBulkCommand) -> Option<String> {
    match command {
        WorkBulkCommand::Create {
            idempotency_key, ..
        }
        | WorkBulkCommand::Update {
            idempotency_key, ..
        }
        | WorkBulkCommand::Transition {
            idempotency_key, ..
        } => idempotency_key.clone(),
        WorkBulkCommand::FromCsv { .. } => {
            unreachable!("from-csv is handled before any request")
        }
    }
}
/// Renders bulk results: the raw `{results: [...]}` body in JSON mode, a
/// per-item table with a total/succeeded/failed summary and actionable
/// failure details in human mode, or a compact success/error listing in quiet.
/// Renders bulk results: the raw `{results: [...]}` body in JSON mode, a
/// per-item table with a total/succeeded/failed summary and actionable
/// failure details in human mode, or a compact success/error listing in quiet.
fn render_bulk(
    session: &mut Session<'_>,
    raw: &Value,
    concurrency_mode: Option<&str>,
) -> Result<(), CliError> {
    if session.json() {
        // JSON preserves every documented per-operation field exactly as the
        // server returned it (index, status, workItem, error).
        return emit_json(session, raw);
    }
    // State the selected concurrency mode explicitly so last-write-wins is
    // always a visible, deliberate choice in the transcript.
    if let Some(mode) = concurrency_mode {
        let label = bulk_preflight::concurrency_label(Some(mode));
        session
            .out
            .line(&format!(
                "concurrency: {} ({})",
                label["mode"].as_str().unwrap_or(mode),
                label["note"].as_str().unwrap_or_default()
            ))
            .map_err(CliError::general)?;
    }
    let results = raw
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let failed: Vec<&Value> = results
        .iter()
        .filter(|result| result.get("error").is_some_and(|e| !e.is_null()))
        .collect();
    let total = results.len();
    let succeeded = total - failed.len();
    if session.out.is_quiet() {
        for result in &results {
            let index = result.get("index").and_then(Value::as_i64).unwrap_or(0);
            let status = result.get("status").and_then(Value::as_i64).unwrap_or(0);
            if let Some(error) = result.get("error").filter(|e| !e.is_null()) {
                let code = error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("UNKNOWN");
                session
                    .out
                    .line(&format!("{index}\t{status}\t{code}"))
                    .map_err(CliError::general)?;
            } else {
                let key = result
                    .get("workItem")
                    .and_then(|w| w.get("key"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                session
                    .out
                    .line(&format!("{index}\t{status}\t{key}"))
                    .map_err(CliError::general)?;
            }
        }
        return Ok(());
    }
    let rows: Vec<Vec<String>> = results
        .iter()
        .map(|result| {
            let index = result.get("index").and_then(Value::as_i64).unwrap_or(0);
            let status = result.get("status").and_then(Value::as_i64).unwrap_or(0);
            let outcome = if let Some(error) = result.get("error").filter(|e| !e.is_null()) {
                let code = error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("UNKNOWN");
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                format!("{code}: {message}")
            } else {
                result
                    .get("workItem")
                    .and_then(|w| w.get("key"))
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string()
            };
            vec![index.to_string(), status.to_string(), outcome]
        })
        .collect();
    // A leading summary line states the mixed-outcome counts up front.
    let summary = if failed.is_empty() {
        format!("bulk results: {total}/{total} succeeded")
    } else {
        format!(
            "bulk results: {succeeded}/{total} succeeded, {} failed",
            failed.len()
        )
    };
    session.out.line(&summary).map_err(CliError::general)?;
    if !failed.is_empty() {
        session
            .out
            .line("failed operations (actionable details):")
            .map_err(CliError::general)?;
        for result in &failed {
            let index = result.get("index").and_then(Value::as_i64).unwrap_or(0);
            let status = result.get("status").and_then(Value::as_i64).unwrap_or(0);
            let error = result.get("error").unwrap_or(&Value::Null);
            let code = error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("UNKNOWN");
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the server did not provide a message");
            let detail = match (code, status) {
                ("REVISION_CONFLICT", _) => Some(
                    "re-read the Work Item (work view) and either set the new revision on the \
                     operation or re-run with --concurrency last-write-wins to accept any prior state"
                        .to_string(),
                ),
                ("INVALID_TRANSITION", _) => Some(format!(
                    "list the allowed transitions with: hamstik work transitions (operation index {index})"
                )),
                ("NOT_FOUND", 404) => Some(format!(
                    "the Work Item targeted by operations[{index}] does not exist in this project; \
                     verify the key"
                )),
                ("ARCHIVED", _) | ("CONFLICT", _) if status == 409 || status == 404 => Some(
                    "the target Work Item may be archived; list archived items with \
                     `work list --archived true` and unarchive it first"
                        .to_string(),
                ),
                _ => None,
            };
            session
                .out
                .line(&format!(
                    "  - operations[{index}] (HTTP {status}) {code}: {message}"
                ))
                .map_err(CliError::general)?;
            if let Some(detail) = detail {
                session
                    .out
                    .line(&format!("    fix: {detail}"))
                    .map_err(CliError::general)?;
            }
            if let Some(request_id) = error.get("requestId").and_then(Value::as_str) {
                session
                    .out
                    .line(&format!("    request id: {request_id}"))
                    .map_err(CliError::general)?;
            }
        }
    }
    emit_table(session, raw, &["INDEX", "STATUS", "OUTCOME"], &rows)
}
