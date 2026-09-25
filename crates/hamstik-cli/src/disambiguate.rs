// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Interactive disambiguation for ambiguous Organization, Project, and Work
//! Item references (CLI-38).
//!
//! Exact references (an Organization slug, a Project key, or a Work Item key)
//! keep resolving through the Public API exactly as before. This module only
//! engages *after* an exact lookup failed with `NOT_FOUND` and only when
//! interactive prompting is permitted ([`crate::app::Session::can_pick`]):
//!
//! - The candidate collection is read from the same Public API list endpoints
//!   the completion layer uses (a single bounded page).
//! - Fuzzy matching ranks exact key/name matches first, then prefix matches,
//!   then substring matches, so a unique match resolves automatically.
//! - Multiple plausible matches present an interactive select list and resolve
//!   only after an explicit choice; cancelling aborts with a usage error and
//!   no side effects.
//!
//! Every non-interactive path (`--no-input`, `--json`, `--quiet`, or a
//! non-TTY stdin/stdout) returns [`None`] so the caller surfaces the original
//! `NOT_FOUND` failure verbatim — no prompt is ever attempted and no script,
//! agent, or CI invocation changes behavior.

use std::collections::HashSet;
use std::sync::Arc;

use hamstik_api_client::{
    ClientError, HamstikApi, ListOptions, ListProjectsOptions, ListWorkItemsQuery,
};

use crate::error::CliError;
use crate::input::Prompt;

/// One page of candidates is enough for an interactive choice; the same bound
/// the shell-completion layer uses (CLI-32).
const CANDIDATE_PAGE_SIZE: u32 = 100;

/// A candidate reference the user can choose between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The canonical value used to resolve the reference (key or slug).
    pub value: String,
    /// The human-readable name/title shown alongside the value.
    pub label: String,
    /// Optional distinguishing detail (status, project, archived state).
    pub detail: String,
}

impl Candidate {
    /// Builds a candidate.
    #[must_use]
    pub fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            detail: detail.into(),
        }
    }

    /// The single-line label rendered in the picker.
    ///
    /// The key is always shown first so the choice maps back to an exact
    /// Public API reference; the name and detail follow only when they add
    /// information.
    #[must_use]
    pub fn display(&self) -> String {
        let mut rendered = self.value.clone();
        if !self.label.is_empty() && self.label != self.value {
            rendered.push_str(" — ");
            rendered.push_str(&self.label);
        }
        if !self.detail.is_empty() {
            rendered.push_str(" (");
            rendered.push_str(&self.detail);
            rendered.push(')');
        }
        rendered
    }
}

/// True when a client error is a `NOT_FOUND` the picker could disambiguate.
///
/// Transport, authentication, authorization, and server failures are never
/// treated as "maybe the user meant another resource"; only a clean 404 is.
#[must_use]
pub fn is_not_found(err: &ClientError) -> bool {
    matches!(err, ClientError::Api(api) if api.status == 404)
}

/// Returns the plausible candidates for `reference`, best match first.
///
/// Matching is case-insensitive over both the canonical value and the display
/// label: exact matches rank before prefix matches, which rank before
/// substring matches. The stable order preserves the server's listing order
/// within each rank so the picker is deterministic.
#[must_use]
pub fn plausible_matches(reference: &str, candidates: &[Candidate]) -> Vec<Candidate> {
    let needle = reference.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut exact = Vec::new();
    let mut prefix = Vec::new();
    let mut substring = Vec::new();
    for candidate in candidates {
        let value = candidate.value.to_lowercase();
        let label = candidate.label.to_lowercase();
        if value == needle || label == needle {
            exact.push(candidate.clone());
        } else if value.starts_with(&needle) || label.starts_with(&needle) {
            prefix.push(candidate.clone());
        } else if value.contains(&needle) || label.contains(&needle) {
            substring.push(candidate.clone());
        }
    }
    exact.into_iter().chain(prefix).chain(substring).collect()
}

/// Resolves an ambiguous reference from `candidates`, prompting when needed.
///
/// Returns `Ok(None)` when the reference is not plausible against any
/// candidate, or when prompting is not permitted (`interactive == false`), so
/// the caller can surface the original `NOT_FOUND` unchanged. Returns
/// `Ok(Some(value))` for the single best match or the user's explicit choice.
///
/// Cancelling the picker is a usage error: the command aborts without any
/// side effects and without falling back to a guessed candidate.
///
/// # Errors
/// Returns a usage error when the user cancels, or a general error when the
/// terminal prompt itself fails.
pub fn choose(
    prompt: &mut dyn Prompt,
    interactive: bool,
    what: &str,
    reference: &str,
    candidates: &[Candidate],
) -> Result<Option<String>, CliError> {
    if !interactive {
        return Ok(None);
    }
    let matches = plausible_matches(reference, candidates);
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0].value.clone())),
        _ => {
            let options: Vec<String> = matches.iter().map(Candidate::display).collect();
            let message =
                format!("Multiple {what} match {reference:?}; select one (Esc/Ctrl+C cancels):");
            let selected = prompt
                .select(&message, &options)
                .map_err(|err| CliError::general(format!("selection prompt failed: {err}")))?;
            let index = selected.ok_or_else(|| {
                CliError::usage(format!("{what} selection cancelled; no changes were made"))
            })?;
            matches
                .get(index)
                .map(|candidate| Some(candidate.value.clone()))
                .ok_or_else(|| {
                    CliError::general("selection prompt returned an invalid candidate index")
                })
        }
    }
}

/// Reads one bounded page of Organization candidates.
///
/// # Errors
/// Returns the mapped client error when the list request fails.
pub async fn organization_candidates(
    api: &Arc<dyn HamstikApi>,
) -> Result<Vec<Candidate>, CliError> {
    let response = api
        .list_organizations(ListOptions {
            limit: Some(CANDIDATE_PAGE_SIZE),
            cursor: None,
        })
        .await
        .map_err(CliError::from_client)?;
    Ok(response
        .value
        .items
        .into_iter()
        .map(|org| Candidate::new(org.slug, org.name, ""))
        .collect())
}

/// Reads one bounded page of Project candidates, archived and unarchived.
///
/// # Errors
/// Returns the mapped client error when either list request fails.
pub async fn project_candidates(
    api: &Arc<dyn HamstikApi>,
    org: &str,
) -> Result<Vec<Candidate>, CliError> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for archived in [false, true] {
        let response = api
            .list_projects(
                org,
                ListProjectsOptions {
                    limit: Some(CANDIDATE_PAGE_SIZE),
                    cursor: None,
                    archived: Some(archived),
                },
            )
            .await
            .map_err(CliError::from_client)?;
        for project in response.value.items {
            if seen.insert(project.key.clone()) {
                let detail = if project.archived_at.is_some() {
                    "archived"
                } else {
                    ""
                };
                candidates.push(Candidate::new(project.key, project.name, detail));
            }
        }
    }
    Ok(candidates)
}

/// Reads one bounded page of Work Item candidates for the resolved scope.
///
/// When `project` is `Some`, the query is scoped to that Project's Work Items;
/// otherwise the Organization Work collection is queried and each candidate
/// carries its Project key as detail.
///
/// # Errors
/// Returns the mapped client error when the list request fails.
pub async fn work_item_candidates(
    api: &Arc<dyn HamstikApi>,
    org: &str,
    project: Option<&str>,
) -> Result<Vec<Candidate>, CliError> {
    let query = ListWorkItemsQuery {
        limit: Some(CANDIDATE_PAGE_SIZE),
        ..Default::default()
    };
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    if let Some(project) = project {
        let response = api
            .list_work_items(org, project, query)
            .await
            .map_err(CliError::from_client)?;
        for item in response.value.items {
            if seen.insert(item.key.clone()) {
                candidates.push(Candidate::new(
                    item.key,
                    item.title.unwrap_or_default(),
                    item.status.unwrap_or_default(),
                ));
            }
        }
    } else {
        let response = api
            .list_organization_work_items(org, query)
            .await
            .map_err(CliError::from_client)?;
        for item in response.value.items {
            let summary = item.summary;
            if seen.insert(summary.key.clone()) {
                let status = summary.status.unwrap_or_default();
                let detail = if status.is_empty() {
                    item.project.key
                } else {
                    format!("{status} · {}", item.project.key)
                };
                candidates.push(Candidate::new(
                    summary.key,
                    summary.title.unwrap_or_default(),
                    detail,
                ));
            }
        }
    }
    Ok(candidates)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    struct ScriptedPrompt {
        choice: Option<usize>,
    }

    impl Prompt for ScriptedPrompt {
        fn read_line(&mut self, _prompt: &str) -> std::io::Result<String> {
            unimplemented!("disambiguation only selects")
        }
        fn read_secret(&mut self, _prompt: &str) -> std::io::Result<String> {
            unimplemented!("disambiguation only selects")
        }
        fn select(&mut self, _prompt: &str, options: &[String]) -> std::io::Result<Option<usize>> {
            assert!(!options.is_empty(), "the picker must offer candidates");
            Ok(self.choice)
        }
    }

    fn candidates() -> Vec<Candidate> {
        vec![
            Candidate::new("APP", "App Platform", "active"),
            Candidate::new("ACCT", "Accounting", "active"),
            Candidate::new("ABC", "Alphabet", "archived"),
        ]
    }

    #[test]
    fn exact_match_is_the_only_plausible_match() {
        let matches = plausible_matches("abc", &candidates());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].value, "ABC");
    }

    #[test]
    fn prefix_ranks_before_substring() {
        let ranked = vec![
            Candidate::new("HAM-1", "First", ""),
            Candidate::new("XHAM-2", "Second", ""),
            Candidate::new("WHAM-3", "Third", ""),
        ];
        let matches = plausible_matches("ham", &ranked);
        let values: Vec<&str> = matches.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(values, vec!["HAM-1", "XHAM-2", "WHAM-3"]);
    }

    #[test]
    fn label_matches_are_plausible() {
        let matches = plausible_matches("accounting", &candidates());
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].value, "ACCT");
    }

    #[test]
    fn no_plausible_match_returns_empty() {
        assert!(plausible_matches("zzz", &candidates()).is_empty());
        assert!(plausible_matches("   ", &candidates()).is_empty());
    }

    #[test]
    fn unique_match_resolves_without_prompting() {
        let mut prompt = ScriptedPrompt { choice: None };
        let resolved = choose(&mut prompt, true, "projects", "accounting", &candidates()).unwrap();
        assert_eq!(resolved.as_deref(), Some("ACCT"));
    }

    #[test]
    fn multiple_matches_prompt_and_use_explicit_choice() {
        // All three candidates match the prefix "a", in listing order.
        let mut prompt = ScriptedPrompt { choice: Some(1) };
        let resolved = choose(&mut prompt, true, "projects", "a", &candidates()).unwrap();
        assert_eq!(resolved.as_deref(), Some("ACCT"));
    }

    #[test]
    fn cancellation_is_a_usage_error_and_never_guesses() {
        let mut prompt = ScriptedPrompt { choice: None };
        let err = choose(&mut prompt, true, "projects", "a", &candidates()).unwrap_err();
        assert_eq!(err.kind, crate::error::ErrorKind::Usage);
        assert!(err.message.contains("cancelled"), "{}", err.message);
    }

    #[test]
    fn non_interactive_never_prompts() {
        // `choice` would panic if `select` were called (unimplemented choice
        // is `None`, which would be a cancellation); the interactive=false
        // gate must return `None` first.
        let mut prompt = ScriptedPrompt { choice: Some(0) };
        let resolved = choose(&mut prompt, false, "projects", "ac", &candidates()).unwrap();
        assert_eq!(resolved, None);
    }

    #[test]
    fn display_avoids_repeating_the_value() {
        assert_eq!(
            Candidate::new("APP", "APP", "").display(),
            "APP",
            "a label equal to the value is not repeated"
        );
        assert_eq!(
            Candidate::new("APP", "App Platform", "archived").display(),
            "APP — App Platform (archived)"
        );
    }

    #[test]
    fn not_found_detection_is_specific() {
        let not_found = ClientError::Api(hamstik_api_client::ApiError {
            status: 404,
            code: "NOT_FOUND".to_string(),
            message: "missing".to_string(),
            request_id: None,
            field_errors: std::collections::BTreeMap::new(),
            details: None,
            retry_after: None,
            rate_limit: None,
        });
        assert!(is_not_found(&not_found));
        let forbidden = ClientError::Api(hamstik_api_client::ApiError {
            status: 403,
            code: "FORBIDDEN".to_string(),
            message: "no".to_string(),
            request_id: None,
            field_errors: std::collections::BTreeMap::new(),
            details: None,
            retry_after: None,
            rate_limit: None,
        });
        assert!(!is_not_found(&forbidden));
    }
}
