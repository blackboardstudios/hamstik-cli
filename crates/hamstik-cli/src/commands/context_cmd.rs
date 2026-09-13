// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik context` (show / set / clear / init).

use std::path::PathBuf;

use serde_json::json;

use crate::app::{Selection, Session};
use crate::args::{ContextArgs, ContextCommand};
use crate::context::{self, CONTEXT_FILENAME, ContextFile};
use crate::error::CliError;

use super::{emit_json, emit_view};

/// Runs the `context` subcommands.
pub async fn run(session: &mut Session<'_>, args: &ContextArgs) -> Result<(), CliError> {
    match &args.command {
        ContextCommand::Show { explain } => show(session, *explain),
        ContextCommand::Set { org, project } => {
            set(session, org.as_deref(), project.as_deref()).await
        }
        ContextCommand::Clear => clear(session),
        ContextCommand::Init => init(session),
    }
}

fn token_source(selection: &Selection) -> &'static str {
    if selection.ephemeral_token {
        "environment"
    } else if selection.profile.is_some() {
        "credential_store"
    } else {
        "none"
    }
}

fn show(session: &mut Session<'_>, explain: bool) -> Result<(), CliError> {
    let selection = session.selection()?;

    if session.json() {
        let value = json!({
            "host": { "value": selection.host.as_str(), "source": selection.host_source.as_key() },
            "profile": selection.profile,
            "organization": {
                "value": selection.organization.value,
                "source": selection.organization.source.as_key(),
            },
            "project": {
                "value": selection.project.value,
                "source": selection.project.source.as_key(),
            },
            "contextFile": selection.context_path,
            "token": { "source": token_source(&selection) },
        });
        return emit_json(session, &value);
    }

    let annotate = |value: &str, source: crate::context::Source| -> String {
        if explain {
            format!("{value}  ({})", source.label())
        } else {
            value.to_string()
        }
    };

    let rows = |field: &crate::context::ResolvedField| -> String {
        match &field.value {
            Some(value) => annotate(value, field.source),
            None => annotate("(not set)", crate::context::Source::Default),
        }
    };

    let profile_display = selection
        .profile
        .clone()
        .unwrap_or_else(|| "(none)".to_string());
    let context_display = selection
        .context_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "(none)".to_string());

    let lines = [
        (
            "host",
            annotate(selection.host.as_str(), selection.host_source),
        ),
        ("profile", profile_display),
        ("organization", rows(&selection.organization)),
        ("project", rows(&selection.project)),
        ("context file", context_display),
        ("token", token_source(&selection).to_string()),
    ];
    let width = lines.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (key, value) in lines {
        session
            .out
            .line(&format!("{key:<width$}  {value}"))
            .map_err(CliError::general)?;
    }
    Ok(())
}

async fn set(
    session: &mut Session<'_>,
    org: Option<&str>,
    project: Option<&str>,
) -> Result<(), CliError> {
    if org.is_none() && project.is_none() {
        return Err(CliError::usage(
            "nothing to set; pass --org and/or --project",
        ));
    }
    let selection = session.selection()?;
    // SPEC §35: validate the selected resources through the Public API before
    // persisting them. A project is validated inside the resolved
    // organization, so a typo fails here instead of surfacing later as an
    // opaque not-found.
    let resolved_org = match org {
        Some(org) => org.to_string(),
        None => session.require_org(&selection)?,
    };
    let api = session.api(&selection)?;
    if org.is_some() {
        api.get_organization(&resolved_org)
            .await
            .map_err(CliError::from_client)?;
    }
    if let Some(project) = project {
        api.get_project(&resolved_org, project)
            .await
            .map_err(CliError::from_client)?;
    }
    let path = selection
        .context_path
        .clone()
        .unwrap_or_else(|| PathBuf::from(CONTEXT_FILENAME));
    let mut document = if path.exists() {
        context::load(&path)?
    } else {
        ContextFile {
            version: context::CONTEXT_VERSION,
            host: None,
            organization: None,
            project: None,
        }
    };
    if let Some(org) = org {
        document.organization = Some(org.to_string());
    }
    if let Some(project) = project {
        document.project = Some(project.to_string());
    }
    context::save(&path, &document)?;

    emit_view(
        session,
        &json!({ "contextFile": path, "organization": document.organization, "project": document.project }),
        &path.display().to_string(),
        |session| {
            session
                .out
                .line(&format!("updated context {}", path.display()))
                .map_err(CliError::general)
        },
    )
}

fn clear(session: &mut Session<'_>) -> Result<(), CliError> {
    let selection = session.selection()?;
    let path = match &selection.context_path {
        Some(path) => path.clone(),
        None => {
            if session.json() {
                return emit_json(session, &json!({ "cleared": false, "contextFile": null }));
            }
            session
                .out
                .human("no .hamstik.toml in scope")
                .map_err(CliError::general)?;
            return Ok(());
        }
    };
    let document = ContextFile {
        version: context::CONTEXT_VERSION,
        host: None,
        organization: None,
        project: None,
    };
    context::save(&path, &document)?;
    let display = path.display().to_string();
    emit_view(
        session,
        &json!({ "cleared": true, "contextFile": display }),
        &display,
        |session| {
            session
                .out
                .line(&format!("cleared context {display}"))
                .map_err(CliError::general)
        },
    )
}

fn init(session: &mut Session<'_>) -> Result<(), CliError> {
    let selection = session.selection()?;
    let path = session.cwd.join(CONTEXT_FILENAME);
    if path.exists() {
        return Err(CliError::config(format!(
            "context file already exists: {}",
            path.display()
        )));
    }
    let document = ContextFile {
        version: context::CONTEXT_VERSION,
        host: None,
        organization: selection.organization.value.clone(),
        project: selection.project.value.clone(),
    };
    context::save(&path, &document)?;
    let display = path.display().to_string();
    emit_view(
        session,
        &json!({
            "created": true,
            "contextFile": display,
            "organization": document.organization,
            "project": document.project,
        }),
        &display,
        |session| {
            session
                .out
                .line(&format!("created context {display}"))
                .map_err(CliError::general)
        },
    )
}
