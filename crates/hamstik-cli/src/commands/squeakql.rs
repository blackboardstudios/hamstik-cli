// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik squeakql` — validate, save, run, and reuse SqueakQL expressions.
//!
//! Query sources, in strict precedence with conflicts rejected at
//! argument-parse time: inline positional value, `--file <PATH>` (`-` =
//! stdin), or `--saved <NAME>` through the shared saved-query library
//! (CLI-57). All sources produce byte-equivalent query values after the
//! documented newline policy: the expression is passed through verbatim
//! except that a single trailing newline is stripped (the common editor
//! artifact) and no other normalization is applied — Unicode, whitespace,
//! and internal newlines are preserved exactly.
//!
//! The saved-query library resolves `<NAME>` through strict documented
//! precedence, first match wins:
//!
//! 1. repository `.hamstik/queries/<NAME>.squeakql` — discovered by walking
//!    up from the working directory; version control is the sharing
//!    mechanism, so a query committed to a repository runs identically for
//!    anyone who checks it out;
//! 2. config-dir `queries/<NAME>.squeakql` — a per-user library relocated
//!    by `HAMSTIK_CONFIG` like every other config-dir file;
//! 3. `queries.toml` entries written by `squeakql save` (unchanged).
//!
//! Name collisions across sources are never silently overridden: the first
//! match in this precedence order wins, and `squeakql list` reports every
//! source holding each name (`sources`) plus the winning one (`source`),
//! so a collision is always visible. Only plain query text is stored or
//! read — never credentials, headers, or server-side constructs. There is
//! no sync/upload: committing the file to version control is the sharing.

use std::fs;

use serde_json::{Value, json};

use hamstik_api_client::SqueakQlValidateRequest;

use crate::app::Session;
use crate::args::{SqueakQlArgs, SqueakQlCacheCommand, SqueakQlCommand};
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
        SqueakQlCommand::Run {
            name,
            explain,
            pagination,
        } => {
            // Resolution happens before any network activity; the search
            // pipeline then owns execution, output modes, and pagination.
            crate::commands::work::search_saved(session, name, *explain, pagination).await
        }
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
        SqueakQlCommand::Cache { command } => match command {
            SqueakQlCacheCommand::Size => cache_size(session),
            SqueakQlCacheCommand::Clear => cache_clear(session),
        },
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
        let library = resolve_library_query(session, name)?;
        return Ok(library.expression);
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

/// One resolved saved-query library entry.
pub(crate) struct LibraryQuery {
    /// The expression, normalized by the documented newline policy.
    pub expression: String,
    /// The winning source, in documented precedence order.
    pub source: Source,
    /// The file path for directory sources.
    pub path: Option<std::path::PathBuf>,
    /// Every source holding this name, in precedence order (a collision is
    /// visible here; the first entry is the winner).
    pub sources: Vec<Source>,
}

/// A saved-query library source, in documented precedence order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// Repository `.hamstik/queries/<NAME>.squeakql` (version-controlled).
    Repo,
    /// Config-dir `queries/<NAME>.squeakql` (per-user).
    ConfigDir,
    /// `queries.toml` entries written by `squeakql save`.
    SavedFile,
}

impl Source {
    /// The stable machine-readable source label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Source::Repo => "repo",
            Source::ConfigDir => "config-dir",
            Source::SavedFile => "saved-file",
        }
    }
}

/// Repository query directory name, discovered by walking up from the
/// working directory.
const REPO_QUERIES_DIR: &str = ".hamstik/queries";

/// Config-dir query directory name (next to `config.toml`).
const CONFIG_QUERIES_DIR: &str = "queries";

/// File suffix of library queries.
const QUERY_SUFFIX: &str = ".squeakql";

/// Upper bound on library entries per directory (mirrors the `queries.toml`
/// reviewability cap).
const MAX_LIBRARY_ENTRIES: usize = 200;

/// The repository query directory for the session's working directory, if a
/// `.hamstik/queries` directory exists in the working directory or an
/// ancestor.
fn repo_queries_dir(session: &Session<'_>) -> Option<std::path::PathBuf> {
    let mut current: Option<&std::path::Path> = Some(&session.cwd);
    while let Some(dir) = current {
        let candidate = dir.join(REPO_QUERIES_DIR);
        if candidate.is_dir() {
            return Some(candidate);
        }
        current = dir.parent();
    }
    None
}

/// Reads one library query file with the documented newline policy and the
/// saved-query size cap.
fn read_library_query(path: &std::path::Path, name: &str) -> Result<String, CliError> {
    let text = fs::read_to_string(path).map_err(|err| {
        CliError::general(format!(
            "cannot read library query {name:?} at {} ({err})",
            path.display()
        ))
    })?;
    let expression = normalize_query(&text);
    if expression.len() > crate::config::MAX_SAVED_QUERY_LENGTH {
        return Err(CliError::usage(format!(
            "library query {name:?} exceeds the {MAX} character limit",
            MAX = crate::config::MAX_SAVED_QUERY_LENGTH
        )));
    }
    Ok(expression)
}

/// Lists the valid query names in one library directory (sorted).
fn scan_library_dir(dir: &std::path::Path, label: &str) -> Result<Vec<String>, CliError> {
    let entries = fs::read_dir(dir)
        .map_err(|err| CliError::general(format!("cannot read {label} ({err})")))?;
    let mut names = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|err| CliError::general(format!("cannot read {label} ({err})")))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if path.extension().and_then(|ext| ext.to_str()) != Some("squeakql") {
            continue;
        }
        // Files whose stem is not a valid query name are not part of the
        // library surface; they are ignored rather than failing every
        // command in the directory.
        if crate::config::SavedQueryStore::validate_name(stem).is_err() {
            continue;
        }
        names.push(stem.to_string());
    }
    if names.len() > MAX_LIBRARY_ENTRIES {
        return Err(CliError::general(format!(
            "{label} holds more than {MAX_LIBRARY_ENTRIES} queries; keep the directory reviewable"
        )));
    }
    names.sort();
    Ok(names)
}

/// Resolves one saved-query name through the documented library precedence.
/// The first source holding the name wins; every holding source is reported
/// so a collision is never silent.
pub(crate) fn resolve_library_query(
    session: &Session<'_>,
    name: &str,
) -> Result<LibraryQuery, CliError> {
    crate::config::SavedQueryStore::validate_name(name)?;
    let store = saved_store(session)?;
    let file_document = store.load()?;

    let repo_dir = repo_queries_dir(session);
    let config_dir = session
        .config
        .path()
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(CONFIG_QUERIES_DIR);

    let repo_names = repo_dir
        .as_ref()
        .map(|dir| scan_library_dir(dir, "the repository query directory"))
        .transpose()?;
    let config_names = if config_dir.is_dir() {
        Some(scan_library_dir(&config_dir, "the config query directory")?)
    } else {
        None
    };

    let mut sources: Vec<Source> = Vec::new();
    if repo_names
        .as_ref()
        .is_some_and(|names| names.contains(&name.to_string()))
    {
        sources.push(Source::Repo);
    }
    if config_names
        .as_ref()
        .is_some_and(|names| names.contains(&name.to_string()))
    {
        sources.push(Source::ConfigDir);
    }
    if file_document.queries.contains_key(name) {
        sources.push(Source::SavedFile);
    }
    if sources.is_empty() {
        return Err(CliError::usage(format!(
            "no saved query named {name:?}; list saved queries with `hamstik squeakql list`"
        )));
    }

    let winner = sources[0];
    let (expression, path) = match winner {
        Source::Repo => {
            let Some(dir) = repo_dir else {
                return Err(CliError::general(
                    "repository query directory disappeared during resolution",
                ));
            };
            let path = dir.join(format!("{name}{QUERY_SUFFIX}"));
            (read_library_query(&path, name)?, Some(path))
        }
        Source::ConfigDir => {
            let path = config_dir.join(format!("{name}{QUERY_SUFFIX}"));
            (read_library_query(&path, name)?, Some(path))
        }
        Source::SavedFile => {
            let entry = &file_document.queries[name];
            (entry.query.clone(), None)
        }
    };

    Ok(LibraryQuery {
        expression,
        source: winner,
        path,
        sources,
    })
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

    // Merge every library source into one sorted view. A name held by
    // several sources lists all of them (`sources`, precedence order); the
    // `source` field names the winner, so a collision is never silent.
    let mut merged: std::collections::BTreeMap<String, (Value, Vec<&'static str>)> =
        std::collections::BTreeMap::new();
    let repo_dir = repo_queries_dir(session);
    if let Some(dir) = &repo_dir {
        for name in scan_library_dir(dir, "the repository query directory")? {
            let path = dir.join(format!("{name}{QUERY_SUFFIX}"));
            let entry = json!({
                "name": name,
                "source": Source::Repo.label(),
                "path": path.display().to_string(),
                "query": Value::Null,
            });
            merged.insert(name, (entry, vec![Source::Repo.label()]));
        }
    }
    let config_dir = session
        .config
        .path()
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(CONFIG_QUERIES_DIR);
    if config_dir.is_dir() {
        for name in scan_library_dir(&config_dir, "the config query directory")? {
            let path = config_dir.join(format!("{name}{QUERY_SUFFIX}"));
            let entry = json!({
                "name": name,
                "source": Source::ConfigDir.label(),
                "path": path.display().to_string(),
                "query": Value::Null,
            });
            merged
                .entry(name)
                .and_modify(|(existing, sources)| {
                    existing["query"] = Value::Null;
                    sources.push(Source::ConfigDir.label());
                })
                .or_insert((entry, vec![Source::ConfigDir.label()]));
        }
    }
    for (name, entry) in &file.queries {
        let query_entry = json!({
            "name": name,
            "source": Source::SavedFile.label(),
            "path": store.path().display().to_string(),
            "query": entry.query,
            "savedAt": entry.saved_at,
        });
        merged
            .entry(name.clone())
            .and_modify(|(existing, sources)| {
                existing["query"] = json!(entry.query);
                existing["savedAt"] = json!(entry.saved_at);
                sources.push(Source::SavedFile.label());
            })
            .or_insert((query_entry, vec![Source::SavedFile.label()]));
    }

    if merged.is_empty() {
        if session.json() {
            return emit_json(session, &json!({ "queries": [] }));
        }
        return session
            .out
            .human("no saved queries; save one with `hamstik squeakql save <name> '<query>'`, \n                   or add `<NAME>.squeakql` files under `.hamstik/queries/` or the config `queries/` directory")
            .map_err(CliError::general);
    }

    if session.json() {
        let queries: Vec<serde_json::Value> = merged
            .values()
            .map(|(entry, sources)| {
                let mut entry = entry.clone();
                entry["sources"] =
                    json!(sources.iter().map(|s| (*s).to_string()).collect::<Vec<_>>());
                entry
            })
            .collect();
        return emit_json(session, &json!({ "queries": queries }));
    }
    for (name, (entry, sources)) in &merged {
        let mut line = String::from(name);
        if sources.len() > 1 {
            let labels: Vec<&str> = sources.clone();
            line.push_str(&format!(
                " [{} -> {}]",
                labels.join(", "),
                entry["source"].as_str().unwrap_or_default()
            ));
        } else {
            line.push_str(&format!(
                " [{}]",
                entry["source"].as_str().unwrap_or_default()
            ));
        }
        if let Some(query) = entry["query"].as_str() {
            let preview = query.lines().next().unwrap_or("").to_string();
            let ellipsis = if query.contains('\n') {
                format!(" {}", session.glyphs().ellipsis())
            } else {
                String::new()
            };
            line.push_str(&format!(": {preview}{ellipsis}"));
        }
        session.out.line(&line).map_err(CliError::general)?;
    }
    Ok(())
}

/// Shows one saved query's full expression.
fn show(session: &mut Session<'_>, name: &str) -> Result<(), CliError> {
    let library = resolve_library_query(session, name)?;
    if session.json() {
        return emit_json(
            session,
            &json!({
                "name": name,
                "query": library.expression,
                "source": library.source.label(),
                "path": library.path.as_ref().map(|p| p.display().to_string()),
                "sources": library.sources.iter().map(|s| s.label().to_string()).collect::<Vec<_>>(),
            }),
        );
    }
    session
        .out
        .line(&library.expression)
        .map_err(CliError::general)
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
    crate::audit::record(
        &session.config,
        &mut session.out,
        "squeakql.save",
        name,
        None,
    );
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
    crate::audit::record(
        &session.config,
        &mut session.out,
        "squeakql.delete",
        name,
        None,
    );
    if session.json() {
        return emit_json(session, &json!({ "deleted": true, "name": name }));
    }
    session
        .out
        .line(&format!("deleted query {name}"))
        .map_err(CliError::general)
}

/// Locates the saved-query store for the session's config.
fn saved_store(session: &Session<'_>) -> Result<crate::config::SavedQueryStore, CliError> {
    Ok(crate::config::SavedQueryStore::adjacent_to(
        session.config.path(),
    ))
}

/// Reports the local saved-query cache size without any network access.
fn cache_size(session: &mut Session<'_>) -> Result<(), CliError> {
    let store = saved_store(session)?;
    let file = store.load()?;
    let count = file.queries.len();
    let bytes = store.byte_len()?;
    if session.json() {
        return emit_json(
            session,
            &json!({
                "path": store.path().display().to_string(),
                "exists": store.path().exists(),
                "queries": count,
                "bytes": bytes,
            }),
        );
    }
    session
        .out
        .line(&format!("Saved queries: {count}"))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("On-disk size:  {bytes} bytes"))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!("Path:          {}", store.path().display()))
        .map_err(CliError::general)
}

/// Clears the local saved-query cache without any network access.
///
/// Idempotent: clearing an already-empty cache succeeds and reports no
/// removal. Corrupt or oversized files are removed without parsing, so this is
/// also the recovery path.
fn cache_clear(session: &mut Session<'_>) -> Result<(), CliError> {
    let store = saved_store(session)?;
    let path = store.path().display().to_string();
    let cleared = store.clear()?;
    if session.json() {
        return emit_json(session, &json!({ "cleared": cleared, "path": path }));
    }
    if cleared {
        session
            .out
            .line(&format!("cleared the saved-query cache ({path})"))
            .map_err(CliError::general)
    } else {
        session
            .out
            .human(&format!("saved-query cache is already empty ({path})"))
            .map_err(CliError::general)
    }
}
