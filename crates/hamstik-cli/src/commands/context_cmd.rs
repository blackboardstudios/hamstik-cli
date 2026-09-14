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
        ContextCommand::Explain => explain(session),
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

/// One entry in a precedence chain for stable JSON output.
fn chain_json(field: &crate::context::ResolvedChain, env_var: &str) -> serde_json::Value {
    let mut sources = Vec::new();
    if let Some((value, source)) = &field.winner {
        sources.push(json!({
            "source": source.as_key(),
            "status": "winner",
            "value": value,
        }));
    }
    for (value, source) in &field.shadowed {
        sources.push(json!({
            "source": source.as_key(),
            "status": "shadowed",
            "value": value,
        }));
    }
    if field.winner.is_none() {
        sources.push(json!({
            "source": "default",
            "status": "unset",
            "value": serde_json::Value::Null,
        }));
    }
    json!({
        "envVar": env_var,
        "sources": sources,
        "winningSource": field.source().as_key(),
    })
}

/// The complete precedence explanation, fully offline.
///
/// Reuses the production resolver (`Session::selection`) for the effective
/// values and `context::explain_chains` — built on the identical
/// candidate-order logic — for the shadowed sources, so the two can never
/// disagree. Credential material is never read or displayed: the token is
/// represented only as its source.
fn explain(session: &mut Session<'_>) -> Result<(), CliError> {
    let selection = session.selection()?;
    let config = session.config.load()?;
    // A malformed context file must not abort the whole report: the
    // production resolver surfaces that error through selection(), so here a
    // failed load degrades the report to a source-less chain.
    let context_document = selection
        .context_path
        .as_deref()
        .and_then(|path| context::load(path).ok());
    let (host_chain, org_chain, project_chain) = context::explain_chains(
        (
            &session.global.host,
            &session.global.org,
            &session.global.project,
        ),
        session.env,
        context_document.as_ref(),
        selection.profile_meta.as_ref(),
    );

    // Profile selection precedence mirrors `select_profile` exactly:
    // --profile flag > HAMSTIK_PROFILE > config.active_profile.
    let profile_chain = {
        let mut sources = Vec::new();
        let mut push = |value: Option<String>, source: &str, status: &str| {
            sources.push(json!({
                "source": source,
                "status": status,
                "value": value,
            }));
        };
        match (
            session.global.profile.clone(),
            session.env.var("HAMSTIK_PROFILE"),
            config.active_profile.clone(),
        ) {
            (Some(flag), env, active) => {
                push(Some(flag), "cli", "winner");
                if let Some(env) = env {
                    push(Some(env), "environment", "shadowed");
                }
                if let Some(active) = active {
                    push(Some(active), "profile", "shadowed");
                }
            }
            (None, Some(env), active) => {
                push(Some(env), "environment", "winner");
                if let Some(active) = active {
                    push(Some(active), "profile", "shadowed");
                }
            }
            (None, None, Some(active)) => push(Some(active), "profile", "winner"),
            (None, None, None) => push(None, "default", "unset"),
        }
        json!({ "sources": sources })
    };

    // The token is represented only as its source; material is never read
    // from the store when the environment token is authoritative (and never
    // displayed in either case).
    let token_chain = {
        let env_token = session.env.var("HAMSTIK_TOKEN").is_some();
        let mut sources = Vec::new();
        if env_token {
            sources.push(json!({
                "source": "environment",
                "status": "winner",
                "value": "<set; value not displayed>",
            }));
        } else if selection.profile.is_some() {
            sources.push(json!({
                "source": "credential_store",
                "status": "winner",
                "value": "<stored credential; value never displayed>",
            }));
        } else {
            sources.push(json!({
                "source": "default",
                "status": "unset",
                "value": serde_json::Value::Null,
            }));
        }
        json!({ "sources": sources })
    };

    let context_discovery = json!({
        "searchedFrom": session.cwd.display().to_string(),
        "filename": crate::context::CONTEXT_FILENAME,
        "found": selection
            .context_path
            .as_ref()
            .map(|path| path.display().to_string()),
    });

    let color_probe = crate::terminal::color_probe(
        session.env,
        session.global.no_color,
        session.env.stdout_is_terminal(),
    );
    let input_mode = if session.global.no_input {
        "no-input (prompts and editors disabled)"
    } else {
        "interactive allowed"
    };
    let retry_mode = if session.global.no_retry {
        "disabled (--no-retry)"
    } else {
        "enabled (default policy)"
    };

    if session.json() {
        return emit_json(
            session,
            &json!({
                "schemaVersion": 1,
                "values": {
                    "host": chain_json(&host_chain, "HAMSTIK_HOST"),
                    "organization": chain_json(&org_chain, "HAMSTIK_ORG"),
                    "project": chain_json(&project_chain, "HAMSTIK_PROJECT"),
                    "profile": profile_chain,
                    "token": token_chain,
                },
                "contextDiscovery": context_discovery,
                "behavior": {
                    "color": {
                        "enabled": color_probe.ok,
                        "detail": color_probe.detail,
                    },
                    "input": input_mode,
                    "retry": retry_mode,
                },
                "localOnly": true,
            }),
        );
    }

    let render_chain = |session: &mut Session<'_>,
                        field: &crate::context::ResolvedChain,
                        env_var: &str|
     -> Result<(), CliError> {
        session
            .out
            .line(&format!("  env var: {env_var}"))
            .map_err(CliError::general)?;
        if let Some((value, source)) = &field.winner {
            session
                .out
                .line(&format!("  -> {value}  ({})", source.label()))
                .map_err(CliError::general)?;
        }
        for (value, source) in &field.shadowed {
            session
                .out
                .line(&format!(
                    "     shadowed: {value}  (would come from {})",
                    source.label()
                ))
                .map_err(CliError::general)?;
        }
        if field.winner.is_none() {
            session
                .out
                .line("  -> (unset; built-in default applies)")
                .map_err(CliError::general)?;
        }
        Ok(())
    };

    session.out.line("host:").map_err(CliError::general)?;
    render_chain(session, &host_chain, "HAMSTIK_HOST")?;
    session
        .out
        .line("organization:")
        .map_err(CliError::general)?;
    render_chain(session, &org_chain, "HAMSTIK_ORG")?;
    session.out.line("project:").map_err(CliError::general)?;
    render_chain(session, &project_chain, "HAMSTIK_PROJECT")?;
    session.out.line("profile:").map_err(CliError::general)?;
    session
        .out
        .line(&format!(
            "  -> {}",
            selection
                .profile
                .clone()
                .unwrap_or_else(|| "(none selected)".to_string())
        ))
        .map_err(CliError::general)?;
    session.out.line("token:").map_err(CliError::general)?;
    session
        .out
        .line(&format!(
            "  -> {}",
            match token_source(&selection) {
                "environment" => {
                    "HAMSTIK_TOKEN (ephemeral; value never displayed)".to_string()
                }
                "credential_store" => {
                    "stored credential for the selected profile (value never displayed)".to_string()
                }
                _ => "(no credential; authentication-required commands will fail with exit 3)"
                    .to_string(),
            }
        ))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!(
            "context discovery: searched upward from {} for {} -> {}",
            session.cwd.display(),
            crate::context::CONTEXT_FILENAME,
            selection
                .context_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "none found".to_string()),
        ))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!(
            "behavior: color={} ({}), input={input_mode}, retry={retry_mode}",
            color_probe.ok, color_probe.detail,
        ))
        .map_err(CliError::general)?;
    session
        .out
        .line("this report is fully local; no network connection was made")
        .map_err(CliError::general)?;
    Ok(())
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
