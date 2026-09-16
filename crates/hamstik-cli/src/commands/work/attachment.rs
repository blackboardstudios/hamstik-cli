// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work attachment upload|delete`.

use serde_json::json;

use hamstik_api_client::ListOptions;

use crate::app::Session;
use crate::args::{WorkAttachmentArgs, WorkAttachmentCommand};
use crate::error::CliError;

use super::common::idem_key;
use super::dryrun;
use super::emit_json;
use super::emit_table;
use super::emit_view;
use super::render_lines;
pub(super) async fn attachment(
    session: &mut Session<'_>,
    args: &WorkAttachmentArgs,
) -> Result<(), CliError> {
    match &args.command {
        WorkAttachmentCommand::List { key, pagination } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let api = session.api(&selection)?;
            let response = api
                .list_attachments(
                    &org,
                    &project,
                    key,
                    ListOptions {
                        limit: pagination.limit,
                        cursor: pagination.cursor.clone(),
                    },
                )
                .await
                .map_err(CliError::from_client)?;
            let rows: Vec<Vec<String>> = response
                .value
                .items
                .iter()
                .map(|a| {
                    vec![
                        a.id.clone(),
                        a.file_name.clone(),
                        a.content_type.clone(),
                        a.size.to_string(),
                        a.created_by
                            .clone()
                            .map(|u| u.name().to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        a.created_at.clone(),
                    ]
                })
                .collect();
            emit_table(
                session,
                &response.raw,
                &["ID", "FILE", "TYPE", "SIZE", "BY", "CREATED"],
                &rows,
            )
        }
        WorkAttachmentCommand::Upload {
            key,
            file,
            file_name,
            content_type,
            idempotency_key,
        } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let (bytes, name) = read_upload(file, file_name.as_deref()).map_err(CliError::usage)?;
            let upload = hamstik_api_client::client::MultipartFile {
                file_name: name,
                content_type: content_type.clone(),
                bytes,
            };
            let idempotency = idem_key(idempotency_key.clone())?;

            if session.global.dry_run {
                return dryrun::emit_preview(
                    session,
                    dryrun::PreviewRequest {
                        operation: "work.attachment.upload",
                        method: "POST",
                        path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/attachments",
                        path: format!(
                            "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/attachments"
                        ),
                        resolved: json!({
                            "organization": org,
                            "project": project,
                            "workItem": key,
                            "fileName": upload.file_name,
                            "contentType": upload.content_type,
                            "size": upload.bytes.len(),
                        }),
                        if_match: None,
                        idempotency_key: Some(&idempotency),
                        body: Some(json!({
                            "multipart": "form-data",
                            "part": "file",
                            "fileName": upload.file_name,
                            "contentType": upload.content_type.as_deref().unwrap_or("application/octet-stream"),
                            "size": upload.bytes.len(),
                        })),
                        notes: vec!["file bytes are not shown and will not be uploaded"],
                    },
                );
            }

            let api = session.api(&selection)?;
            let response = api
                .upload_attachment(&org, &project, key, &upload, &idempotency)
                .await
                .map_err(CliError::from_client)?;
            if response.idempotency_replayed {
                session
                    .out
                    .warn("note: request replayed (idempotent duplicate)");
            }
            let attachment_id = response.value.id.clone();
            emit_view(session, &response.raw, &attachment_id.clone(), |session| {
                render_attachment(session, &response.value)
            })
        }
        WorkAttachmentCommand::Download {
            key,
            attachment_id,
            output,
        } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;
            let api = session.api(&selection)?;
            let download = api
                .download_attachment(&org, &project, key, attachment_id)
                .await
                .map_err(CliError::from_client)?;
            let target = match output {
                Some(path) => std::path::PathBuf::from(path),
                None => {
                    let name = download
                        .file_name
                        .clone()
                        .unwrap_or_else(|| format!("{attachment_id}.bin"));
                    // Refuse to write outside the current directory implicitly:
                    // use only the final path component of the server-suggested name.
                    let safe = name.rsplit(['/', '\\']).next().unwrap_or(&name);
                    std::path::PathBuf::from(safe)
                }
            };
            std::fs::write(&target, &download.bytes).map_err(|err| {
                CliError::general(format!("cannot write {}: {err}", target.display()))
            })?;
            if session.json() {
                emit_json(
                    session,
                    &json!({
                        "attachmentId": attachment_id,
                        "path": target.display().to_string(),
                        "size": download.bytes.len(),
                        "fileName": download.file_name,
                        "contentType": download.content_type,
                        "contentLength": download.content_length,
                        "contentDisposition": download.content_disposition,
                        "requestId": download.request_id,
                    }),
                )
            } else {
                session
                    .out
                    .line(&format!(
                        "Downloaded {} ({} bytes) to {}",
                        attachment_id,
                        download.bytes.len(),
                        target.display()
                    ))
                    .map_err(CliError::general)
            }
        }
        WorkAttachmentCommand::Delete { key, attachment_id } => {
            let selection = session.selection()?;
            let org = session.require_org(&selection)?;
            let project = session.require_project(&selection)?;

            if session.global.dry_run {
                return dryrun::emit_preview(
                    session,
                    dryrun::PreviewRequest {
                        operation: "work.attachment.delete",
                        method: "DELETE",
                        path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}/attachments/{attachmentId}",
                        path: format!(
                            "/api/v1/organizations/{org}/projects/{project}/work-items/{key}/attachments/{attachment_id}"
                        ),
                        resolved: json!({
                            "organization": org,
                            "project": project,
                            "workItem": key,
                            "attachmentId": attachment_id,
                        }),
                        if_match: None,
                        idempotency_key: None,
                        body: None,
                        notes: Vec::new(),
                    },
                );
            }

            let api = session.api(&selection)?;
            api.delete_attachment(&org, &project, key, attachment_id)
                .await
                .map_err(CliError::from_client)?;
            if session.json() {
                emit_json(
                    session,
                    &json!({ "deleted": true, "attachmentId": attachment_id }),
                )
            } else {
                session
                    .out
                    .line(&format!("Deleted attachment {attachment_id}"))
                    .map_err(CliError::general)
            }
        }
    }
}

fn render_attachment(
    session: &mut Session<'_>,
    attachment: &hamstik_api_client::Attachment,
) -> Result<(), CliError> {
    let lines = [
        ("id", attachment.id.clone()),
        ("file", attachment.file_name.clone()),
        ("type", attachment.content_type.clone()),
        ("size", attachment.size.to_string()),
        (
            "by",
            attachment
                .created_by
                .clone()
                .map(|u| u.name().to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("created", attachment.created_at.clone()),
    ];
    render_lines(session, &lines)
}
/// Reads upload bytes from a path or stdin (`-`), capping at the response-body
/// budget (10 MiB), and derives a default file name from the path.
///
/// Both sources are streamed through the cap, so an oversized or misdirected
/// stream is rejected at the first excess byte instead of being buffered.
/// Reads upload bytes from a path or stdin (`-`), capping at the response-body
/// budget (10 MiB), and derives a default file name from the path.
///
/// Both sources are streamed through the cap, so an oversized or misdirected
/// stream is rejected at the first excess byte instead of being buffered.
fn read_upload(file: &str, explicit_name: Option<&str>) -> Result<(Vec<u8>, String), String> {
    const MAX_UPLOAD_BYTES: usize = hamstik_api_client::client::MAX_BODY_BYTES;
    let (bytes, default_name) = if file == "-" {
        let stdin = std::io::stdin();
        let bytes = crate::input::read_bytes_capped(stdin.lock(), MAX_UPLOAD_BYTES)
            .map_err(|err| format!("cannot read stdin: {err}"))?;
        (bytes, "attachment".to_string())
    } else {
        let path = std::path::Path::new(file);
        let handle =
            std::fs::File::open(path).map_err(|err| format!("cannot read {file}: {err}"))?;
        let bytes = crate::input::read_bytes_capped(handle, MAX_UPLOAD_BYTES)
            .map_err(|err| format!("cannot read {file}: {err}"))?;
        let name = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("attachment")
            .to_string();
        (bytes, name)
    };
    let name = explicit_name.map(str::to_string).unwrap_or(default_name);
    Ok((bytes, name))
}
