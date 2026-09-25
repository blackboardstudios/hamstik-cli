// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik init` — bootstrap the working directory and print first-run
//! guidance.

use serde_json::json;

use crate::app::Session;
use crate::context::{self, CONTEXT_FILENAME, ContextFile};
use crate::error::CliError;

use super::emit_json;

/// Runs `hamstik init`.
pub fn run(session: &mut Session<'_>) -> Result<(), CliError> {
    let path = session.cwd.join(CONTEXT_FILENAME);
    if path.exists() {
        return Err(CliError::config(format!(
            "context file already exists: {}",
            path.display()
        )));
    }

    let selection = session.selection()?;
    let document = ContextFile {
        version: context::CONTEXT_VERSION,
        host: None,
        organization: selection.organization.value.clone(),
        project: selection.project.value.clone(),
        dashboard_projects: None,
    };
    context::save(&path, &document)?;
    let display = path.display().to_string();
    crate::audit::record(
        &session.config,
        &mut session.out,
        "init.run",
        &display,
        None,
    );

    if session.json() {
        return emit_json(
            session,
            &json!({
                "created": true,
                "contextFile": display,
                "organization": document.organization,
                "project": document.project,
                "nextSteps": next_steps(session, &selection),
            }),
        );
    }

    session
        .out
        .line(&format!("created context {display}"))
        .map_err(CliError::general)?;
    session.out.line("").map_err(CliError::general)?;
    session.out.line("next steps:").map_err(CliError::general)?;

    for step in next_steps(session, &selection) {
        session
            .out
            .line(&format!("  - {step}"))
            .map_err(CliError::general)?;
    }

    Ok(())
}

/// Tailored next-step suggestions based on the current resolution state.
fn next_steps(session: &Session<'_>, selection: &crate::app::Selection) -> Vec<String> {
    let mut steps = Vec::new();

    // Auth guidance.
    let has_token = session.env.var("HAMSTIK_TOKEN").is_some()
        || selection.profile.is_some()
        || selection.ephemeral_token;
    if !has_token {
        steps.push("authenticate: run `hamstik auth login` or set HAMSTIK_TOKEN".to_string());
    }

    // Organization guidance.
    if selection.organization.value.is_none() {
        steps.push("select an organization: run `hamstik org list` then `hamstik context set --org <slug>`".to_string());
    }

    if selection.project.value.is_none() && selection.organization.value.is_some() {
        steps.push("select a project: run `hamstik project list` then `hamstik context set --project <key>`".to_string());
    }

    // Exploration guidance.
    steps.push("explore work items: run `hamstik work list --status backlog`".to_string());
    steps.push("verify everything works: run `hamstik doctor`".to_string());

    steps
}
