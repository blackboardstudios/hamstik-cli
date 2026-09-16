// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work link add|delete|list`.

use serde_json::json;

use hamstik_api_client::{CreateWorkItemLinkRequest, ListOptions, PageItems, follow_all};

use crate::app::Session;
use crate::args::{WorkLinkArgs, WorkLinkCommand};
use crate::error::CliError;

use super::common::idem_key;
use super::dryrun;
use super::emit_json;
use super::emit_table;
use super::emit_view;
use super::render_lines;
pub(super) async fn link(session: &mut Session<'_>, args: &WorkLinkArgs) -> Result<(), CliError> {
    match &args.command {
        WorkLinkCommand::List { key, pagination } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let api = session.api(&selection)?;
            let response = list_links(api, &org, &project, key, pagination).await?;
            let rows: Vec<Vec<String>> = response
                .value
                .items
                .iter()
                .map(|l| {
                    vec![
                        l.id.clone(),
                        l.relation.clone(),
                        l.other_work_item.key.clone(),
                        l.other_work_item.title.clone(),
                        l.created_by
                            .as_ref()
                            .map(|u| u.name().to_string())
                            .unwrap_or_else(|| "-".to_string()),
                    ]
                })
                .collect();
            emit_table(
                session,
                &response.raw,
                &["ID", "RELATION", "OTHER", "TITLE", "BY"],
                &rows,
            )
        }
        WorkLinkCommand::Add {
            key,
            target_key,
            target_id,
            relation,
            idempotency_key,
        } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let api = session.api(&selection)?;
            let idempotency = idem_key(idempotency_key.clone())?;

            if session.global.dry_run {
                return dryrun::emit_preview(
                    session,
                    dryrun::PreviewRequest {
                        operation: "work.link.add",
                        method: "POST",
                        path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/links",
                        path: format!(
                            "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/links"
                        ),
                        resolved: json!({
                            "organization": org,
                            "project": project,
                            "workItem": key,
                            "targetKey": target_key,
                            "targetId": target_id,
                            "relation": relation.as_str(),
                        }),
                        if_match: None,
                        idempotency_key: Some(&idempotency),
                        body: Some(json!({
                            "targetId": target_id,
                            "targetKey": target_key,
                            "relation": relation.as_str(),
                        })),
                        notes: Vec::new(),
                    },
                );
            }

            let response = api
                .create_work_item_link(
                    &org,
                    &project,
                    key,
                    &CreateWorkItemLinkRequest {
                        target_id: target_id.clone(),
                        target_key: target_key.clone(),
                        relation: relation.as_str().to_string(),
                    },
                    &idempotency,
                )
                .await
                .map_err(CliError::from_client)?;
            if response.idempotency_replayed {
                session
                    .out
                    .warn("note: request replayed (idempotent duplicate)");
            }
            let link = response.value.clone();
            emit_view(session, &response.raw, &link.id.clone(), |session| {
                render_link(session, &link)
            })
        }
        WorkLinkCommand::Delete {
            key,
            link_id,
            idempotency_key,
        } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let idempotency = idem_key(idempotency_key.clone())?;

            if session.global.dry_run {
                return dryrun::emit_preview(
                    session,
                    dryrun::PreviewRequest {
                        operation: "work.link.delete",
                        method: "DELETE",
                        path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/links/{linkId}",
                        path: format!(
                            "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/links/{link_id}"
                        ),
                        resolved: json!({
                            "organization": org,
                            "project": project,
                            "workItem": key,
                            "linkId": link_id,
                        }),
                        if_match: None,
                        idempotency_key: Some(&idempotency),
                        body: None,
                        notes: Vec::new(),
                    },
                );
            }

            let api = session.api(&selection)?;
            let response = api
                .delete_work_item_link(&org, &project, key, link_id, &idempotency)
                .await
                .map_err(CliError::from_client)?;
            if response.idempotency_replayed {
                session
                    .out
                    .warn("note: request replayed (idempotent duplicate)");
            }
            if session.json() {
                emit_json(session, &json!({ "deleted": true, "linkId": link_id }))
            } else {
                session
                    .out
                    .line(&format!("Deleted link {link_id}"))
                    .map_err(CliError::general)
            }
        }
    }
}
/// Runs `hamstik work watcher` subcommands (dry-run included).
///
/// The watcher surface carries no revision guard (no `If-Match`): the server
/// resolves the authenticated user's own state, and actions are idempotent.
async fn list_links(
    api: std::sync::Arc<dyn hamstik_api_client::HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
    pagination: &crate::args::PaginationArgs,
) -> Result<hamstik_api_client::ApiResponse<hamstik_api_client::WorkItemLinkList>, CliError> {
    let opts = ListOptions {
        limit: pagination.limit,
        cursor: pagination.cursor.clone(),
    };
    if pagination.all {
        let fetch_api = api.clone();
        let org = org.to_string();
        let project = project.to_string();
        let key = key.to_string();
        let limit = pagination.limit;
        let page = follow_all(move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let project = project.clone();
            let key = key.to_string();
            async move {
                let response = fetch_api
                    .list_work_item_links(&org, &project, &key, ListOptions { limit, cursor })
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
        Ok(hamstik_api_client::ApiResponse {
            value: hamstik_api_client::WorkItemLinkList {
                items: page.items,
                page: page.page.clone(),
            },
            raw: json!({ "items": page.raw_items, "page": page.page }),
            request_id: None,
            etag: None,
            idempotency_replayed: false,
            location: None,
            rate_limit: None,
        })
    } else {
        api.list_work_item_links(org, project, key, opts)
            .await
            .map_err(CliError::from_client)
    }
}

fn render_link(
    session: &mut Session<'_>,
    link: &hamstik_api_client::WorkItemLink,
) -> Result<(), CliError> {
    let lines = [
        ("id", link.id.clone()),
        ("relation", link.relation.clone()),
        ("other", link.other_work_item.key.clone()),
        ("title", link.other_work_item.title.clone()),
        ("project", link.other_work_item.project.key.clone()),
        (
            "created by",
            link.created_by
                .as_ref()
                .map(|u| u.name().to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("created", link.created_at.clone()),
    ];
    render_lines(session, &lines)
}
