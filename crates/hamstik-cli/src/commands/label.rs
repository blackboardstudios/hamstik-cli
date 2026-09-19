// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik label` (list / create).

use serde_json::json;

use hamstik_api_client::{
    CreateLabelRequest, ListOptions, PageItems, ProjectLabel, follow_with, generate_key,
    validate_key,
};

use crate::app::Session;
use crate::args::{LabelArgs, LabelCommand};
use crate::error::CliError;

use super::dryrun;
use super::{emit_table, emit_view, follow_policy};

/// Runs the `label` subcommands.
pub async fn run(session: &mut Session<'_>, args: &LabelArgs) -> Result<(), CliError> {
    match &args.command {
        LabelCommand::List {
            project,
            pagination,
        } => list(session, project.as_deref(), pagination).await,
        LabelCommand::Create {
            name,
            color,
            project,
            idempotency_key,
        } => {
            create(
                session,
                name,
                color.as_deref(),
                project.as_deref(),
                idempotency_key.as_deref(),
            )
            .await
        }
    }
}

/// Resolves the effective project key: explicit flag wins over context.
fn require_project(session: &Session<'_>, explicit: Option<&str>) -> Result<String, CliError> {
    if let Some(project) = explicit {
        return Ok(project.to_string());
    }
    let selection = session.selection()?;
    session.require_project(&selection)
}

async fn list(
    session: &mut Session<'_>,
    project_flag: Option<&str>,
    pagination: &crate::args::PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;
    let api = session.api(&selection)?;

    if pagination.all {
        let limit = pagination.directory_page_size();
        let fetch_api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let page = follow_with(follow_policy(pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let project = project.clone();
            async move {
                let response = fetch_api
                    .list_labels(&org, &project, ListOptions { limit, cursor })
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
        let rows: Vec<Vec<String>> = page
            .items
            .iter()
            .map(|label| label_row(session, label))
            .collect();
        let json_value = serde_json::json!({ "items": page.raw_items, "page": page.page });
        emit_table(session, &json_value, &["ID", "NAME", "COLOR"], &rows)
    } else {
        let response = api
            .list_labels(
                &org,
                &project,
                ListOptions {
                    limit: pagination.directory_page_size(),
                    cursor: pagination.cursor.clone(),
                },
            )
            .await
            .map_err(CliError::from_client)?;
        let rows: Vec<Vec<String>> = response
            .value
            .items
            .iter()
            .map(|label| label_row(session, label))
            .collect();
        emit_table(session, &response.raw, &["ID", "NAME", "COLOR"], &rows)
    }
}

/// One label row: the color cell carries a swatch in the label's color.
fn label_row(session: &Session<'_>, label: &ProjectLabel) -> Vec<String> {
    vec![
        label.id.clone(),
        label.name.clone(),
        super::project::color_detail(session, &label.color),
    ]
}

async fn create(
    session: &mut Session<'_>,
    name: &Option<String>,
    color: Option<&str>,
    project_flag: Option<&str>,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = require_project(session, project_flag)?;

    let name = match name {
        Some(name) => name.clone(),
        None if session.can_prompt() => session
            .prompt
            .read_line("Label name: ")
            .map_err(|err| CliError::general(format!("prompt failed: {err}")))?,
        None => return Err(CliError::usage("missing required option --name")),
    };
    if name.trim().is_empty() {
        return Err(CliError::usage("name must not be empty"));
    }

    let body = CreateLabelRequest {
        name,
        color: color.map(str::to_string),
    };
    let idempotency = match idempotency_key {
        Some(key) => {
            validate_key(key).map_err(|err| CliError::usage(err.to_string()))?;
            key.to_string()
        }
        None => generate_key(),
    };

    if session.global.dry_run {
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "label.create",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/labels",
                path: format!("/api/v1/organizations/{org}/projects/{project}/labels"),
                resolved: json!({ "organization": org, "project": project }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let api = session.api(&selection)?;
    let response = api
        .create_label(&org, &project, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let label = response.value.clone();
    crate::audit::record(
        &session.config,
        &mut session.out,
        "label.create",
        &label.id.clone(),
        response.request_id.as_deref(),
    );
    emit_view(session, &response.raw, &label.id.clone(), |session| {
        session
            .out
            .line(&format!(
                "{}  {}  {}",
                label.id,
                label.name,
                super::project::color_detail(session, &label.color)
            ))
            .map_err(CliError::general)
    })
}
