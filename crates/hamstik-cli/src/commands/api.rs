// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik api` — Public API metadata and the v1 passthrough.

use hamstik_api_client::{MultipartFile, RateLimitSnapshot};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;
use std::path::Path;
use uuid::Uuid;

use crate::app::Session;
use crate::args::{ApiArgs, ApiCommand, InvokeArgs, RequestArgs};
use crate::error::CliError;

use super::dryrun;
use super::emit_json;

const BUNDLED_OPENAPI: &str = include_str!("../../../../openapi/hamstik-v1.json");

/// Runs a Public API command.
pub async fn run(session: &mut Session<'_>, args: &ApiArgs) -> Result<(), CliError> {
    match args.command {
        ApiCommand::Openapi => openapi(session).await,
        ApiCommand::Invoke(ref invoke_args) => run_invoke(session, invoke_args).await,
        ApiCommand::Request(ref request_args) => run_request(session, request_args).await,
        ApiCommand::RateLimit => rate_limit(session).await,
        ApiCommand::Passthrough(ref parts) => passthrough(session, parts).await,
    }
}

#[derive(Debug)]
struct OperationParameter {
    name: String,
    location: String,
    required: bool,
}

#[derive(Debug)]
struct OperationDefinition {
    id: String,
    method: String,
    path: String,
    parameters: Vec<OperationParameter>,
    request_required: bool,
    request_content_types: BTreeSet<String>,
    response_content_types: BTreeSet<String>,
}

impl OperationDefinition {
    fn has_header(&self, name: &str) -> bool {
        self.parameters.iter().any(|parameter| {
            parameter.location == "header" && parameter.name.eq_ignore_ascii_case(name)
        })
    }

    fn requires_header(&self, name: &str) -> bool {
        self.parameters.iter().any(|parameter| {
            parameter.location == "header"
                && parameter.name.eq_ignore_ascii_case(name)
                && parameter.required
        })
    }

    fn is_multipart(&self) -> bool {
        self.request_content_types
            .iter()
            .any(|content_type| content_type.starts_with("multipart/form-data"))
    }

    fn has_json_body(&self) -> bool {
        self.request_content_types.contains("application/json")
    }

    fn has_binary_response(&self) -> bool {
        self.response_content_types.iter().any(|content_type| {
            content_type != "application/json"
                && !content_type.ends_with("+json")
                && content_type != "text/plain"
        })
    }
}

fn resolve_contract_ref<'a>(contract: &'a Value, value: &'a Value) -> Result<&'a Value, CliError> {
    let Some(reference) = value.get("$ref").and_then(Value::as_str) else {
        return Ok(value);
    };
    let pointer = reference.strip_prefix('#').ok_or_else(|| {
        CliError::general(format!(
            "unsupported non-local OpenAPI reference {reference:?}"
        ))
    })?;
    contract.pointer(pointer).ok_or_else(|| {
        CliError::general(format!(
            "bundled OpenAPI reference {reference:?} does not resolve"
        ))
    })
}

fn operation_definition(operation_id: &str) -> Result<OperationDefinition, CliError> {
    let contract: Value = serde_json::from_str(BUNDLED_OPENAPI)
        .map_err(|err| CliError::general(format!("bundled OpenAPI contract is invalid: {err}")))?;
    let paths = contract["paths"]
        .as_object()
        .ok_or_else(|| CliError::general("bundled OpenAPI contract has no paths object"))?;

    for (path, item) in paths {
        for method in ["get", "put", "post", "delete", "patch"] {
            let Some(operation) = item.get(method) else {
                continue;
            };
            if operation.get("operationId").and_then(Value::as_str) != Some(operation_id) {
                continue;
            }

            let mut parameters = BTreeMap::<(String, String), OperationParameter>::new();
            let path_parameters = item
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            let operation_parameters = operation
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten();
            for raw in path_parameters.chain(operation_parameters) {
                let parameter = resolve_contract_ref(&contract, raw)?;
                let name = parameter["name"]
                    .as_str()
                    .ok_or_else(|| CliError::general("OpenAPI parameter has no name"))?
                    .to_string();
                let location = parameter["in"]
                    .as_str()
                    .ok_or_else(|| CliError::general("OpenAPI parameter has no location"))?
                    .to_string();
                parameters.insert(
                    (location.clone(), name.clone()),
                    OperationParameter {
                        name,
                        location,
                        required: parameter["required"].as_bool().unwrap_or(false),
                    },
                );
            }

            let request_body = operation
                .get("requestBody")
                .map(|body| resolve_contract_ref(&contract, body))
                .transpose()?;
            let request_content_types = request_body
                .and_then(|body| body.get("content"))
                .and_then(Value::as_object)
                .map(|content| content.keys().cloned().collect())
                .unwrap_or_default();
            let request_required = request_body
                .and_then(|body| body.get("required"))
                .and_then(Value::as_bool)
                .unwrap_or(false);

            let mut response_content_types = BTreeSet::new();
            if let Some(responses) = operation.get("responses").and_then(Value::as_object) {
                for (status, response) in responses {
                    if !status.starts_with('2') {
                        continue;
                    }
                    let response = resolve_contract_ref(&contract, response)?;
                    if let Some(content) = response.get("content").and_then(Value::as_object) {
                        response_content_types.extend(content.keys().cloned());
                    }
                }
            }

            return Ok(OperationDefinition {
                id: operation_id.to_string(),
                method: method.to_ascii_uppercase(),
                path: path.to_string(),
                parameters: parameters.into_values().collect(),
                request_required,
                request_content_types,
                response_content_types,
            });
        }
    }

    Err(CliError::usage(format!(
        "unknown Public API v1 operationId {operation_id:?}; inspect the bundled contract with `hamstik api openapi`"
    )))
}

fn parse_assignment(spec: &str, flag: &str) -> Result<(String, String), CliError> {
    let (name, value) = spec
        .split_once('=')
        .ok_or_else(|| CliError::usage(format!("{flag} expects NAME=VALUE, got {spec:?}")))?;
    if name.is_empty() || value.is_empty() {
        return Err(CliError::usage(format!(
            "{flag} expects non-empty NAME=VALUE, got {spec:?}"
        )));
    }
    Ok((name.to_string(), value.to_string()))
}

fn read_json_body(args: &InvokeArgs) -> Result<Option<Value>, CliError> {
    if let Some(file) = &args.body_file {
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
            return Err(CliError::usage(format!("--body-file {file:?} is empty")));
        }
        return serde_json::from_str(&text).map(Some).map_err(|err| {
            CliError::usage(format!("--body-file {file:?} is not valid JSON: {err}"))
        });
    }
    if args.field.is_empty() {
        return Ok(None);
    }
    let mut object = serde_json::Map::new();
    for spec in &args.field {
        let (key, value) = parse_field(spec)?;
        if object.insert(key.clone(), value).is_some() {
            return Err(CliError::usage(format!("--field {key:?} was given twice")));
        }
    }
    Ok(Some(Value::Object(object)))
}

fn read_upload(path: &Path, content_type: Option<&str>) -> Result<MultipartFile, CliError> {
    let file = std::fs::File::open(path).map_err(|err| {
        CliError::usage(format!("cannot read --upload {}: {err}", path.display()))
    })?;
    let bytes = crate::input::read_bytes_capped(file, hamstik_api_client::client::MAX_BODY_BYTES)
        .map_err(|err| {
        CliError::usage(format!("cannot read --upload {}: {err}", path.display()))
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| CliError::usage("--upload path has no valid UTF-8 file name"))?
        .to_string();
    Ok(MultipartFile {
        file_name,
        content_type: content_type.map(str::to_string),
        bytes,
    })
}

/// Invokes one operation from the frozen v1 contract.
pub(crate) async fn run_invoke(
    session: &mut Session<'_>,
    args: &InvokeArgs,
) -> Result<(), CliError> {
    let operation = operation_definition(&args.operation_id)?;
    let selection = session.selection()?;

    let mut supplied_path = BTreeMap::new();
    for spec in &args.param {
        let (name, value) = parse_assignment(spec, "--param")?;
        if supplied_path.insert(name.clone(), value).is_some() {
            return Err(CliError::usage(format!("--param {name:?} was given twice")));
        }
    }
    let allowed_path: BTreeSet<_> = operation
        .parameters
        .iter()
        .filter(|parameter| parameter.location == "path")
        .map(|parameter| parameter.name.as_str())
        .collect();
    for name in supplied_path.keys() {
        if !allowed_path.contains(name.as_str()) {
            return Err(CliError::usage(format!(
                "operation {} has no path parameter {name:?}; expected: {}",
                operation.id,
                allowed_path.iter().copied().collect::<Vec<_>>().join(", ")
            )));
        }
    }
    for parameter in operation
        .parameters
        .iter()
        .filter(|parameter| parameter.location == "path")
    {
        if supplied_path.contains_key(&parameter.name) {
            continue;
        }
        let value = match parameter.name.as_str() {
            "organizationSlug" => session.require_org(&selection)?,
            "projectKey" => session.require_project(&selection)?,
            _ => {
                return Err(CliError::usage(format!(
                    "operation {} requires --param {}=VALUE",
                    operation.id, parameter.name
                )));
            }
        };
        supplied_path.insert(parameter.name.clone(), value);
    }

    let route = operation
        .path
        .strip_prefix("/api/v1/")
        .ok_or_else(|| CliError::general("bundled operation is outside /api/v1"))?;
    let mut segments = Vec::new();
    for segment in route.split('/') {
        if let Some(name) = segment
            .strip_prefix('{')
            .and_then(|value| value.strip_suffix('}'))
        {
            segments.push(
                supplied_path.get(name).cloned().ok_or_else(|| {
                    CliError::general(format!("unresolved path parameter {name}"))
                })?,
            );
        } else {
            segments.push(segment.to_string());
        }
    }

    let allowed_query: BTreeMap<_, _> = operation
        .parameters
        .iter()
        .filter(|parameter| parameter.location == "query")
        .map(|parameter| (parameter.name.as_str(), parameter.required))
        .collect();
    let mut query = Vec::new();
    for spec in &args.query {
        let (name, value) = parse_assignment(spec, "--query")?;
        if !allowed_query.contains_key(name.as_str()) {
            return Err(CliError::usage(format!(
                "operation {} has no query parameter {name:?}; expected: {}",
                operation.id,
                allowed_query.keys().copied().collect::<Vec<_>>().join(", ")
            )));
        }
        query.push((name, value));
    }
    for (name, required) in &allowed_query {
        if *required && !query.iter().any(|(provided, _)| provided == name) {
            return Err(CliError::usage(format!(
                "operation {} requires --query {name}=VALUE",
                operation.id
            )));
        }
    }

    let has_if_match = operation.has_header("If-Match");
    if args.if_match.is_some() && !has_if_match {
        return Err(CliError::usage(format!(
            "operation {} does not document an If-Match header",
            operation.id
        )));
    }
    if operation.requires_header("If-Match") && args.if_match.is_none() {
        return Err(CliError::usage(format!(
            "operation {} requires --if-match ETAG",
            operation.id
        )));
    }
    let mut headers = Vec::new();
    if let Some(if_match) = &args.if_match {
        headers.push(("If-Match".to_string(), if_match.clone()));
    }

    let has_idempotency = operation.has_header("Idempotency-Key");
    if args.idempotency_key.is_some() && !has_idempotency {
        return Err(CliError::usage(format!(
            "operation {} does not document an Idempotency-Key header",
            operation.id
        )));
    }
    let idempotency_key = if has_idempotency {
        match &args.idempotency_key {
            Some(key) => {
                hamstik_api_client::validate_key(key)
                    .map_err(|err| CliError::usage(err.to_string()))?;
                Some(key.clone())
            }
            None if operation.requires_header("Idempotency-Key") => {
                Some(Uuid::new_v4().to_string())
            }
            None => None,
        }
    } else {
        None
    };

    for required in operation
        .parameters
        .iter()
        .filter(|parameter| parameter.location == "header" && parameter.required)
    {
        if !required.name.eq_ignore_ascii_case("If-Match")
            && !required.name.eq_ignore_ascii_case("Idempotency-Key")
        {
            return Err(CliError::general(format!(
                "operation {} requires unsupported header {}",
                operation.id, required.name
            )));
        }
    }

    let body = read_json_body(args)?;
    if operation.is_multipart() {
        if args.upload.is_none() {
            return Err(CliError::usage(format!(
                "operation {} requires --upload FILE",
                operation.id
            )));
        }
    } else if args.upload.is_some() {
        return Err(CliError::usage(format!(
            "operation {} does not accept multipart upload",
            operation.id
        )));
    }
    if operation.has_json_body() {
        if operation.request_required && body.is_none() {
            return Err(CliError::usage(format!(
                "operation {} requires --body-file FILE or --field KEY=VALUE",
                operation.id
            )));
        }
    } else if body.is_some() {
        return Err(CliError::usage(format!(
            "operation {} does not accept a JSON request body",
            operation.id
        )));
    }

    let binary = operation.has_binary_response();
    if binary && args.output.is_none() {
        return Err(CliError::usage(format!(
            "operation {} returns binary data; --output FILE is required",
            operation.id
        )));
    }
    if !binary && args.output.is_some() {
        return Err(CliError::usage(format!(
            "operation {} returns JSON; --output applies only to binary responses",
            operation.id
        )));
    }
    if binary
        && let Some(content_type) = operation
            .response_content_types
            .iter()
            .find(|content_type| {
                content_type.as_str() != "application/json"
                    && !content_type.ends_with("+json")
                    && content_type.as_str() != "text/plain"
            })
    {
        headers.push(("Accept".to_string(), content_type.clone()));
    }

    if session.global.dry_run {
        if operation.method == "GET" {
            return Err(CliError::usage(
                "--dry-run previews mutations only; this operation is a GET".to_string(),
            ));
        }
        let preview_body = if let Some(path) = &args.upload {
            let upload = read_upload(path, args.content_type.as_deref())?;
            Some(json!({
                "multipart": "form-data",
                "part": "file",
                "fileName": upload.file_name,
                "contentType": upload.content_type.as_deref().unwrap_or("application/octet-stream"),
                "size": upload.bytes.len(),
            }))
        } else {
            body.clone()
        };
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: &operation.id,
                method: &operation.method,
                path_template: &operation.path,
                path: format!("/api/v1/{}", segments.join("/")),
                resolved: json!({"pathParameters": supplied_path, "query": query}),
                if_match: args.if_match.as_deref(),
                idempotency_key: idempotency_key.as_deref(),
                body: preview_body,
                notes: vec!["request shape resolved from the bundled Public API v1 contract"],
            },
        );
    }

    let api = session.api(&selection)?;
    if binary {
        let download = api
            .download_operation(&operation.method, &segments, query, headers)
            .await
            .map_err(CliError::from_client)?;
        let output = args.output.as_deref().ok_or_else(|| {
            CliError::general("binary operation passed validation without an output path")
        })?;
        crate::fsutil::write_atomic(output, &download.bytes).map_err(|err| {
            CliError::general(format!(
                "cannot write download to {}: {err}",
                output.display()
            ))
        })?;
        if session.json() {
            return emit_json(
                session,
                &json!({
                    "operationId": operation.id,
                    "path": format!("/api/v1/{}", segments.join("/")),
                    "output": output,
                    "size": download.bytes.len(),
                    "contentType": download.content_type,
                    "requestId": download.request_id,
                }),
            );
        }
        return session
            .out
            .line(&format!(
                "downloaded {} bytes to {}",
                download.bytes.len(),
                output.display()
            ))
            .map_err(CliError::general);
    }

    let response = if operation.is_multipart() {
        let upload_path = args.upload.as_deref().ok_or_else(|| {
            CliError::general("multipart operation passed validation without an upload path")
        })?;
        let upload = read_upload(upload_path, args.content_type.as_deref())?;
        api.upload_operation_file(
            &operation.method,
            &segments,
            query,
            headers,
            upload,
            idempotency_key.as_deref(),
        )
        .await
    } else {
        api.invoke_operation(
            &operation.method,
            &segments,
            query,
            headers,
            body.as_ref(),
            idempotency_key.as_deref(),
        )
        .await
    }
    .map_err(CliError::from_client)?;

    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    if operation.method != "GET" {
        crate::audit::record(
            &session.config,
            &mut session.out,
            &format!("api.invoke.{}", operation.id),
            &format!("{} /api/v1/{}", operation.method, segments.join("/")),
            response.request_id.as_deref(),
        );
    }

    if session.json() {
        let mut envelope = json!({
            "operationId": operation.id,
            "method": operation.method,
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
        .map_err(CliError::general)
}

/// `hamstik api rate-limit`: one cheap authenticated read that reports the
/// server's current `RateLimit-*` snapshot.
///
/// This is a point-in-time observation, not a promise: limits can change
/// between calls. The read reuses the same transport (and therefore the same
/// snapshot parser) as every other command. It skips the proactive
/// depleted-window wait that typed reads apply on a successful response, so
/// it reports the observed headroom immediately; a hard `429` is still
/// retried under the same bounded policy as any other command.
async fn rate_limit(session: &mut Session<'_>) -> Result<(), CliError> {
    let selection = session.selection()?;
    let api = session.api(&selection)?;
    let probe = api
        .probe_rate_limit()
        .await
        .map_err(CliError::from_client)?;

    if session.json() {
        let mut envelope = json!({ "rateLimit": probe.snapshot.map(RateLimitSnapshot::to_json) });
        if let Some(request_id) = &probe.request_id {
            envelope["requestId"] = json!(request_id);
        }
        return emit_json(session, &envelope);
    }

    match probe.snapshot {
        Some(snapshot) => session
            .out
            .line(&format!(
                "rate limit: {} of {} remaining; window resets in {}s \
                 (snapshot from one read; limits may change between calls)",
                snapshot.remaining, snapshot.limit, snapshot.reset_in
            ))
            .map_err(CliError::general),
        None => {
            session.out.warn(
                "rate limit: the server reported no RateLimit-* headers on this read; \
                 no snapshot is available",
            );
            Ok(())
        }
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

/// True when the raw passthrough generates a replay key by convention.
/// Contract-aware `api invoke` instead follows the operation definition,
/// because Public API v1.0.0 also has selected PUT/PATCH operations that
/// require idempotency and selected POST operations that do not.
fn method_generates_idempotency_key(method: &str) -> bool {
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

    let idempotency_key = if method_generates_idempotency_key(&method) {
        Some(match &args.idempotency_key {
            Some(key) => {
                hamstik_api_client::validate_key(key)
                    .map_err(|err| CliError::usage(err.to_string()))?;
                key.to_string()
            }
            None => Uuid::new_v4().to_string(),
        })
    } else if matches!(method.as_str(), "PATCH" | "PUT") {
        match &args.idempotency_key {
            Some(key) => {
                hamstik_api_client::validate_key(key)
                    .map_err(|err| CliError::usage(err.to_string()))?;
                Some(key.clone())
            }
            None => None,
        }
    } else {
        if args.idempotency_key.is_some() {
            return Err(CliError::usage(format!(
                "--idempotency-key applies to mutations only; {method} is read-only"
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
        meta.insert("rateLimit".to_string(), limit.to_json());
    }
    if meta.is_empty() {
        None
    } else {
        Some(Value::Object(meta))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_operation_is_resolvable() {
        let manifest: Value =
            serde_json::from_str(include_str!("../../../../openapi/api-parity.json")).unwrap();
        let operations = manifest["operations"].as_array().unwrap();
        assert_eq!(operations.len(), 234);
        for entry in operations {
            let id = entry["operationId"].as_str().unwrap();
            let operation = operation_definition(id).unwrap();
            assert_eq!(operation.id, id);
            assert!(operation.path.starts_with("/api/v1/"));
        }
    }

    #[test]
    fn contract_backed_transports_cover_all_content_types_and_headers() {
        let contract: Value = serde_json::from_str(BUNDLED_OPENAPI).unwrap();
        for item in contract["paths"].as_object().unwrap().values() {
            for method in ["get", "put", "post", "delete", "patch"] {
                let Some(raw) = item.get(method) else {
                    continue;
                };
                let id = raw["operationId"].as_str().unwrap();
                let operation = operation_definition(id).unwrap();
                for parameter in operation
                    .parameters
                    .iter()
                    .filter(|parameter| parameter.location == "header" && parameter.required)
                {
                    assert!(
                        parameter.name.eq_ignore_ascii_case("If-Match")
                            || parameter.name.eq_ignore_ascii_case("Idempotency-Key"),
                        "{id} requires unsupported header {}",
                        parameter.name
                    );
                }
                assert!(
                    operation.request_content_types.is_empty()
                        || operation.has_json_body()
                        || operation.is_multipart(),
                    "{id} has unsupported request content types {:?}",
                    operation.request_content_types
                );
            }
        }
    }
}
