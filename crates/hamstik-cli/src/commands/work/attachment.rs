// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work attachment upload|delete`.

use serde_json::json;

use hamstik_api_client::client::DownloadedAttachment;
use hamstik_api_client::{ListOptions, PageItems, follow_with};

use crate::app::Session;
use crate::args::{WorkAttachmentArgs, WorkAttachmentCommand};
use crate::error::CliError;
use crate::terminal_image;

use super::common::idem_key;
use super::dryrun;
use super::emit_json;
use super::emit_table;
use super::emit_view;
use super::follow_policy;
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
            let base = ListOptions {
                limit: pagination.page_size(),
                cursor: pagination.cursor.clone(),
            };
            let (attachments, json_value) = if pagination.all {
                let fetch_api = api.clone();
                let org = org.clone();
                let project = project.clone();
                let key = key.clone();
                let page = follow_with(follow_policy(pagination), move |cursor| {
                    let fetch_api = fetch_api.clone();
                    let org = org.clone();
                    let project = project.clone();
                    let key = key.clone();
                    let mut opts = base.clone();
                    opts.cursor = cursor;
                    async move {
                        let response = fetch_api
                            .list_attachments(&org, &project, &key, opts)
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
                let json_value = json!({ "items": page.raw_items, "page": page.page });
                (page.items, json_value)
            } else {
                let response = api
                    .list_attachments(&org, &project, key, base)
                    .await
                    .map_err(CliError::from_client)?;
                (response.value.items, response.raw)
            };
            let rows: Vec<Vec<String>> = attachments
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
                &json_value,
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
            crate::audit::record(
                &session.config,
                &mut session.out,
                "work.attachment.upload",
                key,
                response.request_id.as_deref(),
            );
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
            let download = fetch_attachment(session, key, attachment_id).await?;
            let target = download_target(output.as_deref(), &download, attachment_id);
            write_download(session, attachment_id, &download, &target)
        }
        WorkAttachmentCommand::View {
            key,
            attachment_id,
            output,
        } => {
            let download = fetch_attachment(session, key, attachment_id).await?;
            let protocol = terminal_image::detect(session.env);
            let inline = protocol.is_some()
                && terminal_image::is_image(
                    download.content_type.as_deref(),
                    download.file_name.as_deref(),
                )
                && session.env.stdout_is_terminal()
                // Inline escape sequences only belong in the human view; every
                // structured, table, and quiet mode falls through to download.
                && session.out.mode() == crate::output::Mode::Human
                && !session.global.no_input;
            if let Some(protocol) = protocol.filter(|_| inline) {
                match terminal_image::render(
                    protocol,
                    &download.bytes,
                    download.file_name.as_deref(),
                ) {
                    Ok(rendered) => {
                        // Raw protocol bytes are only ever written here: the
                        // guards above exclude `--json`/`--jsonl`/`--tsv`,
                        // `--quiet`, `--no-input`, and non-TTY stdout.
                        session
                            .out
                            .write_bytes(&rendered)
                            .map_err(CliError::general)?;
                        session.out.write_bytes(b"\n").map_err(CliError::general)?;
                        session.out.flush().map_err(CliError::general)?;
                        session.out.verbose(&format!(
                            "rendered attachment inline via {}",
                            protocol.name()
                        ));
                        return Ok(());
                    }
                    // A server-declared image that will not decode is not fatal:
                    // fall back to the documented download behavior.
                    Err(err) => session.out.warn(&format!(
                        "note: cannot render attachment inline ({err}); downloading instead"
                    )),
                }
            }
            let target = download_target(output.as_deref(), &download, attachment_id);
            write_download(session, attachment_id, &download, &target)
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
            let response = api
                .delete_attachment(&org, &project, key, attachment_id)
                .await
                .map_err(CliError::from_client)?;
            crate::audit::record(
                &session.config,
                &mut session.out,
                "work.attachment.delete",
                key,
                response.request_id.as_deref(),
            );
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

/// Downloads one attachment's bytes and metadata.
///
/// Shared by `download` and `view` so the fallback path cannot drift from
/// the documented download behavior.
async fn fetch_attachment(
    session: &mut Session<'_>,
    key: &str,
    attachment_id: &str,
) -> Result<DownloadedAttachment, CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;
    api.download_attachment(&org, &project, key, attachment_id)
        .await
        .map_err(CliError::from_client)
}

/// Resolves where a fallback download writes, mirroring `download` exactly.
fn download_target(
    output: Option<&str>,
    download: &DownloadedAttachment,
    attachment_id: &str,
) -> std::path::PathBuf {
    match output {
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
    }
}

/// Writes downloaded bytes to `target` and reports the documented result.
///
/// This is the single implementation of the `download` output contract; both
/// `download` and the `view` fallback call it.
fn write_download(
    session: &mut Session<'_>,
    attachment_id: &str,
    download: &DownloadedAttachment,
    target: &std::path::Path,
) -> Result<(), CliError> {
    std::fs::write(target, &download.bytes)
        .map_err(|err| CliError::general(format!("cannot write {}: {err}", target.display())))?;
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
