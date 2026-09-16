// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work transitions` / `work transition` / `work start` / `work close`.

use serde_json::json;

use hamstik_api_client::{TransitionRequest, generate_key};

use crate::app::Session;
use crate::error::CliError;

use super::dryrun;
use super::emit_json;
use super::emit_view;
use super::view::render_work_item;
pub(super) async fn transitions(session: &mut Session<'_>, key: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .list_transitions(&org, &project, key)
        .await
        .map_err(CliError::from_client)?;
    let list = response.value.clone();
    emit_view(session, &response.raw, key, |session| {
        session
            .out
            .line(&format!("current status: {}", list.current_status))
            .map_err(CliError::general)?;
        if list.transitions.is_empty() {
            session
                .out
                .line("(no transitions available)")
                .map_err(CliError::general)?;
        } else {
            session.out.line("available:").map_err(CliError::general)?;
            for transition in &list.transitions {
                session
                    .out
                    .line(&format!("  {}", transition.target_status))
                    .map_err(CliError::general)?;
            }
        }
        Ok(())
    })
}

pub(super) async fn transition_to(
    session: &mut Session<'_>,
    key: &str,
    target: &str,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let current = api
        .get_work_item(&org, &project, key)
        .await
        .map_err(CliError::from_client)?;
    let current_status = current.value.status.clone();
    let etag = current.etag.clone();

    if current_status == target {
        session.out.warn(&format!("{key} is already {target}"));
        if session.json() {
            return emit_json(session, &current.raw);
        }
        let item = current.value.clone();
        return emit_view(session, &current.raw, key, |session| {
            render_work_item(session, &item)
        });
    }

    let allowed = api
        .list_transitions(&org, &project, key)
        .await
        .map_err(CliError::from_client)?;
    let permitted = allowed
        .value
        .transitions
        .iter()
        .any(|t| t.target_status == target);
    if !permitted {
        let options: Vec<&str> = allowed
            .value
            .transitions
            .iter()
            .map(|t| t.target_status.as_str())
            .collect();
        return Err(CliError::usage(format!(
            "transition to {target} is not allowed from {current_status}; available: {}",
            if options.is_empty() {
                "none".to_string()
            } else {
                options.join(", ")
            }
        )));
    }

    let if_match =
        etag.ok_or_else(|| CliError::protocol("server did not return an ETag for the work item"))?;
    let idempotency = generate_key();

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "work.transition",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/transitions",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/transitions"
                ),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "workItem": key,
                    "currentStatus": current_status,
                    "targetStatus": target,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(json!({ "targetStatus": target })),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .transition_work_item(
            &org,
            &project,
            key,
            &TransitionRequest {
                target_status: target.to_string(),
            },
            &if_match,
            &idempotency,
        )
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let item = response.value.clone();
    emit_view(session, &response.raw, key, |session| {
        render_work_item(session, &item)
    })
}
