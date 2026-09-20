// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik _hamstik_dyn_complete <type> <prefix>` — dynamic shell completion for live values.
//!
//! This is an internal, hidden subcommand invoked by the generated shell
//! completion scripts. It resolves the same context as any other read command,
//! queries the API for live values, and returns one candidate per line to
//! stdout.
//!
//! Acceptance criteria (CLI-32):
//! - Never issues a mutating request.
//! - Never hangs when offline (bounded timeout with graceful fallback).
//! - Never writes credentials to disk.
//! - Respects the existing context precedence (flag > env > `.hamstik.toml` > profile).
//! - Fully offline/no-credential environments degrade to no suggestions without error noise.

use std::collections::HashSet;
use std::sync::Arc;

use hamstik_api_client::{
    ClientError, FollowPolicy, HamstikApi, ListOptions, ListProjectsOptions, ListWorkItemsQuery,
    PageItems, follow_with,
};

use crate::app::{Selection, Session};
use crate::args::{CompleteArgs, CompleteType};
use crate::error::CliError;

/// Page size used for completion traversals. Completion is an interactive
/// operation, so we fetch a bounded number of pages per collection (the client
/// also caps traversal at [`hamstik_api_client::pagination::MAX_FOLLOW_PAGES`] /
/// [`hamstik_api_client::pagination::MAX_FOLLOW_ITEMS`]).
const COMPLETION_PAGE_SIZE: u32 = 100;

/// Returns completion candidates for the given type and prefix.
pub async fn run(session: &mut Session<'_>, args: &CompleteArgs) -> Result<(), CliError> {
    // Resolve context and build the API client. When context or credentials
    // are unavailable (offline, unauthenticated, or no organization resolved
    // for org-scoped types), emit nothing — the completion scripts treat an
    // empty stdout as "no candidates" and the shell falls back to its default
    // completion.
    let selection = match session.selection() {
        Ok(sel) => sel,
        Err(_) => return Ok(()),
    };

    let api = match session.api(&selection) {
        Ok(a) => a,
        Err(_) => return Ok(()),
    };

    let candidates: Vec<String> = match args.typ {
        CompleteType::Org => complete_orgs(session, args, &api).await,
        CompleteType::Project => complete_projects(session, args, &api, &selection).await,
        CompleteType::WorkItemKey => complete_work_items(session, args, &api, &selection).await,
        CompleteType::Label => complete_labels(session, args, &api, &selection).await,
        CompleteType::Status => Ok(complete_static("status", &args.prefix)),
        CompleteType::Type => Ok(complete_static("type", &args.prefix)),
    }
    .map_err(CliError::from_client)?;

    let mut candidates = candidates;
    candidates.sort();
    candidates.dedup();

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    for candidate in candidates {
        std::io::Write::write_all(&mut handle, candidate.as_bytes())
            .map_err(|err| CliError::general(format!("write error: {err}")))?;
        std::io::Write::write_all(&mut handle, b"\n")
            .map_err(|err| CliError::general(format!("write error: {err}")))?;
    }
    Ok(())
}

/// Filter a set of candidates to those matching the typed prefix.
fn filter_candidates<T: AsRef<str>>(
    values: impl IntoIterator<Item = T>,
    prefix: &str,
) -> Vec<String> {
    values
        .into_iter()
        .map(|v| v.as_ref().to_string())
        .filter(|v| v.starts_with(prefix))
        .collect()
}

/// Organization slugs matching the prefix (all pages).
async fn complete_orgs(
    _session: &mut Session<'_>,
    args: &CompleteArgs,
    api: &Arc<dyn HamstikApi>,
) -> Result<Vec<String>, ClientError> {
    let fetch_api = api.clone();
    let page = follow_with(FollowPolicy::all(), move |cursor| {
        let fetch_api = fetch_api.clone();
        async move {
            let response = fetch_api
                .list_organizations(ListOptions {
                    limit: Some(COMPLETION_PAGE_SIZE),
                    cursor,
                })
                .await?;
            Ok(PageItems::new(
                response.value.items,
                &response.raw,
                response.value.page,
            ))
        }
    })
    .await?;
    Ok(filter_candidates(
        page.items.into_iter().map(|org| org.slug),
        &args.prefix,
    ))
}

/// Project keys matching the prefix for the resolved org.
///
/// Fetches both archived and non-archived Projects so a completed key can
/// still point at an archived Project.
async fn complete_projects(
    _session: &mut Session<'_>,
    args: &CompleteArgs,
    api: &Arc<dyn HamstikApi>,
    selection: &Selection,
) -> Result<Vec<String>, ClientError> {
    let org = selection.organization.value.as_ref().ok_or_else(|| {
        ClientError::Protocol("no organization resolved for project completion".to_string())
    })?;

    let mut keys: HashSet<String> = HashSet::new();
    for archived in [false, true] {
        let org = org.to_string();
        let fetch_api = api.clone();
        let page = follow_with(FollowPolicy::all(), move |cursor| {
            let org = org.clone();
            let fetch_api = fetch_api.clone();
            async move {
                let opts = ListProjectsOptions {
                    limit: Some(COMPLETION_PAGE_SIZE),
                    cursor,
                    archived: Some(archived),
                };
                let response = fetch_api.list_projects(&org, opts).await?;
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        })
        .await?;
        for p in page.items {
            keys.insert(p.key);
        }
    }
    Ok(filter_candidates(keys, &args.prefix))
}

/// Work Item keys matching the prefix for the resolved org/project.
///
/// A resolved Project scopes the query to that Project's Work Items; otherwise
/// the Organization Work collection is queried (no project resolved).
async fn complete_work_items(
    _session: &mut Session<'_>,
    args: &CompleteArgs,
    api: &Arc<dyn HamstikApi>,
    selection: &Selection,
) -> Result<Vec<String>, ClientError> {
    let org = selection.organization.value.as_ref().ok_or_else(|| {
        ClientError::Protocol("no organization resolved for work item completion".to_string())
    })?;

    let mut keys: HashSet<String> = HashSet::new();
    let project_key = selection.project.value.as_deref();

    if let Some(project_key) = project_key {
        let fetch_api = api.clone();
        let org = org.to_string();
        let project_key = project_key.to_string();
        let page = follow_with(FollowPolicy::all(), move |cursor| {
            let org = org.clone();
            let project_key = project_key.clone();
            let fetch_api = fetch_api.clone();
            async move {
                let query = ListWorkItemsQuery {
                    limit: Some(COMPLETION_PAGE_SIZE),
                    fields: Some("key".to_string()),
                    cursor,
                    ..Default::default()
                };
                let response = fetch_api.list_work_items(&org, &project_key, query).await?;
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        })
        .await?;
        for item in page.items {
            keys.insert(item.key);
        }
    } else {
        let fetch_api = api.clone();
        let org = org.to_string();
        let page = follow_with(FollowPolicy::all(), move |cursor| {
            let org = org.clone();
            let fetch_api = fetch_api.clone();
            async move {
                let query = ListWorkItemsQuery {
                    limit: Some(COMPLETION_PAGE_SIZE),
                    fields: Some("key".to_string()),
                    cursor,
                    ..Default::default()
                };
                let response = fetch_api.list_organization_work_items(&org, query).await?;
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        })
        .await?;
        for item in page.items {
            keys.insert(item.summary.key);
        }
    }
    Ok(filter_candidates(keys, &args.prefix))
}

/// Project label names matching the prefix for the resolved org/project.
///
/// The v1 API lists a Project's labels directly (`GET .../projects/{key}/labels`),
/// so the full label set is fetched and filtered client-side.
async fn complete_labels(
    _session: &mut Session<'_>,
    args: &CompleteArgs,
    api: &Arc<dyn HamstikApi>,
    selection: &Selection,
) -> Result<Vec<String>, ClientError> {
    let org = selection.organization.value.as_ref().ok_or_else(|| {
        ClientError::Protocol("no organization resolved for label completion".to_string())
    })?;
    let project_key = selection.project.value.as_ref().ok_or_else(|| {
        ClientError::Protocol("no project resolved for label completion".to_string())
    })?;

    let fetch_api = api.clone();
    let org = org.to_string();
    let project_key = project_key.to_string();
    let page = follow_with(FollowPolicy::all(), move |cursor| {
        let org = org.clone();
        let project_key = project_key.clone();
        let fetch_api = fetch_api.clone();
        async move {
            let response = fetch_api
                .list_labels(
                    &org,
                    &project_key,
                    ListOptions {
                        limit: Some(COMPLETION_PAGE_SIZE),
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
    .await?;
    Ok(filter_candidates(
        page.items.into_iter().map(|label| label.name),
        &args.prefix,
    ))
}

/// Static candidates for the work item status or type enums.
///
/// The values are fixed by the API contract (`WorkItemSummary::status` /
/// `::type` and `CreateWorkItemRequest`), so no network lookup is needed.
fn complete_static(kind: &str, prefix: &str) -> Vec<String> {
    let values: Vec<&'static str> = match kind {
        "status" => vec!["backlog", "todo", "in_progress", "in_review", "done"],
        "type" => vec!["task", "bug", "story", "feature", "epic"],
        _ => vec![],
    };
    filter_candidates(values, prefix)
}
