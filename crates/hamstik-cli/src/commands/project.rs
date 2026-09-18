// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik project` (list / view / create / edit / archive / unarchive /
//! activity / use).

use serde_json::{Value, json};

use hamstik_api_client::{
    ActivityOptions, CreateProjectRequest, ListProjectsOptions, PageItems, Project,
    UpdateProjectRequest, follow_all, generate_key, validate_key,
};

use crate::app::Session;
use crate::args::{PaginationArgs, ProjectArgs, ProjectCommand};
use crate::error::CliError;
use crate::input::resolve_text;

use super::dryrun;
use super::org::render_lines;
use super::{emit_json, emit_table, emit_view, ensure_profile_for_default};

/// Runs the `project` subcommands.
pub async fn run(session: &mut Session<'_>, args: &ProjectArgs) -> Result<(), CliError> {
    match &args.command {
        ProjectCommand::List {
            archived,
            pagination,
        } => list(session, *archived, pagination).await,
        ProjectCommand::View { key } => view(session, key).await,
        ProjectCommand::Create(create_args) => create(session, create_args).await,
        ProjectCommand::Edit(edit_args) => edit(session, edit_args).await,
        ProjectCommand::Archive {
            key,
            force,
            idempotency_key,
        } => change_archive(session, key, true, *force, idempotency_key.as_deref()).await,
        ProjectCommand::Unarchive {
            key,
            force,
            idempotency_key,
        } => change_archive(session, key, false, *force, idempotency_key.as_deref()).await,
        ProjectCommand::Activity {
            project,
            since,
            pagination,
        } => activity(session, project.as_deref(), since.as_deref(), pagination).await,
        ProjectCommand::Use { key } => use_project(session, key).await,
    }
}

async fn list(
    session: &mut Session<'_>,
    archived: Option<bool>,
    pagination: &PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let base = ListProjectsOptions {
        limit: pagination.limit,
        cursor: pagination.cursor.clone(),
        archived,
    };
    let json_value: Value = if pagination.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let page = follow_all(move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let mut opts = base.clone();
            opts.cursor = cursor;
            async move {
                let response = fetch_api.list_projects(&org, opts).await?;
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
            .list_projects(&org, base)
            .await
            .map_err(CliError::from_client)?;
        response.raw
    };
    let color = session.color_enabled();
    let truecolor = session.truecolor_enabled();
    let rows: Vec<Vec<String>> = json_value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let state = if item.get("archivedAt").map(Value::is_null).unwrap_or(true) {
                        "active"
                    } else {
                        "archived"
                    };
                    vec![
                        item.get("key")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        item.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        crate::palette::color_cell(
                            color,
                            item.get("color")
                                .and_then(Value::as_str)
                                .unwrap_or_default(),
                            truecolor,
                        ),
                        state.to_string(),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    emit_table(
        session,
        &json_value,
        &["KEY", "NAME", "COLOR", "STATE"],
        &rows,
    )
}

/// Renders the color value for detail views: a swatch in the actual color
/// followed by the plain hex text (human mode only; view closures are not
/// called for `--json`/`--quiet`).
pub(crate) fn color_detail(session: &Session<'_>, color: &str) -> String {
    let enabled = session.color_enabled();
    let truecolor = session.truecolor_enabled();
    crate::palette::color_cell(enabled, color, truecolor)
}

fn render_project(session: &mut Session<'_>, project: &Project) -> Result<(), CliError> {
    let lines = [
        ("name", project.name.clone()),
        ("key", project.key.clone()),
        ("color", color_detail(session, &project.color)),
        (
            "description",
            project.description.clone().unwrap_or_default(),
        ),
        ("revision", project.revision.to_string()),
        (
            "archived",
            project
                .archived_at
                .clone()
                .unwrap_or_else(|| "no".to_string()),
        ),
    ];
    render_lines(session, &lines)
}

async fn view(session: &mut Session<'_>, key: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .get_project(&org, key)
        .await
        .map_err(CliError::from_client)?;
    let project: Project = response.value.clone();
    emit_view(session, &response.raw, key, |session| {
        render_project(session, &project)
    })
}

async fn create(
    session: &mut Session<'_>,
    args: &crate::args::ProjectCreateArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;

    let name = match &args.name {
        Some(name) => name.clone(),
        None if session.can_prompt() => session
            .prompt
            .read_line("Name: ")
            .map_err(|err| CliError::general(format!("prompt failed: {err}")))?,
        None => return Err(CliError::usage("missing required option --name")),
    };
    if name.trim().is_empty() {
        return Err(CliError::usage("name must not be empty"));
    }
    let description = if args.description_editor {
        Some(crate::editor::edit_text(
            session.env,
            session.global.no_input,
            "a project description",
        )?)
    } else {
        resolve_text(
            args.description.clone(),
            args.description_file.as_deref(),
            &mut std::io::stdin(),
        )
        .map_err(|err| CliError::general(format!("cannot read text: {err}")))?
    };
    if let Some(key) = &args.key
        && key.trim().is_empty()
    {
        return Err(CliError::usage("key must not be empty"));
    }

    let body = CreateProjectRequest {
        name,
        key: args.key.clone(),
        description,
        color: args.color.clone(),
    };
    let idempotency = match &args.idempotency_key {
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
                operation: "project.create",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects",
                path: format!("/api/v1/organizations/{org}/projects"),
                resolved: json!({ "organization": org }),
                if_match: None,
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let api = session.api(&selection)?;
    let response = api
        .create_project(&org, &body, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let project = response.value.clone();
    emit_view(session, &response.raw, &project.key.clone(), |session| {
        render_project(session, &project)
    })
}

/// Edits a Project with its Project ETag (Organization administrators).
async fn edit(
    session: &mut Session<'_>,
    args: &crate::args::ProjectEditArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;

    let description = if args.clear_description {
        Some(None)
    } else if args.description_editor {
        Some(Some(crate::editor::edit_text(
            session.env,
            session.global.no_input,
            "a project description edit",
        )?))
    } else {
        resolve_text(
            args.description.clone(),
            args.description_file.as_deref(),
            &mut std::io::stdin(),
        )
        .map_err(|err| CliError::general(format!("cannot read text: {err}")))?
        .map(Some)
    };

    let body = UpdateProjectRequest {
        name: args.name.clone(),
        description,
        color: args.color.clone(),
    };
    if body.is_empty() {
        return Err(CliError::usage("no changes specified"));
    }

    let if_match = if args.force {
        "*".to_string()
    } else {
        let current = api
            .get_project(&org, &args.key)
            .await
            .map_err(CliError::from_client)?;
        current.etag.ok_or_else(|| {
            CliError::protocol("server did not return an ETag; re-run with --force")
        })?
    };
    let idempotency = match &args.idempotency_key {
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
                operation: "project.edit",
                method: "PATCH",
                path_template: "/api/v1/organizations/{organization}/projects/{key}",
                path: format!("/api/v1/organizations/{org}/projects/{}", args.key),
                resolved: json!({
                    "organization": org,
                    "project": args.key,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(serde_json::to_value(&body).map_err(CliError::general)?),
                notes: Vec::new(),
            },
        );
    }

    let response = api
        .update_project(&org, &args.key, &body, &if_match, &idempotency)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let project = response.value.clone();
    emit_view(session, &response.raw, &project.key.clone(), |session| {
        render_project(session, &project)
    })
}

/// Archives or unarchives a Project with its Project ETag.
async fn change_archive(
    session: &mut Session<'_>,
    key: &str,
    archived: bool,
    force: bool,
    idempotency_key: Option<&str>,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;

    let if_match = if force {
        "*".to_string()
    } else {
        let current = api
            .get_project(&org, key)
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
        let (operation, suffix) = if archived {
            ("project.archive", "/archive")
        } else {
            ("project.unarchive", "/unarchive")
        };
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation,
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{key}/archive",
                path: format!("/api/v1/organizations/{org}/projects/{key}{suffix}"),
                resolved: json!({
                    "organization": org,
                    "project": key,
                }),
                if_match: Some(&if_match),
                idempotency_key: Some(&idempotency),
                body: Some(json!({})),
                notes: Vec::new(),
            },
        );
    }

    let response = if archived {
        api.archive_project(&org, key, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?
    } else {
        api.unarchive_project(&org, key, &if_match, &idempotency)
            .await
            .map_err(CliError::from_client)?
    };
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let project = response.value.clone();
    emit_view(session, &response.raw, &project.key.clone(), |session| {
        render_project(session, &project)
    })
}

/// Renders the Project activity feed (newest first).
async fn activity(
    session: &mut Session<'_>,
    project_flag: Option<&str>,
    since: Option<&str>,
    pagination: &PaginationArgs,
) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = match project_flag {
        Some(key) => key.to_string(),
        None => session.require_project(&selection)?,
    };
    let api = session.api(&selection)?;
    let opts = ActivityOptions {
        limit: pagination.limit,
        cursor: pagination.cursor.clone(),
        since: since.map(str::to_string),
    };
    let json_value: Value = if pagination.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let since = opts.since.clone();
        let page = follow_all(move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let project = project.clone();
            let since = since.clone();
            async move {
                let opts = ActivityOptions {
                    limit: None,
                    cursor,
                    since: since.clone(),
                };
                let response = fetch_api
                    .list_project_activity(&org, &project, opts)
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
        let response = api
            .list_project_activity(&org, &project, opts)
            .await
            .map_err(CliError::from_client)?;
        response.raw
    };
    let rows: Vec<Vec<String>> = json_value
        .get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(activity_row_from_raw).collect())
        .unwrap_or_default();
    emit_table(
        session,
        &json_value,
        &["ID", "ACTION", "ACTOR", "WORK ITEM", "DETAIL", "CREATED"],
        &rows,
    )
}

fn activity_row_from_raw(raw: &Value) -> Vec<String> {
    let detail = raw
        .get("detail")
        .filter(|d| !d.is_null())
        .map(Value::to_string)
        .unwrap_or_else(|| "-".to_string());
    vec![
        raw.get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        raw.get("action")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        raw.get("actor")
            .and_then(|a| a.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string(),
        raw.get("workItem")
            .and_then(|w| w.get("key"))
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string(),
        detail,
        raw.get("createdAt")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    ]
}

async fn use_project(session: &mut Session<'_>, key: &str) -> Result<(), CliError> {
    let selection = session.selection()?;
    // SPEC §35: validate the project through the Public API — inside the
    // resolved organization — before persisting it, so a typo never lands in
    // the config and fails later with an opaque not-found.
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .get_project(&org, key)
        .await
        .map_err(CliError::from_client)?;
    let project: Project = response.value;

    let profile_name = ensure_profile_for_default(session, &selection).await?;

    let mut config = session.config.load()?;
    let profile = config
        .profiles
        .get_mut(&profile_name)
        .ok_or_else(|| CliError::config(format!("no such profile: {profile_name}")))?;
    profile.default_project = Some(project.key);
    session.config.save(&config)?;

    if session.json() {
        emit_json(
            session,
            &json!({ "profile": profile_name, "defaultProject": key }),
        )
    } else {
        session
            .out
            .line(&format!(
                "Default project for profile {profile_name}: {key}"
            ))
            .map_err(CliError::general)
    }
}
