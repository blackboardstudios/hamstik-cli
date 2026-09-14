// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik squeakql` — validate, save, and reuse SqueakQL expressions.
//!
//! Query sources (CLI-8), in strict precedence with conflicts rejected at
//! argument-parse time: inline positional value, `--file <PATH>` (`-` =
//! stdin), or `--saved <NAME>` from the local `queries.toml`. All three
//! produce byte-equivalent query values after the documented newline policy:
//! the expression is passed through verbatim except that a single trailing
//! newline is stripped (the common editor artifact) and no other
//! normalization is applied — Unicode, whitespace, and internal newlines are
//! preserved exactly.

use hamstik_api_client::SqueakQlValidateRequest;

use crate::app::Session;
use crate::args::{SqueakQlArgs, SqueakQlCommand};
use crate::config::{MAX_SAVED_QUERIES, MAX_SAVED_QUERY_LENGTH, SavedQueriesFile, SavedQuery};
use crate::error::CliError;

use super::emit_json;

/// Runs a SqueakQL command.
pub async fn run(session: &mut Session<'_>, args: &SqueakQlArgs) -> Result<(), CliError> {
    match &args.command {
        SqueakQlCommand::Validate { query, file, saved } => {
            let expression =
                resolve_expression(session, query.as_deref(), file.as_deref(), saved.as_deref())?;
            validate(session, &expression).await
        }
        SqueakQlCommand::List => list(session),
        SqueakQlCommand::Show { name } => show(session, name),
        SqueakQlCommand::Save {
            name,
            query,
            file,
            force,
        } => {
            let expression = resolve_expression(session, query.as_deref(), file.as_deref(), None)?;
            save(session, name, &expression, *force)
        }
        SqueakQlCommand::Delete { name } => delete(session, name),
    }
}

/// Resolves the effective expression from exactly one source.
///
/// `--saved` is looked up before any network activity; inline and file
/// content are passed through `resolve_text` (which enforces the size cap
/// and validates UTF-8) and then normalized by the documented newline
/// policy.
pub(crate) fn resolve_expression(
    session: &mut Session<'_>,
    query: Option<&str>,
    file: Option<&str>,
    saved: Option<&str>,
) -> Result<String, CliError> {
    if let Some(name) = saved {
        let store = saved_store(session)?;
        let file_document = store.load()?;
        let entry = file_document.queries.get(name).ok_or_else(|| {
            CliError::usage(format!(
                "no saved query named {name:?}; list saved queries with `hamstik squeakql list`"
            ))
        })?;
        return Ok(entry.query.clone());
    }
    let resolved = crate::input::resolve_text(
        query.map(str::to_string),
        file,
        &mut std::io::stdin().lock(),
    )
    .map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => CliError::usage(format!("cannot read query file: {err}")),
        std::io::ErrorKind::InvalidData => CliError::usage(format!(
            "query input is invalid ({err}); SqueakQL expressions must be valid UTF-8 text"
        )),
        _ => CliError::usage(format!("cannot read query: {err}")),
    })?
    .ok_or_else(|| CliError::usage("no SqueakQL expression provided"))?;
    // Newline policy: strip exactly one trailing newline ( editors); any
    // other whitespace is user content and preserved.
    Ok(normalize_query(&resolved))
}

/// The documented newline policy: strip exactly one trailing `\n` (with its
/// optional preceding `\r`), nothing else.
#[must_use]
pub fn normalize_query(expression: &str) -> String {
    let stripped = expression.strip_suffix('\n').unwrap_or(expression);
    stripped.strip_suffix('\r').unwrap_or(stripped).to_string()
}

async fn validate(session: &mut Session<'_>, query: &str) -> Result<(), CliError> {
    if query.trim().is_empty() {
        return Err(CliError::usage("SqueakQL query must not be empty"));
    }
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let response = api
        .validate_squeakql(
            &org,
            &SqueakQlValidateRequest {
                query: query.to_string(),
            },
        )
        .await
        .map_err(CliError::from_client)?;

    if session.json() {
        return emit_json(session, &response.raw);
    }
    if response.value.valid {
        session
            .out
            .line(&format!(
                "Valid SqueakQL (language version {})",
                response.value.language_version
            ))
            .map_err(CliError::general)
    } else {
        session
            .out
            .line(&format!(
                "Invalid SqueakQL ({} error{})",
                response.value.errors.len(),
                if response.value.errors.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ))
            .map_err(CliError::general)?;
        for diagnostic in &response.value.errors {
            session
                .out
                .line(&format!(
                    "  {}:{} {}: {}",
                    diagnostic.line,
                    diagnostic.column,
                    diagnostic.code.as_str(),
                    diagnostic.message
                ))
                .map_err(CliError::general)?;
            if let Some(suggestion) = &diagnostic.suggestion {
                session
                    .out
                    .line(&format!("    suggestion: {suggestion}"))
                    .map_err(CliError::general)?;
            }
        }
        Ok(())
    }
}

/// Lists saved queries (name + first line preview), sorted by name.
fn list(session: &mut Session<'_>) -> Result<(), CliError> {
    let store = saved_store(session)?;
    let file = store.load()?;
    if session.json() {
        let queries: Vec<serde_json::Value> = file
            .queries
            .iter()
            .map(|(name, entry)| {
                json!({
                    "name": name,
                    "query": entry.query,
                    "savedAt": entry.saved_at,
                })
            })
            .collect();
        return emit_json(session, &json!({ "queries": queries }));
    }
    if file.queries.is_empty() {
        return session
            .out
            .human("no saved queries; save one with `hamstik squeakql save <name> '<query>'`")
            .map_err(CliError::general);
    }
    for (name, entry) in &file.queries {
        let preview = entry.query.lines().next().unwrap_or("").to_string();
        let ellipsis = if entry.query.contains('\n') {
            " …"
        } else {
            ""
        };
        session
            .out
            .line(&format!("{name:<24} {preview}{ellipsis}"))
            .map_err(CliError::general)?;
    }
    Ok(())
}

/// Shows one saved query's full expression.
fn show(session: &mut Session<'_>, name: &str) -> Result<(), CliError> {
    crate::config::SavedQueryStore::validate_name(name)?;
    let store = saved_store(session)?;
    let file = store.load()?;
    let entry = file
        .queries
        .get(name)
        .ok_or_else(|| CliError::usage(format!("no saved query named {name}")))?;
    if session.json() {
        return emit_json(
            session,
            &json!({ "name": name, "query": entry.query, "savedAt": entry.saved_at }),
        );
    }
    session.out.line(&entry.query).map_err(CliError::general)
}

/// Saves an expression under a name.
fn save(
    session: &mut Session<'_>,
    name: &str,
    expression: &str,
    force: bool,
) -> Result<(), CliError> {
    crate::config::SavedQueryStore::validate_name(name)?;
    if expression.trim().is_empty() {
        return Err(CliError::usage("SqueakQL query must not be empty"));
    }
    if expression.len() > MAX_SAVED_QUERY_LENGTH {
        return Err(CliError::usage(format!(
            "saved query exceeds the {MAX_SAVED_QUERY_LENGTH} character limit"
        )));
    }
    let store = saved_store(session)?;
    let mut file: SavedQueriesFile = store.load()?;
    if file.queries.contains_key(name) && !force {
        return Err(CliError::usage(format!(
            "a saved query named {name} already exists; re-run with --force to replace it"
        )));
    }
    if file.queries.len() >= MAX_SAVED_QUERIES && !file.queries.contains_key(name) {
        return Err(CliError::usage(format!(
            "the saved-queries file already holds {MAX_SAVED_QUERIES} queries; delete one first"
        )));
    }
    file.queries.insert(
        name.to_string(),
        SavedQuery {
            query: expression.to_string(),
            saved_at: Some(crate::commands::timestamp_rfc3339()),
        },
    );
    store.save(&file)?;
    if session.json() {
        return emit_json(session, &json!({ "saved": true, "name": name }));
    }
    session
        .out
        .line(&format!("saved query {name}"))
        .map_err(CliError::general)
}

/// Deletes a saved query.
fn delete(session: &mut Session<'_>, name: &str) -> Result<(), CliError> {
    crate::config::SavedQueryStore::validate_name(name)?;
    let store = saved_store(session)?;
    let mut file = store.load()?;
    if file.queries.remove(name).is_none() {
        return Err(CliError::usage(format!("no saved query named {name}")));
    }
    store.save(&file)?;
    if session.json() {
        return emit_json(session, &json!({ "deleted": true, "name": name }));
    }
    session
        .out
        .line(&format!("deleted query {name}"))
        .map_err(CliError::general)
}

use serde_json::json;

/// Locates the saved-query store for the session's config.
fn saved_store(session: &Session<'_>) -> Result<crate::config::SavedQueryStore, CliError> {
    Ok(crate::config::SavedQueryStore::adjacent_to(
        session.config.path(),
    ))
}
