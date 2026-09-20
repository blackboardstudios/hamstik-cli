// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik _hamstik_dyn_complete <type> <prefix>` — dynamic shell completion for live values.
//!
//! This is an internal, hidden subcommand invoked by the generated shell
//! completion scripts. It resolves the same context as any other read command,
//! queries the API for live values, and returns one candidate per line to
//! stdout. Live collections fetch only their first page (at most 100 items),
//! use no retries, and use a five-second request timeout. Context, credential,
//! network, and API failures all degrade to an empty successful response so a
//! cursor is never held up by an unavailable service.
//!
//! Acceptance criteria (CLI-32):
//! - Never issues a mutating request.
//! - Never hangs when offline (bounded timeout with graceful fallback).
//! - Never writes credentials to disk.
//! - Respects the existing context precedence (flag > env > `.hamstik.toml` > profile).
//! - Fully offline/no-credential environments degrade to no suggestions without error noise.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use hamstik_api_client::{
    ClientError, HamstikApi, ListOptions, ListProjectsOptions, ListWorkItemsQuery,
};

use crate::app::{Selection, Session};
use crate::args::{CompleteArgs, CompleteType, StatusArg, TypeArg};
use crate::error::CliError;

/// Page size used for one completion request.
const COMPLETION_PAGE_SIZE: u32 = 100;

/// Total request timeout for one completion request, including connection and
/// response-body time. The completion client also disables retries.
const COMPLETION_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Returns completion candidates for the given type and prefix.
pub async fn run(session: &mut Session<'_>, args: &CompleteArgs) -> Result<(), CliError> {
    let mut candidates = match args.typ {
        CompleteType::Status | CompleteType::Type => complete_static(args.typ, &args.prefix),
        CompleteType::Org
        | CompleteType::Project
        | CompleteType::WorkItemKey
        | CompleteType::Label => {
            // Resolve context and build the API client. When context or
            // credentials are unavailable (offline, unauthenticated, or no
            // organization/project is resolved), emit nothing — the shell
            // treats an empty stdout as "no candidates".
            let selection = match session.selection() {
                Ok(sel) => sel,
                Err(_) => return Ok(()),
            };
            let api = match session.completion_api(&selection, COMPLETION_REQUEST_TIMEOUT) {
                Ok(a) => a,
                Err(_) => return Ok(()),
            };

            // Completion is best-effort by design. API and transport failures
            // are indistinguishable from an empty candidate set to the shell.
            match args.typ {
                CompleteType::Org => complete_orgs(args, &api).await,
                CompleteType::Project => complete_projects(args, &api, &selection).await,
                CompleteType::WorkItemKey => complete_work_items(args, &api, &selection).await,
                CompleteType::Label => complete_labels(args, &api, &selection).await,
                CompleteType::Status | CompleteType::Type => Ok(Vec::new()),
            }
            .unwrap_or_default()
        }
    };
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

/// Organization slugs matching the prefix from one page.
async fn complete_orgs(
    args: &CompleteArgs,
    api: &Arc<dyn HamstikApi>,
) -> Result<Vec<String>, ClientError> {
    let response = api
        .list_organizations(ListOptions {
            limit: Some(COMPLETION_PAGE_SIZE),
            cursor: None,
        })
        .await?;
    Ok(filter_candidates(
        response.value.items.into_iter().map(|org| org.slug),
        &args.prefix,
    ))
}

/// Project keys matching the prefix for the resolved org.
///
/// Fetches both archived and non-archived Projects so a completed key can
/// still point at an archived Project.
async fn complete_projects(
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
        let response = api
            .list_projects(
                &org,
                ListProjectsOptions {
                    limit: Some(COMPLETION_PAGE_SIZE),
                    cursor: None,
                    archived: Some(archived),
                },
            )
            .await?;
        for p in response.value.items {
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
        let response = api
            .list_work_items(
                org,
                project_key,
                ListWorkItemsQuery {
                    limit: Some(COMPLETION_PAGE_SIZE),
                    fields: Some("key".to_string()),
                    cursor: None,
                    ..Default::default()
                },
            )
            .await?;
        for item in response.value.items {
            keys.insert(item.key);
        }
    } else {
        let response = api
            .list_organization_work_items(
                org,
                ListWorkItemsQuery {
                    limit: Some(COMPLETION_PAGE_SIZE),
                    fields: Some("key".to_string()),
                    cursor: None,
                    ..Default::default()
                },
            )
            .await?;
        for item in response.value.items {
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

    let response = api
        .list_labels(
            org,
            project_key,
            ListOptions {
                limit: Some(COMPLETION_PAGE_SIZE),
                cursor: None,
            },
        )
        .await?;
    Ok(filter_candidates(
        response.value.items.into_iter().map(|label| label.name),
        &args.prefix,
    ))
}

/// Static candidates for the work item status or type enums.
///
/// The values come from the same request-side enums used for validation, so
/// this branch never performs network I/O.
fn complete_static(typ: CompleteType, prefix: &str) -> Vec<String> {
    match typ {
        CompleteType::Status => {
            filter_candidates(StatusArg::ALL.iter().map(|status| status.as_str()), prefix)
        }
        CompleteType::Type => filter_candidates(
            TypeArg::ALL.iter().map(|item_type| item_type.as_str()),
            prefix,
        ),
        CompleteType::Org
        | CompleteType::Project
        | CompleteType::WorkItemKey
        | CompleteType::Label => Vec::new(),
    }
}
