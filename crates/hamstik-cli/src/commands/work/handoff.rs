// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work export` / `work import`: portable Markdown handoff.
//!
//! Export writes one canonical document — YAML frontmatter plus the
//! description as the Markdown body — suitable for pasting into a
//! GitHub/GitLab issue or handing work to another tracker. Import reads that
//! same document and maps its fields onto the existing create/edit
//! operations, never reimplementing server validation: it sends the typed
//! Public API v1 request bodies the dedicated commands send, and revision
//! conflicts surface exactly as they would from `work edit`.
//!
//! Re-importing a document is idempotent. When the embedded `key` resolves,
//! the item is updated in place; otherwise the create uses an idempotency key
//! derived from the embedded key (or the explicit `--idempotency-key`), so the
//! server replays the create instead of duplicating the item. Label, link, and
//! comment follow-ups are reconciled against the current item (or keyed per
//! document position), so an import never adds the same relationship twice.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use hamstik_api_client::{
    AttachLabelRequest, Comment, CreateCommentRequest, CreateWorkItemLinkRequest,
    CreateWorkItemRequest, FollowPolicy, HamstikApi, Label, ListOptions, PageItems,
    SqueakQlSearchRequest, UpdateWorkItemRequest, WorkItem, WorkItemLink, follow_with,
    generate_key, validate_key,
};

use crate::app::Session;
use crate::args::{StatusArg, WorkExportArgs, WorkImportArgs};
use crate::error::CliError;
use crate::fsutil;
use crate::input::{MAX_TEXT_BYTES, read_capped};

use super::common::is_uuid;
use super::dryrun;
use super::emit_json;
use super::emit_view;
use super::template;

/// Schema version of the exported JSON envelope and the import result.
const DOCUMENT_VERSION: u64 = 1;

/// The import's YAML frontmatter. Unknown keys are rejected rather than
/// silently dropped, matching `work create --template`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frontmatter {
    /// Embedded Work Item key (the document identity / idempotency seed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    /// Work Item title (required in practice; validated on import).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    /// Work Item type.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    item_type: Option<String>,
    /// Work Item status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    /// Work Item priority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<String>,
    /// Label names (or ids).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    labels: Vec<String>,
    /// Outgoing links.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    links: Vec<HandoffLink>,
    /// Comments captured by `work export --comments`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    comments: Vec<HandoffComment>,
}

/// One exported link entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffLink {
    /// The relation as seen from the exported item.
    relation: String,
    /// The other Work Item's key.
    target: String,
    /// The other Work Item's title, kept for human readability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
}

/// One exported comment entry (author/timestamp are informational).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HandoffComment {
    /// Comment author display name, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    /// Creation timestamp (RFC 3339), when known.
    #[serde(default, rename = "createdAt", skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
    /// The comment body.
    body: String,
}

/// Runs `work export`.
pub(super) async fn export(
    session: &mut Session<'_>,
    args: &WorkExportArgs,
) -> Result<(), CliError> {
    let Some(key) = args.key.as_deref() else {
        return export_query(session, args).await;
    };
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let response = api
        .get_work_item(&org, &project, key)
        .await
        .map_err(CliError::from_client)?;
    let item = response.value.clone();

    let links = all_links(&api, &org, &project, &item.key).await?;
    let comments = if args.comments {
        all_comments(&api, &org, &project, &item.key).await?
    } else {
        Vec::new()
    };

    let frontmatter = Frontmatter {
        key: Some(item.key.clone()),
        title: Some(item.title.clone()),
        item_type: Some(item.item_type.clone()),
        status: Some(item.status.clone()),
        priority: Some(item.priority.clone()),
        labels: item.labels.iter().map(|label| label.name.clone()).collect(),
        links: links
            .iter()
            .map(|link| HandoffLink {
                relation: link.relation.clone(),
                target: link.other_work_item.key.clone(),
                title: Some(link.other_work_item.title.clone()),
            })
            .collect(),
        comments: comments
            .iter()
            // Soft-deleted comments have no body and are not portable.
            .filter(|comment| !comment.deleted)
            .filter_map(|comment| {
                comment.body.as_ref().map(|body| HandoffComment {
                    author: Some(comment.author.name().to_string()),
                    created_at: Some(comment.created_at.clone()),
                    body: body.clone(),
                })
            })
            .collect(),
    };

    let body = item.description.clone();
    let document = render_document(&frontmatter, body.as_deref())?;

    match args.output.as_deref().filter(|path| *path != "-") {
        Some(path) => {
            fsutil::write_atomic(Path::new(path), document.as_bytes())
                .map_err(|err| CliError::general(format!("cannot write {path}: {err}")))?;
            if session.json() {
                return emit_json(
                    session,
                    &json!({
                        "documentVersion": DOCUMENT_VERSION,
                        "format": "markdown",
                        "path": path,
                    }),
                );
            }
            session
                .out
                .human(&format!("wrote {path}"))
                .map_err(CliError::general)
        }
        None if session.json() => {
            let envelope = json!({
                "documentVersion": DOCUMENT_VERSION,
                "format": "markdown",
                "frontmatter": serde_json::to_value(&frontmatter).map_err(CliError::general)?,
                "body": body,
                "document": document,
            });
            emit_json(session, &envelope)
        }
        None => session
            .out
            .raw(&document)
            .map_err(|err| CliError::general(format!("cannot write stdout: {err}"))),
    }
}

/// Runs a `--query` collection export.
///
/// The collection is rendered through the shared list-output contract, so
/// `--format csv|jsonl|tsv|markdown|table`, `--columns`, and `--jq` behave
/// exactly as they do on `work list`/`work search`. With `--output <FILE>` the
/// bytes written are exactly what the same invocation would print to stdout.
async fn export_query(session: &mut Session<'_>, args: &WorkExportArgs) -> Result<(), CliError> {
    let expression = crate::commands::squeakql::resolve_expression(
        session,
        args.query.as_deref(),
        args.query_file.as_deref(),
        args.query_saved.as_deref(),
    )?;
    if expression.trim().is_empty() {
        return Err(CliError::usage("SqueakQL query must not be empty"));
    }
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let api = session.api(&selection)?;
    let base = SqueakQlSearchRequest {
        query: expression,
        limit: args.pagination.page_size(),
        cursor: args.pagination.cursor.clone(),
    };
    let json_value = if args.pagination.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let page = follow_with(super::follow_policy(&args.pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let mut body = base.clone();
            body.cursor = cursor;
            async move {
                let response = fetch_api
                    .search_organization_work_items_with_squeakql(&org, &body)
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
        json!({ "items": page.raw_items, "page": page.page })
    } else {
        api.search_organization_work_items_with_squeakql(&org, &base)
            .await
            .map_err(CliError::from_client)?
            .raw
    };

    let headers = [
        "KEY", "PROJECT", "TITLE", "STATUS", "TYPE", "PRIORITY", "ASSIGNEE",
    ];
    let rows = query_rows(&json_value);
    crate::commands::check_columns(&headers, &session.output_options())?;
    match args.output.as_deref().filter(|path| *path != "-") {
        Some(path) => {
            let options = session.output_options();
            let bytes = session
                .out
                .render_list_bytes(&json_value, &headers, &rows, &options)
                .map_err(CliError::general)?;
            fsutil::write_atomic(Path::new(path), &bytes)
                .map_err(|err| CliError::general(format!("cannot write {path}: {err}")))?;
            if session.json() {
                return emit_json(
                    session,
                    &json!({
                        "exportVersion": DOCUMENT_VERSION,
                        "format": output_mode_name(session.out.mode()),
                        "path": path,
                        "items": rows.len(),
                    }),
                );
            }
            session
                .out
                .human(&format!("wrote {path}"))
                .map_err(CliError::general)
        }
        None => crate::commands::render_list(session, &json_value, &headers, &rows),
    }
}

/// Table rows for an Organization-scoped Work Item query result. The columns
/// are the snapshot projection documented for `work export --query` (KEY,
/// PROJECT, TITLE, STATUS, TYPE, PRIORITY, ASSIGNEE) and are stable, so
/// CSV/TSV/Markdown export is deterministic across runs.
fn query_rows(value: &Value) -> Vec<Vec<String>> {
    let Some(items) = value.get("items").and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .map(|item| {
            vec![
                string_field(item, "key"),
                item.get("project")
                    .and_then(|project| project.get("key"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                string_field(item, "title"),
                string_field(item, "status"),
                string_field(item, "type"),
                string_field(item, "priority"),
                item.get("assignee")
                    .and_then(|assignee| assignee.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("-")
                    .to_string(),
            ]
        })
        .collect()
}

/// Reads a top-level string field, defaulting to the empty string.
fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Stable name for an output mode in export envelopes.
fn output_mode_name(mode: crate::output::Mode) -> &'static str {
    match mode {
        crate::output::Mode::Human => "table",
        crate::output::Mode::Json => "json",
        crate::output::Mode::Quiet => "quiet",
        crate::output::Mode::JsonLines => "jsonl",
        crate::output::Mode::Tsv => "tsv",
        crate::output::Mode::Csv => "csv",
        crate::output::Mode::Markdown => "markdown",
    }
}

/// Runs `work import`.
pub(super) async fn import(
    session: &mut Session<'_>,
    args: &WorkImportArgs,
) -> Result<(), CliError> {
    let source = read_source(&args.file)?;
    let source_label = if args.file == "-" {
        "stdin".to_string()
    } else {
        args.file.clone()
    };
    let (frontmatter, body) = parse_document(&source, &source_label)?;
    let normalized = Normalized::from_frontmatter(&frontmatter, &source_label)?;
    // Validate the idempotency key locally before any network access.
    let base_key = import_base_key(normalized.key.as_deref(), args.idempotency_key.as_deref())?;

    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    // A resolvable embedded key means this document describes an item that
    // already exists in the selected Project; update it in place. A missing
    // item (or no embedded key) creates one. Any other failure is a real error.
    let existing = match normalized.key.as_deref() {
        Some(key) => match api.get_work_item(&org, &project, key).await {
            Ok(response) => Some(response),
            Err(err) if err.as_api().is_some_and(|error| error.status == 404) => None,
            Err(err) => return Err(CliError::from_client(err)),
        },
        None => None,
    };

    match existing {
        Some(current) => {
            update(
                session,
                Target {
                    api: &api,
                    org: &org,
                    project: &project,
                },
                &normalized,
                body,
                base_key,
                current,
            )
            .await
        }
        None => create(session, &api, &org, &project, &normalized, body, base_key).await,
    }
}

/// The allow-listed, parsed document used by both import paths.
struct Normalized {
    key: Option<String>,
    title: String,
    item_type: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    labels: Vec<String>,
    links: Vec<HandoffLink>,
    comments: Vec<HandoffComment>,
}

impl Normalized {
    fn from_frontmatter(frontmatter: &Frontmatter, source: &str) -> Result<Self, CliError> {
        let title = frontmatter
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .ok_or_else(|| CliError::usage(format!("{source} has no `title` in its frontmatter")))?
            .to_string();

        let item_type = frontmatter
            .item_type
            .as_deref()
            .map(template::normalize_type)
            .transpose()
            .map_err(|message| CliError::usage(format!("invalid {source}: {message}")))?;
        let priority = frontmatter
            .priority
            .as_deref()
            .map(template::normalize_priority)
            .transpose()
            .map_err(|message| CliError::usage(format!("invalid {source}: {message}")))?;
        let status = frontmatter
            .status
            .as_deref()
            .map(normalize_status)
            .transpose()
            .map_err(|message| CliError::usage(format!("invalid {source}: {message}")))?;

        if frontmatter
            .labels
            .iter()
            .any(|label| label.trim().is_empty())
        {
            return Err(CliError::usage(format!(
                "invalid {source}: labels must be non-empty strings"
            )));
        }
        let labels = frontmatter
            .labels
            .iter()
            .map(|label| label.trim().to_string())
            .collect();

        for link in &frontmatter.links {
            if link.target.trim().is_empty() {
                return Err(CliError::usage(format!(
                    "invalid {source}: a link is missing its `target`"
                )));
            }
            if !matches!(link.relation.trim(), "blocks" | "blocked_by" | "relates") {
                return Err(CliError::usage(format!(
                    "invalid {source}: link relation {:?} is not one of blocks, blocked_by, relates",
                    link.relation
                )));
            }
        }
        for comment in &frontmatter.comments {
            if comment.body.trim().is_empty() {
                return Err(CliError::usage(format!(
                    "invalid {source}: a comment has an empty `body`"
                )));
            }
        }

        let key = frontmatter
            .key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string);

        Ok(Self {
            key,
            title,
            item_type,
            status,
            priority,
            labels,
            links: frontmatter.links.clone(),
            comments: frontmatter.comments.clone(),
        })
    }

    fn create_body(&self, description: Option<String>) -> CreateWorkItemRequest {
        CreateWorkItemRequest {
            title: self.title.clone(),
            description,
            item_type: self.item_type.clone(),
            status: self.status.clone(),
            priority: self.priority.clone(),
            assignee_id: None,
            assignee_public_id: None,
            sprint_id: None,
            parent_id: None,
            story_points: None,
            due_date: None,
            attributes: None,
        }
    }

    fn update_body(&self, description: Option<String>) -> UpdateWorkItemRequest {
        UpdateWorkItemRequest {
            title: Some(self.title.clone()),
            // An absent body leaves the existing description untouched; a
            // present body replaces it.
            description: description.map(Some),
            item_type: self.item_type.clone(),
            priority: self.priority.clone(),
            ..UpdateWorkItemRequest::default()
        }
    }
}

/// The resolved API target for an import.
#[derive(Clone, Copy)]
struct Target<'a> {
    api: &'a Arc<dyn HamstikApi>,
    org: &'a str,
    project: &'a str,
}

/// Creates a new Work Item and applies the document's follow-ups.
async fn create(
    session: &mut Session<'_>,
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    normalized: &Normalized,
    body: Option<String>,
    base_key: String,
) -> Result<(), CliError> {
    let create_body = normalized.create_body(body);

    if session.global.dry_run {
        let notes = follow_up_notes(normalized);
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "work.import",
                method: "POST",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items",
                path: format!("/api/v1/organizations/{org}/projects/{project}/work-items"),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "action": "create",
                    "key": normalized.key,
                }),
                if_match: None,
                idempotency_key: Some(&base_key),
                body: Some(serde_json::to_value(&create_body).map_err(CliError::general)?),
                notes,
            },
        );
    }

    let response = api
        .create_work_item(org, project, &create_body, &base_key)
        .await
        .map_err(CliError::from_client)?;
    if response.idempotency_replayed {
        session
            .out
            .warn("note: request replayed (idempotent duplicate)");
    }
    let mut item = response.value.clone();
    let mut raw = response.raw.clone();
    let mut etag = response.etag.clone();
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "work.import",
        &item.key.clone(),
        None,
        Some(item.revision),
        response.request_id.as_deref(),
    );

    // Labels, comments, and links are separate documented endpoints, so they
    // are applied after the create exactly as `work create`/`work link add`/
    // `work comment add` would. Each carries a key derived from the document
    // position so a replayed import cannot duplicate it.
    for (index, label) in normalized.labels.iter().enumerate() {
        let current_etag = ensure_etag(api, org, project, &item.key, &mut etag).await?;
        let attach_body = AttachLabelRequest {
            label_id: is_uuid(label).then(|| label.clone()),
            label: (!is_uuid(label)).then(|| label.clone()),
        };
        let attached = api
            .attach_label(
                org,
                project,
                &item.key,
                &attach_body,
                &current_etag,
                &sub_key(&base_key, "label", index),
            )
            .await
            .map_err(CliError::from_client)?;
        item = attached.value;
        raw = attached.raw;
        etag = attached.etag;
    }

    for (index, comment) in normalized.comments.iter().enumerate() {
        api.create_comment(
            org,
            project,
            &item.key,
            &CreateCommentRequest {
                body: comment.body.clone(),
                parent_comment_id: None,
            },
            &sub_key(&base_key, "comment", index),
        )
        .await
        .map_err(CliError::from_client)?;
    }

    for (index, link) in normalized.links.iter().enumerate() {
        api.create_work_item_link(
            org,
            project,
            &item.key,
            &CreateWorkItemLinkRequest {
                target_id: None,
                target_key: Some(link.target.clone()),
                relation: link.relation.clone(),
            },
            &sub_key(&base_key, "link", index),
        )
        .await
        .map_err(CliError::from_client)?;
    }

    emit_import(session, "created", normalized.key.as_deref(), &item, &raw)
}

/// Updates the existing Work Item and reconciles its follow-ups.
async fn update(
    session: &mut Session<'_>,
    target: Target<'_>,
    normalized: &Normalized,
    body: Option<String>,
    base_key: String,
    current: hamstik_api_client::ApiResponse<WorkItem>,
) -> Result<(), CliError> {
    let Target { api, org, project } = target;
    let key = current.value.key.clone();
    let update_body = normalized.update_body(body);
    let if_match = current
        .etag
        .clone()
        .ok_or_else(|| CliError::protocol("server did not return an ETag; import cannot update"))?;

    if session.global.dry_run {
        let mut notes = follow_up_notes(normalized);
        if normalized.status.is_some() {
            notes.push(
                "status is not changed on update; the existing edit command does not expose status",
            );
        }
        return dryrun::emit_preview(
            session,
            dryrun::PreviewRequest {
                operation: "work.import",
                method: "PATCH",
                path_template: "/api/v1/organizations/{organization}/projects/{project}/work-items/{key}",
                path: format!("/api/v1/organizations/{org}/projects/{project}/work-items/{key}"),
                resolved: json!({
                    "organization": org,
                    "project": project,
                    "action": "update",
                    "workItem": key,
                }),
                if_match: Some(&if_match),
                idempotency_key: None,
                body: Some(serde_json::to_value(&update_body).map_err(CliError::general)?),
                notes,
            },
        );
    }

    let response = api
        .update_work_item(org, project, &key, &update_body, &if_match)
        .await
        .map_err(CliError::from_client)?;
    let mut item = response.value.clone();
    let mut raw = response.raw.clone();
    let mut etag = response.etag.clone();
    crate::audit::record_with_revisions(
        &session.config,
        &mut session.out,
        "work.import",
        &key,
        Some(current.value.revision),
        Some(item.revision),
        response.request_id.as_deref(),
    );

    // Reconcile labels to match the document, adding and removing only the
    // difference so a re-import is a no-op.
    if let Some(latest) = reconcile_labels(
        api,
        org,
        project,
        &mut item,
        &mut etag,
        &normalized.labels,
        &base_key,
    )
    .await?
    {
        raw = latest;
    }

    // Links and comments are additive and deduplicated against what the item
    // already carries, so re-importing the same document adds nothing.
    let existing_links = all_links(api, org, project, &item.key).await?;
    for (index, link) in normalized.links.iter().enumerate() {
        let already = existing_links.iter().any(|existing| {
            existing.relation == link.relation && existing.other_work_item.key == link.target
        });
        if already {
            continue;
        }
        api.create_work_item_link(
            org,
            project,
            &item.key,
            &CreateWorkItemLinkRequest {
                target_id: None,
                target_key: Some(link.target.clone()),
                relation: link.relation.clone(),
            },
            &sub_key(&base_key, "link", index),
        )
        .await
        .map_err(CliError::from_client)?;
    }

    let existing_comments = all_comments(api, org, project, &item.key).await?;
    let existing_bodies: Vec<&str> = existing_comments
        .iter()
        .filter_map(|comment| comment.body.as_deref())
        .collect();
    for (index, comment) in normalized.comments.iter().enumerate() {
        if existing_bodies
            .iter()
            .any(|existing| *existing == comment.body)
        {
            continue;
        }
        api.create_comment(
            org,
            project,
            &item.key,
            &CreateCommentRequest {
                body: comment.body.clone(),
                parent_comment_id: None,
            },
            &sub_key(&base_key, "comment", index),
        )
        .await
        .map_err(CliError::from_client)?;
    }

    emit_import(session, "updated", normalized.key.as_deref(), &item, &raw)
}

/// Adds missing labels and removes labels the document no longer lists.
async fn reconcile_labels(
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    item: &mut WorkItem,
    etag: &mut Option<String>,
    desired: &[String],
    base_key: &str,
) -> Result<Option<Value>, CliError> {
    let mut latest_raw: Option<Value> = None;
    let to_remove: Vec<Label> = item
        .labels
        .iter()
        .filter(|current| !desired.iter().any(|want| label_matches(want, current)))
        .cloned()
        .collect();
    let to_add: Vec<&String> = desired
        .iter()
        .filter(|want| {
            !item
                .labels
                .iter()
                .any(|current| label_matches(want, current))
        })
        .collect();

    for (index, label) in to_remove.iter().enumerate() {
        let current_etag = ensure_etag(api, org, project, &item.key, etag).await?;
        let response = api
            .detach_label(
                org,
                project,
                &item.key,
                &label.id,
                &current_etag,
                &sub_key(base_key, "label-remove", index),
            )
            .await
            .map_err(CliError::from_client)?;
        *item = response.value;
        *etag = response.etag;
        latest_raw = Some(response.raw);
    }

    for (index, label) in to_add.iter().enumerate() {
        let current_etag = ensure_etag(api, org, project, &item.key, etag).await?;
        let attach_body = AttachLabelRequest {
            label_id: is_uuid(label).then(|| (*label).clone()),
            label: (!is_uuid(label)).then(|| (*label).clone()),
        };
        let response = api
            .attach_label(
                org,
                project,
                &item.key,
                &attach_body,
                &current_etag,
                &sub_key(base_key, "label", index),
            )
            .await
            .map_err(CliError::from_client)?;
        *item = response.value;
        *etag = response.etag;
        latest_raw = Some(response.raw);
    }
    Ok(latest_raw)
}

/// True when a document label selector names the given attached label.
fn label_matches(want: &str, current: &Label) -> bool {
    want == current.id || want.eq_ignore_ascii_case(&current.name)
}

/// Returns the current ETag, re-reading the item when the last response did
/// not carry one (the create response optionally omits it).
async fn ensure_etag(
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
    etag: &mut Option<String>,
) -> Result<String, CliError> {
    if let Some(value) = etag.clone() {
        return Ok(value);
    }
    let current = api
        .get_work_item(org, project, key)
        .await
        .map_err(CliError::from_client)?;
    let value = current.etag.ok_or_else(|| {
        CliError::protocol("server did not return an ETag; follow-up operations were not applied")
    })?;
    *etag = Some(value.clone());
    Ok(value)
}

/// Emits the import result: a structured envelope under `--json`, otherwise a
/// one-line summary plus the regular Work Item detail.
fn emit_import(
    session: &mut Session<'_>,
    action: &str,
    embedded_key: Option<&str>,
    item: &WorkItem,
    raw: &Value,
) -> Result<(), CliError> {
    if session.json() {
        return emit_json(
            session,
            &json!({
                "importVersion": 1,
                "action": action,
                "key": embedded_key,
                "workItem": raw,
            }),
        );
    }
    emit_view(session, raw, &item.key.clone(), |session| {
        session
            .out
            .line(&format!(
                "imported {} -> {} ({action})",
                embedded_key.unwrap_or("(no key)"),
                item.key
            ))
            .map_err(CliError::general)?;
        super::view::render_work_item(session, item, false)
    })
}

/// Human-readable notes naming the follow-ups an import would apply.
fn follow_up_notes(normalized: &Normalized) -> Vec<&'static str> {
    let mut notes = Vec::new();
    if !normalized.labels.is_empty() {
        notes.push("labels are attached after the Work Item is written");
    }
    if !normalized.links.is_empty() {
        notes.push("links are created after the Work Item is written");
    }
    if !normalized.comments.is_empty() {
        notes.push("comments are added after the Work Item is written");
    }
    notes
}

/// Parses the document's frontmatter and Markdown body.
fn parse_document(text: &str, source: &str) -> Result<(Frontmatter, Option<String>), CliError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (yaml, body) = template::split_frontmatter(text).map_err(|message| {
        CliError::usage(format!("invalid import document {source}: {message}"))
    })?;
    let frontmatter: Frontmatter = if yaml.trim().is_empty() {
        Frontmatter::default()
    } else {
        serde_norway::from_str(yaml).map_err(|err| {
            CliError::usage(format!(
                "invalid import document {source}: frontmatter is not valid YAML: {err}"
            ))
        })?
    };
    let body = body.trim_matches(['\r', '\n']);
    let body = if body.trim().is_empty() {
        None
    } else {
        Some(body.to_string())
    };
    Ok((frontmatter, body))
}

/// Renders the canonical Markdown document (frontmatter plus body).
fn render_document(
    frontmatter: &Frontmatter,
    description: Option<&str>,
) -> Result<String, CliError> {
    let yaml = serde_norway::to_string(frontmatter)
        .map_err(|err| CliError::general(format!("cannot serialize frontmatter: {err}")))?;
    let yaml = yaml.trim_end_matches('\n');
    let body = description.unwrap_or("").trim_end_matches(['\r', '\n']);
    if body.is_empty() {
        Ok(format!("---\n{yaml}\n---\n"))
    } else {
        Ok(format!("---\n{yaml}\n---\n\n{body}\n"))
    }
}

/// The idempotency key base for an import. An explicit flag wins; otherwise
/// the embedded key seeds a stable key (so re-import replays the create), and
/// a document without a key falls back to a generated one.
fn import_base_key(embedded: Option<&str>, explicit: Option<&str>) -> Result<String, CliError> {
    if let Some(key) = explicit {
        validate_key(key).map_err(|err| CliError::usage(err.to_string()))?;
        return Ok(key.to_string());
    }
    match embedded {
        Some(key) => {
            let sanitized: String = key
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':') {
                        c
                    } else {
                        '-'
                    }
                })
                .collect();
            let mut base = format!("work-import:{sanitized}");
            if base.len() > 128 {
                base.truncate(128);
            }
            validate_key(&base).map_err(|err| {
                CliError::usage(format!("cannot derive an idempotency key: {err}"))
            })?;
            Ok(base)
        }
        None => Ok(generate_key()),
    }
}

/// Derives a per-operation key from the import's base key, keeping every key
/// within the documented 8–128 byte window.
fn sub_key(base: &str, kind: &str, index: usize) -> String {
    let suffix = format!(":{kind}:{index}");
    let max_base = 128usize.saturating_sub(suffix.len()).max(8);
    let trimmed = if base.len() > max_base {
        &base[..max_base]
    } else {
        base
    };
    format!("{trimmed}{suffix}")
}

/// Validates a template `status` against the documented choices.
fn normalize_status(raw: &str) -> Result<String, String> {
    let value = raw.trim().to_ascii_lowercase();
    StatusArg::ALL
        .iter()
        .copied()
        .find(|status| status.as_str() == value)
        .map(|status| status.as_str().to_string())
        .ok_or_else(|| {
            let allowed: Vec<&str> = StatusArg::ALL
                .iter()
                .map(|status| status.as_str())
                .collect();
            format!(
                "unknown status {raw:?}; expected one of {}",
                allowed.join(", ")
            )
        })
}

/// Reads an import document from a path or stdin, capped like every text input.
fn read_source(file: &str) -> Result<String, CliError> {
    if file == "-" {
        let stdin = std::io::stdin();
        read_capped(stdin.lock(), MAX_TEXT_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read stdin: {err}")))
    } else {
        let handle = std::fs::File::open(file)
            .map_err(|err| CliError::usage(format!("cannot read {file}: {err}")))?;
        read_capped(handle, MAX_TEXT_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read {file}: {err}")))
    }
}

/// Fetches every link page for a Work Item.
async fn all_links(
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
) -> Result<Vec<WorkItemLink>, CliError> {
    let api = api.clone();
    let org = org.to_string();
    let project = project.to_string();
    let key = key.to_string();
    let page = follow_with(FollowPolicy::all(), move |cursor| {
        let api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let key = key.clone();
        async move {
            let response = api
                .list_work_item_links(
                    &org,
                    &project,
                    &key,
                    ListOptions {
                        limit: None,
                        cursor,
                    },
                )
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
    Ok(page.items)
}

/// Fetches every comment page for a Work Item.
async fn all_comments(
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
) -> Result<Vec<Comment>, CliError> {
    let api = api.clone();
    let org = org.to_string();
    let project = project.to_string();
    let key = key.to_string();
    let page = follow_with(FollowPolicy::all(), move |cursor| {
        let api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let key = key.clone();
        async move {
            let response = api
                .list_comments(
                    &org,
                    &project,
                    &key,
                    ListOptions {
                        limit: None,
                        cursor,
                    },
                )
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
    Ok(page.items)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn full_document() -> (Frontmatter, Option<String>) {
        let text = "---\n\
            key: HAM-42\n\
            title: Fix the thing\n\
            type: bug\n\
            status: in_progress\n\
            priority: high\n\
            labels:\n\
            \x20 - regression\n\
            \x20 - 11111111-1111-4111-8111-111111111111\n\
            links:\n\
            \x20 - relation: blocks\n\
            \x20   target: HAM-7\n\
            \x20   title: Other\n\
            comments:\n\
            \x20 - author: Rae\n\
            \x20   createdAt: 2026-02-01T00:00:00Z\n\
            \x20   body: A comment\n\
            ---\n\
            \n## Steps\n\nRepro\n";
        parse_document(text, "test.md").unwrap()
    }

    #[test]
    fn parses_and_validates_every_documented_field() {
        let (frontmatter, body) = full_document();
        let normalized = Normalized::from_frontmatter(&frontmatter, "test.md").unwrap();
        assert_eq!(normalized.key.as_deref(), Some("HAM-42"));
        assert_eq!(normalized.title, "Fix the thing");
        assert_eq!(normalized.item_type.as_deref(), Some("bug"));
        assert_eq!(normalized.status.as_deref(), Some("in_progress"));
        assert_eq!(normalized.priority.as_deref(), Some("high"));
        assert_eq!(normalized.labels.len(), 2);
        assert_eq!(normalized.links.len(), 1);
        assert_eq!(normalized.links[0].target, "HAM-7");
        assert_eq!(normalized.comments.len(), 1);
        assert_eq!(body.as_deref(), Some("## Steps\n\nRepro"));
    }

    #[test]
    fn render_then_parse_round_trips_the_documented_fields() {
        let (frontmatter, body) = full_document();
        let document = render_document(&frontmatter, body.as_deref()).unwrap();
        let (parsed, parsed_body) = parse_document(&document, "roundtrip.md").unwrap();
        assert_eq!(
            serde_json::to_value(&parsed).unwrap(),
            serde_json::to_value(&frontmatter).unwrap()
        );
        assert_eq!(parsed_body, body);
        assert!(document.starts_with("---\n"));
    }

    #[test]
    fn rejects_unknown_frontmatter_keys() {
        let text = "---\ntitle: T\nbogus: nope\n---\n";
        let err = parse_document(text, "test.md").unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");
    }

    #[test]
    fn rejects_missing_title_and_bad_enums() {
        let (frontmatter, _) = parse_document("---\n---\nbody\n", "t").unwrap();
        assert!(Normalized::from_frontmatter(&frontmatter, "t").is_err());

        for (field, value) in [
            ("type", "gizmo"),
            ("priority", "whenever"),
            ("status", "blocked"),
        ] {
            let text = format!("---\ntitle: T\n{field}: {value}\n---\n");
            let (frontmatter, _) = parse_document(&text, "t").unwrap();
            assert!(
                Normalized::from_frontmatter(&frontmatter, "t").is_err(),
                "{field}={value} must be rejected"
            );
        }
    }

    #[test]
    fn derives_stable_idempotency_keys() {
        let first = import_base_key(Some("GH-1234"), None).unwrap();
        let second = import_base_key(Some("GH-1234"), None).unwrap();
        assert_eq!(first, second);
        assert!(first.len() >= 8 && first.len() <= 128);
        validate_key(&first).unwrap();

        // Explicit keys always win and are validated.
        assert_eq!(
            import_base_key(Some("GH-1234"), Some("explicit-key-1")).unwrap(),
            "explicit-key-1"
        );
        assert!(import_base_key(None, Some("short")).is_err());
        // No embedded key falls back to a fresh key each time.
        assert_ne!(
            import_base_key(None, None).unwrap(),
            import_base_key(None, None).unwrap()
        );
    }

    #[test]
    fn sub_keys_stay_within_the_documented_window() {
        let long = "a".repeat(128);
        for kind in ["label", "comment", "link", "label-remove"] {
            let key = sub_key(&long, kind, 9);
            assert!(key.len() <= 128, "{key} is too long");
            validate_key(&key).unwrap();
        }
    }
}
