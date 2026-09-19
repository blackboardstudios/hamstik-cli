// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik api` — Public API metadata and the v1 passthrough.

use serde_json::{Value, json};
use std::io::Read as _;
use uuid::Uuid;

use crate::app::Session;
use crate::args::{ApiArgs, ApiCommand, RequestArgs};
use crate::error::CliError;

use super::dryrun;
use super::emit_json;

/// Runs a Public API command.
pub async fn run(session: &mut Session<'_>, args: &ApiArgs) -> Result<(), CliError> {
    match args.command {
        ApiCommand::Openapi => openapi(session).await,
        ApiCommand::Request(ref request_args) => run_request(session, request_args).await,
        ApiCommand::Passthrough(ref parts) => passthrough(session, parts).await,
    }
}

async fn openapi(session: &mut Session<'_>) -> Result<(), CliError> {
    let selection = session.selection()?;
    let api = session.public_api(&selection)?;
    let response = api.get_open_api().await.map_err(CliError::from_client)?;
    // OpenAPI is itself JSON data, so it is printed as JSON in every output
    // mode. No bearer credential is required or sent for this operation.
    emit_json(session, &response.raw)
}

/// A bare `hamstik api /api/v1/...` invocation forwards to `api request`.
///
/// The external subcommand catches any non-`openapi` first word; anything
/// that is not a valid Public API v1 path is rejected locally (exit 2)
/// before any network access, exactly like a mistyped command. `--help`
/// after the path renders the `api request` help locally (the external
/// subcommand form bypasses clap's help dispatch).
async fn passthrough(session: &mut Session<'_>, parts: &[String]) -> Result<(), CliError> {
    if parts.iter().any(|part| part == "--help" || part == "-h") {
        use clap::CommandFactory as _;
        let mut root = crate::args::Cli::command();
        if let Some(api) = root.find_subcommand_mut("api")
            && let Some(request) = api.find_subcommand_mut("request")
        {
            request.print_help().map_err(CliError::general)?;
            return Ok(());
        }
    }
    let request = RequestArgs {
        path: parts
            .first()
            .cloned()
            .ok_or_else(|| CliError::usage("expected a Public API v1 path"))?,
        method: "GET".to_string(),
        body_file: None,
        field: Vec::new(),
        query: Vec::new(),
        header: Vec::new(),
        idempotency_key: None,
    };
    run_request(session, &request).await
}

/// Validates a passthrough path and splits it into URL segments.
///
/// Only `/api/v1/...` paths are accepted — never private browser routes,
/// non-v1 paths, other origins, or anything embedding credentials. The
/// checks are local and strict so a hostile or mistaken path can never
/// reach the network: backslashes (Windows separators never occur in v1
/// routes), empty segments from `//`, `.`/`..` traversal, `@` (userinfo),
/// whitespace/control characters, and any `?`/`#` (query strings and
/// fragments go through `--query` only).
pub(crate) fn validate_api_path(path: &str) -> Result<Vec<String>, String> {
    if path.is_empty() {
        return Err("path is empty; expected /api/v1/...".to_string());
    }
    if !path.starts_with("/api/v1") {
        return Err(format!(
            "path {path:?} is not under the Public API v1 prefix /api/v1"
        ));
    }
    if path == "/api/v1" || path == "/api/v1/" {
        return Err(
            "a route is required after /api/v1 (for example /api/v1/work-items)".to_string(),
        );
    }
    if !path.starts_with("/api/v1/") {
        return Err(format!(
            "path {path:?} must start a route segment after /api/v1"
        ));
    }
    if path.contains('\\') {
        return Err(format!(
            "path {path:?} contains a backslash; Public API v1 paths use / separators only"
        ));
    }
    for (marker, reason) in [
        (
            '?',
            "query strings go through --query key=value, not the path",
        ),
        ('#', "fragments are not part of Public API v1 paths"),
    ] {
        if path.contains(marker) {
            return Err(format!(
                "path {path:?} contains {marker:?}: {marker} {reason}"
            ));
        }
    }
    let route = path["/api/v1".len()..]
        .strip_prefix('/')
        .ok_or_else(|| format!("path {path:?} must start a route segment after /api/v1"))?;
    let mut segments = Vec::new();
    for segment in route.split('/') {
        if segment.is_empty() {
            return Err(format!(
                "path {path:?} contains an empty segment; use exactly one / between segments"
            ));
        }
        if segment == "." || segment == ".." {
            return Err(format!(
                "path {path:?} contains {segment:?}; path traversal is not allowed"
            ));
        }
        if segment.chars().any(char::is_whitespace) || segment.chars().any(char::is_control) {
            return Err(format!(
                "path {path:?} contains whitespace or control characters in a segment"
            ));
        }
        if segment.contains('@') || segment.contains(':') && false {
            return Err(format!(
                "path {path:?} segment {segment:?} must not embed credentials"
            ));
        }
        if segment.contains('%') {
            // Percent-encoding is the server's contract for reserved
            // characters; let the client's resource_url percent-encoding do
            // the work instead of double-encoding here.
            return Err(format!(
                "path {path:?} contains a percent-encoded segment; pass the raw value \
                 (the CLI percent-encodes segments itself)"
            ));
        }
        segments.push(segment.to_string());
    }
    Ok(segments)
}

/// True when the method's documented contract is idempotency-key protected
/// (POST creates, DELETE removes). PATCH/PUT are revision-guarded via
/// `If-Match`, which the API does not protect with replay keys.
fn method_wants_idempotency_key(method: &str) -> bool {
    matches!(method, "POST" | "DELETE")
}

/// Parses `Name: value` header overrides against the safe allowlist.
fn parse_header_override(spec: &str) -> Result<(String, String), CliError> {
    let (name, value) = spec
        .split_once(':')
        .ok_or_else(|| CliError::usage(format!("--header expects NAME:VALUE, got {spec:?}")))?;
    let name = name.trim();
    let value = value.trim();
    if value.is_empty() {
        return Err(CliError::usage(format!(
            "--header {name:?} has an empty value"
        )));
    }
    const ALLOWED: &[&str] = &["accept", "content-type", "if-match"];
    if !ALLOWED.contains(&name.to_ascii_lowercase().as_str()) {
        return Err(CliError::usage(format!(
            "--header {name:?} is not in the safe allowlist ({ALLOWED_TEXT}); \
             credential-bearing headers are always rejected"
        )));
    }
    Ok((name.to_string(), value.to_string()))
}

const ALLOWED_TEXT: &str = "Accept, Content-Type, If-Match";

/// Parses one `key=value` query parameter; the key must be a bare token.
fn parse_query_param(spec: &str) -> Result<(String, String), CliError> {
    let (key, value) = spec
        .split_once('=')
        .ok_or_else(|| CliError::usage(format!("--query expects KEY=VALUE, got {spec:?}")))?;
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(CliError::usage(format!(
            "--query key {key:?} must be alphanumeric with - or _"
        )));
    }
    Ok((key.to_string(), value.to_string()))
}

/// Parses one structured `key=value` field; the value is parsed as JSON when
/// it parses (so `--field tags=[\"a\",\"b\"]` is structured), else a string.
fn parse_field(spec: &str) -> Result<(String, Value), CliError> {
    let (key, value) = spec
        .split_once('=')
        .ok_or_else(|| CliError::usage(format!("--field expects KEY=VALUE, got {spec:?}")))?;
    if key.is_empty() {
        return Err(CliError::usage("--field key is empty".to_string()));
    }
    let parsed = serde_json::from_str::<Value>(value).unwrap_or(Value::String(value.to_string()));
    Ok((key.to_string(), parsed))
}

async fn run_request(session: &mut Session<'_>, args: &RequestArgs) -> Result<(), CliError> {
    let segments = validate_api_path(&args.path).map_err(CliError::usage)?;
    let method = args.method.to_ascii_uppercase();

    if method == "GET" && (args.body_file.is_some() || !args.field.is_empty()) {
        return Err(CliError::usage(
            "GET requests carry no body; use --method for a mutation or drop the body".to_string(),
        ));
    }

    let body: Option<Value> = if let Some(file) = &args.body_file {
        let text = if file == "-" {
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .map_err(|err| CliError::general(format!("cannot read stdin: {err}")))?;
            buffer
        } else {
            std::fs::read_to_string(file).map_err(|err| {
                CliError::usage(format!("cannot read --body-file {file:?}: {err}"))
            })?
        };
        if text.trim().is_empty() {
            return Err(CliError::usage(format!(
                "--body-file {file:?} is empty; the API requires a JSON body for {method}"
            )));
        }
        Some(serde_json::from_str(&text).map_err(|err| {
            CliError::usage(format!("--body-file {file:?} is not valid JSON: {err}"))
        })?)
    } else if !args.field.is_empty() {
        let mut object = serde_json::Map::new();
        for spec in &args.field {
            let (key, value) = parse_field(spec)?;
            if object.insert(key.clone(), value).is_some() {
                return Err(CliError::usage(format!(
                    "--field {key:?} given twice with different values"
                )));
            }
        }
        Some(Value::Object(object))
    } else {
        None
    };

    let mut query = Vec::new();
    for spec in &args.query {
        query.push(parse_query_param(spec)?);
    }
    let mut headers = Vec::new();
    for spec in &args.header {
        let (name, value) = parse_header_override(spec)?;
        headers.push((name, value));
    }

    let idempotency_key = if method_wants_idempotency_key(&method) {
        Some(match &args.idempotency_key {
            Some(key) => {
                hamstik_api_client::validate_key(key)
                    .map_err(|err| CliError::usage(err.to_string()))?;
                key.to_string()
            }
            None => Uuid::new_v4().to_string(),
        })
    } else {
        if args.idempotency_key.is_some() {
            return Err(CliError::usage(format!(
                "--idempotency-key applies to POST/DELETE only; {method} requests are \
                 revision-guarded, not idempotency-protected"
            )));
        }
        None
    };

    if session.global.dry_run {
        if method == "GET" {
            return Err(CliError::usage(
                "--dry-run previews mutations only; GET has nothing to preview".to_string(),
            ));
        }
        let mut header_intent = serde_json::Map::new();
        for (name, value) in &headers {
            header_intent.insert(name.clone(), json!(value));
        }
        if let Some(key) = &idempotency_key {
            header_intent.insert("Idempotency-Key".to_string(), json!(key));
        }
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "api.request",
                method: &method,
                path_template: "/api/v1/...",
                path: format!("/api/v1/{}", segments.join("/")),
                resolved: json!({}),
                if_match: headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("if-match"))
                    .map(|(_, value)| value.as_str()),
                idempotency_key: idempotency_key.as_deref(),
                body,
                notes: vec![
                    "the passthrough sends exactly this request to the configured host origin",
                ],
            },
        );
    }

    let selection = session.selection()?;
    let api = session.api(&selection)?;

    let response = api
        .raw_request(
            &method,
            &segments,
            query,
            headers,
            body.as_ref(),
            idempotency_key.as_deref(),
        )
        .await
        .map_err(CliError::from_client)?;

    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }

    // The passthrough can reach any Public API mutation, so audited requests
    // are recorded like the typed commands. Only the method and path are
    // recorded; bodies, headers, and query parameters never enter the log.
    if !matches!(method.as_str(), "GET" | "HEAD") {
        crate::audit::record(
            &session.config,
            &mut session.out,
            "api.request",
            &format!("{method} /api/v1/{}", segments.join("/")),
            response.request_id.as_deref(),
        );
    }

    if session.json() {
        let mut envelope = json!({
            "method": method,
            "path": format!("/api/v1/{}", segments.join("/")),
            "data": response.raw,
        });
        if let Some(meta) = response_meta(&response) {
            envelope["meta"] = meta;
        }
        return emit_json(session, &envelope);
    }

    session
        .out
        .line(&serde_json::to_string_pretty(&response.raw).map_err(CliError::general)?)
        .map_err(CliError::general)?;
    if let Some(meta) = response_meta(&response) {
        let meta = serde_json::to_string_pretty(&meta).map_err(CliError::general)?;
        session
            .out
            .line(&format!("  meta: {meta}"))
            .map_err(CliError::general)?;
    }
    Ok(())
}

fn response_meta(response: &hamstik_api_client::ApiResponse<Value>) -> Option<Value> {
    let mut meta = serde_json::Map::new();
    if let Some(request_id) = &response.request_id {
        meta.insert("requestId".to_string(), json!(request_id));
    }
    if let Some(etag) = &response.etag {
        meta.insert("etag".to_string(), json!(etag));
    }
    if response.idempotency_replayed {
        meta.insert("idempotencyReplayed".to_string(), json!(true));
    }
    if let Some(location) = &response.location {
        meta.insert("location".to_string(), json!(location));
    }
    if let Some(limit) = &response.rate_limit {
        meta.insert(
            "rateLimit".to_string(),
            json!({"limit": limit.limit, "remaining": limit.remaining, "resetIn": limit.reset_in}),
        );
    }
    if meta.is_empty() {
        None
    } else {
        Some(Value::Object(meta))
    }
}
