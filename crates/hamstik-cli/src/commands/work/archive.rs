// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work archive` / `work unarchive` / `work delete`.

use serde::de::DeserializeOwned;
use serde_json::json;

use crate::app::Session;
use crate::error::CliError;

use super::bulk_preflight;
use super::common::idem_key_ref;
use super::dryrun;
use super::emit_json;
use super::emit_view;
use super::view::render_work_item;
/// Archives (or unarchives) a Work Item with the Work Item ETag.
pub(super) async fn change_archive(
    session: &mut Session<'_>,
    key: &str,
    archived: bool,
    force: bool,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let if_match = if force {
        "*".to_string()
    } else {
        let current = api
            .get_work_item(&org, &project, key)
            .await
            .map_err(CliError::from_client)?;
        current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?
    };
    let idempotency = idem_key_ref(idempotency_key)?;

    if session.global.dry_run {
        let (operation, suffix) = if archived {
            ("work.archive", "/archive")
        } else {
            ("work.unarchive", "/unarchive")
        };
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation,
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/archive",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/work-items/{key}{suffix}"
                ),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "workItem": key,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(json!({})),
                notes: Vec::new(),
            },
        );
    }

    let response = if archived {
        api.archive_work_item(&org, &project, key, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?
    } else {
        api.unarchive_work_item(&org, &project, key, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?
    };
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let item = response.value.clone();
    emit_view(session, &response.raw, &item.key.clone(), |session| {
        render_work_item(session, &item, false)
    })
}
/// Soft-deletes a Work Item (Organization owner only).
/// Soft-deletes a Work Item (Organization owner only).
pub(super) async fn delete(
    session: &mut Session<'_>,
    key: &str,
    cascade: bool,
    force: bool,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let if_match = if force {
        "*".to_string()
    } else {
        let current = api
            .get_work_item(&org, &project, key)
            .await
            .map_err(CliError::from_client)?;
        current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?
    };
    let idempotency = idem_key_ref(idempotency_key)?;

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "work.delete",
                method: "DELETE",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}",
                path: format!("/api/v1/organizations/{org}/projects/{project}/work-items/{key}"),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "workItem": key,
                    "cascade": cascade,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(json!({ "cascade": cascade })),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .delete_work_item(&org, &project, key, cascade, &if_match, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    if session.json() {
        emit_json(
            session,
            &json!({ "deleted": true, "key": key, "cascade": cascade }),
        )
    } else {
        session
            .out
            .line(&format!("Deleted work item {key}"))
            .map_err(CliError::general)
    }
}
/// Reads and preflights the bulk operations payload (path or `-` for stdin).
///
/// The JSON syntax, envelope shape, per-operation required fields, unknown
/// fields, enum spellings, and revision constraints are validated locally
/// against the checked-in Public API schema-derived rules before any HTTP
/// request. Malformed input fails with the failing operation index and field
/// path and never echoes unrelated payload content.
/// Reads and preflights the bulk operations payload (path or `-` for stdin).
///
/// The JSON syntax, envelope shape, per-operation required fields, unknown
/// fields, enum spellings, and revision constraints are validated locally
/// against the checked-in Public API schema-derived rules before any HTTP
/// request. Malformed input fails with the failing operation index and field
/// path and never echoes unrelated payload content.
pub(super) fn read_operations<T: DeserializeOwned>(
    path: &str,
    kind: bulk_preflight::PreflightKind,
) -> Result<Vec<T>, CliError> {
    const MAX_OPERATIONS_BYTES: usize = 1024 * 1024;
    let text = if path == "-" {
        let stdin = std::io::stdin();
        crate::input::read_capped(stdin.lock(), MAX_OPERATIONS_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read stdin: {err}")))?
    } else {
        let handle = std::fs::File::open(path)
            .map_err(|err| CliError::usage(format!("cannot read {path}: {err}")))?;
        crate::input::read_capped(handle, MAX_OPERATIONS_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read {path}: {err}")))?
    };
    let source = if path == "-" { "stdin" } else { path };
    let items: Vec<T> = serde_json::from_str(
        &serde_json::to_string(&bulk_preflight::preflight(&text, source, kind)?)
            .map_err(CliError::general)?,
    )
    .map_err(|err| {
        CliError::usage(format!(
            "operations must match the Public API JSON array schema: {err}"
        ))
    })?;
    Ok(items)
}
