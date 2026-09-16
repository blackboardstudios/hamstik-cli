// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work label add|remove|clear`.

use serde_json::json;

use hamstik_api_client::{AttachLabelRequest, ListOptions, generate_key, validate_key};

use crate::app::Session;
use crate::args::{WorkLabelArgs, WorkLabelCommand};
use crate::error::CliError;

use super::common::is_uuid;
use super::dryrun;
use super::emit_view;
use super::view::render_work_item;
pub(super) async fn label(session: &mut Session<'_>, args: &WorkLabelArgs) -> Result<(), CliError> {
    let (command, label, key, force, idempotency_key) = match &args.command {
        WorkLabelCommand::Add {
            key,
            label,
            force,
            idempotency_key,
        } => ("add", label, key, *force, idempotency_key.as_deref()),
        WorkLabelCommand::Remove {
            key,
            label,
            force,
            idempotency_key,
        } => ("remove", label, key, *force, idempotency_key.as_deref()),
    };
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    // Attach accepts either selector directly in the live contract. Detach
    // remains a label-id path operation, so names are resolved only there.
    let attach_body = AttachLabelRequest {
        label_id: is_uuid(label).then(|| label.clone()),
        label: (!is_uuid(label)).then(|| label.trim().to_lowercase()),
    };
    let label_id = if command == "remove" && !is_uuid(label) {
        let name = label.trim().to_lowercase();
        let mut cursor: Option<String> = None;
        let mut resolved: Option<String> = None;
        loop {
            let response = api
                .list_labels(
                    &org,
                    &project,
                    ListOptions {
                        // The label collection caps `limit` at 100.
                        limit: Some(100),
                        cursor: cursor.clone(),
                    },
                )
                .await
                .map_err(CliError::from_client)?;
            if let Some(matched) = response
                .value
                .items
                .iter()
                .find(|label| label.name == name)
                .map(|l| l.id.clone())
            {
                resolved = Some(matched);
                break;
            }
            if !response.value.page.has_more {
                break;
            }
            cursor = response.value.page.next_cursor;
        }
        resolved.ok_or_else(|| {
            CliError::not_found(format!("no label named {label:?} in project {project}"))
        })?
    } else {
        label.clone()
    };

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

    let idempotency = match idempotency_key {
        Some(key) => {
            validate_key(key).map_err(|err| CliError::usage(err.to_string()))?;
            key.to_string()
        }
        None => generate_key(),
    };

    if session.global.dry_run {
        let (operation, method, body) = if command == "add" {
            (
                "work.label.add",
                "POST",
                Some(serde_json::to_value(&attach_body).map_err(CliError::general)?),
            )
        } else {
            ("work.label.remove", "DELETE", None)
        };
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation,
                method,
                path_template: if command == "add" {
                    "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/labels"
                } else {
                    "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/labels/{labelId}"
                },
                path: if command == "add" {
                    format!(
                        "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/labels"
                    )
                } else {
                    format!(
                        "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/labels/{label_id}"
                    )
                },
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "workItem": key,
                    "label": label,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body,
                notes: Vec::new(),
            },
        );
    }

    let response = match command {
        "add" => api
            .attach_label(&org, &project, key, &attach_body, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?,
        _ => api
            .detach_label(&org, &project, key, &label_id, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?,
    };
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let item = response.value.clone();
    emit_view(session, &response.raw, &item.key.clone(), |session| {
        render_work_item(session, &item)
    })
}
