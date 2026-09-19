// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work watcher watch|unwatch|list`.

use serde_json::json;

use hamstik_api_client::{WatcherAction, WorkItemWatcher};

use crate::app::Session;
use crate::args::{WorkWatcherArgs, WorkWatcherCommand};
use crate::error::CliError;

use super::common::idem_key;
use super::dryrun;
use super::emit_view;
/// Runs `hamstik work watcher` subcommands (dry-run included).
///
/// The watcher surface carries no revision guard (no `If-Match`): the server
/// resolves the authenticated user's own state, and actions are idempotent.
pub(super) async fn watcher(
    session: &mut Session<'_>,
    args: &WorkWatcherArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let (key, action, idempotency_key) = match &args.command {
        WorkWatcherCommand::Show { key } => (key, None, None),
        WorkWatcherCommand::Watch {
            key,
            idempotency_key,
        } => (key, Some(WatcherAction::Watch), idempotency_key.as_deref()),
        WorkWatcherCommand::Unwatch {
            key,
            idempotency_key,
        } => (
            key,
            Some(WatcherAction::Unwatch),
            idempotency_key.as_deref(),
        ),
        WorkWatcherCommand::Mute {
            key,
            idempotency_key,
        } => (key, Some(WatcherAction::Mute), idempotency_key.as_deref()),
        WorkWatcherCommand::Unmute {
            key,
            idempotency_key,
        } => (key, Some(WatcherAction::Unmute), idempotency_key.as_deref()),
    };
    let idempotency = idem_key(idempotency_key.map(str::to_string))?;

    if let Some(action) = action
        && session.global.dry_run
    {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "work.watcher.action",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/watcher",
                path: format!(
                    "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/watcher"
                ),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "workItem": key,
                    "action": action.as_str(),
                }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(json!({ "action": action.as_str() })),
                notes: Vec::new(),
            },
        );
    }

    let api = session.api(&selection)?;
    let response = match action {
        None => api
            .get_work_item_watcher(&org, &project, key)
            .await
            .map_err(CliError::from_client)?,
        Some(action) => {
            let response = api
                .update_work_item_watcher(&org, &project, key, action, &idempotency)
                .await
                .map_err(CliError::from_client)?;
            if response.idempotency_replayed {
                session
                    .out
                    .warn("note: request replayed (idempotent duplicate)");
            }
            crate::audit::record(
                &session.config,
                &mut session.out,
                &format!("work.watcher.{}", action.as_str()),
                key,
                response.request_id.as_deref(),
            );
            response
        }
    };
    let watcher = response.value.clone();
    emit_view(session, &response.raw, key, |session| {
        render_watcher(session, &watcher)
    })
}
/// Renders the authenticated user's watcher state.
/// Renders the authenticated user's watcher state.
fn render_watcher(session: &mut Session<'_>, watcher: &WorkItemWatcher) -> Result<(), CliError> {
    let state = if watcher.watched {
        "watching"
    } else {
        "not watching"
    };
    session
        .out
        .line(&format!("Watcher state: {state}"))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("  manual watch:    {}", watcher.manual_watch))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("  from assignment: {}", watcher.assignee_origin))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("  muted:           {}", watcher.muted))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("  assigned:        {}", watcher.assigned))
        .map_err(CliError::general)?;
    Ok(())
}
