// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `--dry-run` previews for mutation commands (CLI-10).
//!
//! A preview is emitted instead of sending the mutation: the envelope carries
//! the operation name, the method plus path template, the resolved request
//! path, the sanitized header intent (`If-Match` and `Idempotency-Key` only —
//! never `Authorization` or any credential material), and the typed request
//! body the real invocation would serialize. Bulk operations and attachment
//! uploads preview metadata instead of bytes.
//!
//! Previews are client-side only: they resolve identifiers and validate local
//! input shape exactly as a real invocation would, but they are never proof
//! that server-side validation, authorization, concurrency, or idempotency
//! will succeed. No mutation request is sent and no idempotency key is
//! consumed while previewing.

use serde_json::{Value, json};

use crate::app::Session;
use crate::error::CliError;

use super::emit_json;

/// Envelope schema version for the dry-run preview document.
pub(crate) const PREVIEW_VERSION: u64 = 1;

/// Parameters describing one would-be mutation request.
pub(crate) struct PreviewRequest<'a> {
    /// Stable operation identifier (e.g. `work.create`).
    pub operation: &'a str,
    /// HTTP method the real invocation would use.
    pub method: &'a str,
    /// Path template with `{organization}`/`{project}`/`{key}` placeholders.
    pub path_template: &'a str,
    /// Fully resolved request path (placeholders substituted).
    pub path: String,
    /// Resolved target identifiers shown next to the request.
    pub resolved: Value,
    /// Optional `If-Match` value (`*` only when the user forced last-write-wins).
    pub if_match: Option<&'a str>,
    /// Idempotency key the real invocation would send, when the operation is
    /// idempotent.
    pub idempotency_key: Option<&'a str>,
    /// The typed request body as it would be serialized (metadata only for
    /// uploads), when the operation carries a body.
    pub body: Option<Value>,
    /// Human-readable notes (e.g. that an upload's bytes are not shown).
    pub notes: Vec<&'a str>,
}

/// Emits the versioned dry-run preview and returns control: the caller has
/// already resolved every input exactly as a real invocation would and sends
/// nothing after this returns `Ok(())`.
pub(crate) fn emit_preview(
    session: &mut Session<'_>,
    request: PreviewRequest<'_>,
) -> Result<(), CliError> {
    let mut headers = json!({});
    if let Some(if_match) = request.if_match {
        headers["If-Match"] = json!(if_match);
    }
    if let Some(key) = request.idempotency_key {
        headers["Idempotency-Key"] = json!(key);
    }

    let mut envelope = json!({
        "dryRun": true,
        "previewVersion": PREVIEW_VERSION,
        "operation": request.operation,
        "request": {
            "method": request.method,
            "pathTemplate": request.path_template,
            "path": request.path,
            "headers": headers,
        },
        "resolved": request.resolved,
    });
    if let Some(body) = &request.body {
        envelope["body"] = body.clone();
    }
    if !request.notes.is_empty() {
        envelope["notes"] = json!(request.notes);
    }

    if session.json() {
        return emit_json(session, &envelope);
    }

    // Human (and quiet) summary: the preview is the command's essential
    // result, so quiet mode keeps one compact line and human mode adds the
    // detail block.
    let summary = format!(
        "dry-run: {} {} (client-side preview only; no request sent, nothing validated server-side)",
        request.method, request.path
    );
    session.out.line(&summary).map_err(CliError::general)?;
    if session.out.is_quiet() {
        return Ok(());
    }
    render_detail(session, &envelope, &request)
}

/// Renders the human detail block: headers, body, and notes.
fn render_detail(
    session: &mut Session<'_>,
    envelope: &Value,
    request: &PreviewRequest<'_>,
) -> Result<(), CliError> {
    let request_headers = &envelope["request"]["headers"];
    if let Some(headers) = request_headers.as_object()
        && !headers.is_empty()
    {
        session.out.line("  headers:").map_err(CliError::general)?;
        for (name, value) in headers {
            let value = value.as_str().unwrap_or("");
            let shown = if name == "If-Match" && value == "*" {
                format!("{name}: * (last-write-wins: revision conflict protection bypassed)")
            } else {
                format!("{name}: {value}")
            };
            session
                .out
                .line(&format!("    {shown}"))
                .map_err(CliError::general)?;
        }
    }
    if let Some(body) = &request.body {
        session
            .out
            .line(&format!("  body: {body}"))
            .map_err(CliError::general)?;
    }
    for note in &request.notes {
        session
            .out
            .line(&format!("  note: {note}"))
            .map_err(CliError::general)?;
    }
    session
        .out
        .line("  this preview does not exercise server validation, authorization, concurrency, or idempotency")
        .map_err(CliError::general)?;
    Ok(())
}
