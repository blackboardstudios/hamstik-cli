// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Project context (`.hamstik.toml`) and cross-source precedence resolution.
//!
//! Resolution is pure (SPEC §34) so it can be unit-tested without touching the
//! filesystem; discovery/loading/writing are thin IO wrappers around it.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{ConfigFile, Profile};
use crate::environment::Environment;
use crate::error::CliError;

/// Current context schema version.
pub const CONTEXT_VERSION: u32 = 1;

/// Maximum accepted `.hamstik.toml` size (64 KiB).
pub const MAX_CONTEXT_BYTES: u64 = 64 * 1024;

/// The name of the per-directory context file.
pub const CONTEXT_FILENAME: &str = ".hamstik.toml";

/// Where a resolved value came from (for `context show --explain`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A `--flag` on this invocation.
    Cli,
    /// An environment variable.
    Env,
    /// A `.hamstik.toml` value.
    ContextFile,
    /// The active profile's stored default.
    Profile,
    /// Nothing set anywhere; the built-in default applies.
    Default,
}

impl Source {
    /// Human-readable source label (parenthesized annotations).
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Source::Cli => "--flag",
            Source::Env => "environment",
            Source::ContextFile => ".hamstik.toml",
            Source::Profile => "global config",
            Source::Default => "default",
        }
    }

    /// Stable machine-readable identifier (JSON output).
    #[must_use]
    pub fn as_key(self) -> &'static str {
        match self {
            Source::Cli => "cli",
            Source::Env => "environment",
            Source::ContextFile => "context_file",
            Source::Profile => "profile",
            Source::Default => "default",
        }
    }
}

/// A resolved value together with its winning source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedField {
    /// The resolved value, when any source provided one.
    pub value: Option<String>,
    /// The source that won precedence.
    pub source: Source,
}

/// A resolved value plus the complete precedence chain: the winning source
/// and every lower-precedence source that offered a value but was shadowed.
///
/// The chain is computed by the same `pick` logic that selects the winner
/// (`resolve`/`resolve_inner`), so it can never disagree with production
/// resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChain {
    /// The winning value/source pair, when any source provided one.
    pub winner: Option<(String, Source)>,
    /// Lower-precedence sources that also offered a value, in precedence
    /// order (nearest first).
    pub shadowed: Vec<(String, Source)>,
}

impl ResolvedChain {
    /// The winning source, or [`Source::Default`] when nothing was set.
    #[must_use]
    pub fn source(&self) -> Source {
        self.winner
            .as_ref()
            .map(|(_, source)| *source)
            .unwrap_or(Source::Default)
    }

    /// The winning value, when any source provided one.
    #[must_use]
    pub fn value(&self) -> Option<&str> {
        self.winner.as_ref().map(|(value, _)| value.as_str())
    }
}

/// The `.hamstik.toml` document.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextFile {
    /// On-disk schema version.
    pub version: u32,
    /// Host origin for work done in this directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// Default organization slug for this directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    /// Default project key for this directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Project keys included by `work dashboard` when `--project` is omitted.
    /// An empty or absent list falls back to the resolved single Project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_projects: Option<Vec<String>>,
}

/// The full resolved selection for a command invocation.
#[derive(Debug, Clone)]
pub struct Resolution {
    /// The effective host (resolved value or the built-in default).
    pub host: String,
    /// Where the host value came from.
    pub host_source: Source,
    /// The selected profile name (attached by the caller).
    pub profile: Option<String>,
    /// The effective organization slug and its source.
    pub organization: ResolvedField,
    /// The effective project key and its source.
    pub project: ResolvedField,
}

/// Selects the active profile name (SPEC §28).
pub fn select_profile(
    config: &ConfigFile,
    cli_profile: Option<&str>,
    env: &dyn Environment,
) -> Result<Option<String>, CliError> {
    let requested = cli_profile
        .map(str::to_string)
        .or_else(|| env.var("HAMSTIK_PROFILE"))
        .or_else(|| config.active_profile.clone());
    match requested {
        Some(name) => {
            // Accept names already present in the config even if they predate
            // stricter validation (e.g. legacy hosts that included a port).
            if config.profiles.contains_key(&name) {
                return Ok(Some(name));
            }
            crate::config::validate_profile_name(&name)?;
            Err(CliError::config(format!("no such profile: {name}")))
        }
        None => Ok(None),
    }
}

/// Resolves host/organization/project by precedence (SPEC §34).
#[must_use]
pub fn resolve(
    cli: (&Option<String>, &Option<String>, &Option<String>),
    env: &dyn Environment,
    context: Option<&ContextFile>,
    profile: Option<&Profile>,
) -> Resolution {
    let (cli_host, cli_org, cli_project) = cli;
    let mut resolution = resolve_inner(
        cli_host.as_deref(),
        cli_org.as_deref(),
        cli_project.as_deref(),
        env,
        context,
        profile,
    );
    // The profile name is attached by the caller once it is known.
    resolution.profile = None;
    resolution
}

/// Computes the full precedence chains behind a [`Resolution`] using the
/// identical candidate-order logic as `resolve`, so the diagnostic view can
/// never disagree with production resolution.
///
/// The host chain includes the built-in default host as the final fallback.
#[must_use]
pub fn explain_chains(
    cli: (&Option<String>, &Option<String>, &Option<String>),
    env: &dyn Environment,
    context: Option<&ContextFile>,
    profile: Option<&Profile>,
) -> (ResolvedChain, ResolvedChain, ResolvedChain) {
    let (cli_host, cli_org, cli_project) = cli;
    let mut host_chain = chain_string(
        cli_host.as_deref().map(|v| (v, Source::Cli)),
        env.var("HAMSTIK_HOST").map(|v| (Some(v), Source::Env)),
        context
            .and_then(|c| c.host.as_deref())
            .map(|v| (v, Source::ContextFile)),
        profile.map(|p| (p.host.as_str(), Source::Profile)),
    );
    if host_chain.winner.is_none() {
        host_chain.winner = Some((
            hamstik_api_client::host::DEFAULT_HOST.to_string(),
            Source::Default,
        ));
    }
    let organization_chain = chain_string(
        cli_org.as_deref().map(|v| (v, Source::Cli)),
        env.var("HAMSTIK_ORG").map(|v| (Some(v), Source::Env)),
        context
            .and_then(|c| c.organization.as_deref())
            .map(|v| (v, Source::ContextFile)),
        profile
            .and_then(|p| p.default_organization.as_deref())
            .map(|v| (v, Source::Profile)),
    );
    let project_chain = chain_string(
        cli_project.as_deref().map(|v| (v, Source::Cli)),
        env.var("HAMSTIK_PROJECT").map(|v| (Some(v), Source::Env)),
        context
            .and_then(|c| c.project.as_deref())
            .map(|v| (v, Source::ContextFile)),
        profile
            .and_then(|p| p.default_project.as_deref())
            .map(|v| (v, Source::Profile)),
    );
    (host_chain, organization_chain, project_chain)
}

fn resolve_inner(
    cli_host: Option<&str>,
    cli_org: Option<&str>,
    cli_project: Option<&str>,
    env: &dyn Environment,
    context: Option<&ContextFile>,
    profile: Option<&Profile>,
) -> Resolution {
    let host = pick_string(
        cli_host.map(|v| (v, Source::Cli)),
        env.var("HAMSTIK_HOST").map(|v| (v, Source::Env)),
        context
            .and_then(|c| c.host.as_deref())
            .map(|v| (v, Source::ContextFile)),
        profile.map(|p| (p.host.as_str(), Source::Profile)),
    );
    let host_value = host
        .value
        .clone()
        .unwrap_or_else(|| hamstik_api_client::host::DEFAULT_HOST.to_string());
    let host_source = if host.value.is_some() {
        host.source
    } else {
        Source::Default
    };

    let organization = pick_string(
        cli_org.map(|v| (v, Source::Cli)),
        env.var("HAMSTIK_ORG").map(|v| (v, Source::Env)),
        context
            .and_then(|c| c.organization.as_deref())
            .map(|v| (v, Source::ContextFile)),
        profile
            .and_then(|p| p.default_organization.as_deref())
            .map(|v| (v, Source::Profile)),
    );
    let project = pick_string(
        cli_project.map(|v| (v, Source::Cli)),
        env.var("HAMSTIK_PROJECT").map(|v| (v, Source::Env)),
        context
            .and_then(|c| c.project.as_deref())
            .map(|v| (v, Source::ContextFile)),
        profile
            .and_then(|p| p.default_project.as_deref())
            .map(|v| (v, Source::Profile)),
    );

    Resolution {
        host: host_value,
        host_source,
        profile: None,
        organization,
        project,
    }
}

fn pick_string(
    cli: Option<(&str, Source)>,
    env: Option<(String, Source)>,
    context: Option<(&str, Source)>,
    profile: Option<(&str, Source)>,
) -> ResolvedField {
    chain_string(
        cli,
        env.map(|(value, source)| (Some(value), source)),
        context,
        profile,
    )
    .into()
}

/// Computes the full precedence chain for one string setting.
///
/// `env` already carries its value (`Option<Option<String>>` is avoided by
/// letting the caller pass `None` when the variable is absent).
fn chain_string(
    cli: Option<(&str, Source)>,
    env: Option<(Option<String>, Source)>,
    context: Option<(&str, Source)>,
    profile: Option<(&str, Source)>,
) -> ResolvedChain {
    let candidates: [(Option<String>, Source); 4] = [
        (cli.map(|(v, _)| v.to_string()), Source::Cli),
        (env.and_then(|(value, _)| value), Source::Env),
        (context.map(|(v, _)| v.to_string()), Source::ContextFile),
        (profile.map(|(v, _)| v.to_string()), Source::Profile),
    ];
    let mut winner = None;
    let mut shadowed = Vec::new();
    for (value, source) in candidates {
        if let Some(value) = value {
            if winner.is_none() {
                winner = Some((value, source));
            } else {
                shadowed.push((value, source));
            }
        }
    }
    ResolvedChain { winner, shadowed }
}

impl From<ResolvedChain> for ResolvedField {
    fn from(chain: ResolvedChain) -> Self {
        let source = chain.source();
        Self {
            value: chain.winner.map(|(value, _)| value),
            source,
        }
    }
}

/// Builds a non-blocking "configuration drift" advisory when the resolved
/// Organization is absent from the authenticated user's membership list.
///
/// `member_slugs` is the `organizations[].slug` list from `GET /me`, which is
/// the only membership data the Public API exposes. The check never changes
/// context and never fails a command; it returns `None` when there is nothing
/// to report:
///
/// - no Organization is resolved (nothing to compare);
/// - the membership list is empty, which the callers treat as "membership data
///   unavailable" (offline, a token without membership scope, or an account
///   with no Organizations);
/// - the resolved Organization is a member.
///
/// Keeping the decision pure (SPEC §34 pattern) lets it be unit-tested without
/// a session or a network call; the command layer only prints the result.
#[must_use]
pub fn membership_drift_warning(
    organization: &ResolvedField,
    member_slugs: &[String],
) -> Option<String> {
    let configured = organization.value.as_deref()?;
    if member_slugs.is_empty() {
        return None;
    }
    if member_slugs.iter().any(|slug| slug == configured) {
        return None;
    }
    let mut available: Vec<&str> = member_slugs.iter().map(String::as_str).collect();
    available.sort_unstable();
    Some(format!(
        "warning: configuration drift: resolved organization {configured:?} ({}) is not among \
         the authenticated user's memberships [{}]; the command continued and the context was \
         not changed",
        organization.source.label(),
        available.join(", ")
    ))
}

/// Searches upward from `start` for the nearest `.hamstik.toml`.
///
/// Discovery is purely additive: the directory walk-up runs first and nearest
/// still wins. When the walk-up finds nothing but `start` is inside a git
/// checkout, the primary checkout behind a linked worktree is searched as
/// well: git's `--git-common-dir` names the primary `.git` directory, whose
/// parent is the main checkout root. The current position inside the worktree
/// is mapped onto the same relative path in the main checkout, then that path
/// and its ancestors (up to the main root) are probed. This lets a linked
/// worktree inherit the `.hamstik.toml` of the primary checkout without copying
/// the file, including one Project per monorepo subtree.
#[must_use]
pub fn discover(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    let mut saw_git = false;
    while let Some(dir) = current {
        let candidate = dir.join(CONTEXT_FILENAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        if dir.join(".git").exists() {
            saw_git = true;
        }
        current = dir.parent();
    }
    if saw_git {
        return discover_through_git_worktree(start);
    }
    None
}

/// Fallback discovery for a linked git worktree (SPEC §33).
///
/// Returns `None` when git is unavailable, `start` is not in a checkout, or
/// the checkout is not a linked worktree (the primary worktree was already
/// covered by the walk-up). The primary checkout root is only trusted when the
/// common git directory is actually named `.git`, which rules out bare repos
/// and submodule `modules/<name>` directories whose parent is not a checkout.
fn discover_through_git_worktree(start: &Path) -> Option<PathBuf> {
    let current_root = git_output(start, &["rev-parse", "--show-toplevel"])?;
    let common = git_output(start, &["rev-parse", "--git-common-dir"])?;
    let common = if common.is_absolute() {
        common
    } else {
        start.join(common)
    };
    // Canonicalize so the worktree/main comparison and the relative mapping
    // are separator- and symlink-independent (notably on Windows, where git
    // may print forward slashes while `canonicalize` uses backslashes).
    let start = fs::canonicalize(start).ok()?;
    let current_root = fs::canonicalize(current_root).ok()?;
    let common = fs::canonicalize(common).ok()?;
    if common.file_name() != Some(std::ffi::OsStr::new(".git")) {
        return None;
    }
    let main_root = common.parent()?;
    if main_root == current_root.as_path() {
        // Primary worktree: the walk-up already searched it.
        return None;
    }
    // Map the current position onto the primary checkout. `strip_prefix`
    // failing (the cwd is not under the reported toplevel, for example)
    // degrades to probing the main root.
    let relative = start.strip_prefix(&current_root).unwrap_or(Path::new(""));
    let mut probe = main_root.join(relative);
    loop {
        let candidate = probe.join(CONTEXT_FILENAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        if main_root == probe.as_path() {
            return None;
        }
        probe = probe.parent()?.to_path_buf();
    }
}

/// Runs `git -C <start> <args>` and returns its trimmed stdout on success.
fn git_output(start: &Path, args: &[&str]) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(start)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(PathBuf::from(trimmed))
}

/// Loads and validates a context file.
///
/// Failures always name the file: `.hamstik.toml` is discovered by walking up
/// from the working directory, so the user cannot guess which one broke.
pub fn load(path: &Path) -> Result<ContextFile, CliError> {
    let metadata =
        fs::metadata(path).map_err(|err| fail(path, &format!("cannot read file ({err})")))?;
    if metadata.len() > MAX_CONTEXT_BYTES {
        return Err(fail(path, "file is too large (exceeds the 64 KiB limit)"));
    }
    let contents =
        fs::read_to_string(path).map_err(|err| fail(path, &format!("cannot read file ({err})")))?;
    let context: ContextFile = toml::from_str(&contents).map_err(|err| {
        fail(
            path,
            &format!(
                "invalid context file (repair the file, or upgrade this CLI if it was written \
                 by a newer version): {err}"
            ),
        )
    })?;
    if context.version != CONTEXT_VERSION {
        return Err(fail(
            path,
            &format!(
                "schema version {} is not supported (this CLI uses version {CONTEXT_VERSION})",
                context.version
            ),
        ));
    }
    Ok(context)
}

fn fail(path: &Path, reason: &str) -> CliError {
    CliError::config(format!(
        "{}: {reason}\nhint: repair or remove this file to recover; it holds only non-secret working context",
        path.display()
    ))
}

/// Writes a context file to disk.
///
/// The write is atomic and durable (unique temp file, synced, renamed) so a
/// crash or concurrent invocation can never leave a torn `.hamstik.toml`, and
/// a pre-planted symlink cannot redirect it. Permissions are restricted to the
/// owner.
pub fn save(path: &Path, context: &ContextFile) -> Result<(), CliError> {
    if context.version == 0 {
        return Err(CliError::config("internal error: context version not set"));
    }
    let serialized = toml::to_string_pretty(context)
        .map_err(|err| CliError::config(format!("cannot serialize context: {err}")))?;
    crate::fsutil::write_atomic(path, serialized.as_bytes())
        .and_then(|()| crate::fsutil::restrict_permissions(path))
        .map_err(|err| CliError::config(format!("cannot write context file: {err}")))?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::environment::MapEnvironment;

    fn profile() -> Profile {
        Profile {
            host: "https://profile.host".to_string(),
            user_id: "u".to_string(),
            email: "u@profile.host".to_string(),
            default_organization: Some("profile-org".to_string()),
            default_project: Some("PRO".to_string()),
        }
    }

    #[test]
    fn cli_overrides_everything() {
        let env = MapEnvironment::new()
            .with_var("HAMSTIK_HOST", "https://env.host")
            .with_var("HAMSTIK_ORG", "env-org")
            .with_var("HAMSTIK_PROJECT", "ENVP");
        let context = ContextFile {
            version: 1,
            host: Some("https://ctx.host".into()),
            organization: Some("ctx-org".into()),
            project: Some("CTX".into()),
            dashboard_projects: None,
        };
        let resolution = resolve(
            (
                &Some("https://cli.host".to_string()),
                &Some("cli-org".to_string()),
                &Some("CLI".to_string()),
            ),
            &env,
            Some(&context),
            Some(&profile()),
        );
        assert_eq!(resolution.host, "https://cli.host");
        assert_eq!(resolution.host_source, Source::Cli);
        assert_eq!(resolution.organization.value.as_deref(), Some("cli-org"));
        assert_eq!(resolution.project.value.as_deref(), Some("CLI"));
    }

    #[test]
    fn falls_back_through_env_context_profile() {
        let env = MapEnvironment::new();
        let context = ContextFile {
            version: 1,
            host: None,
            organization: Some("ctx-org".into()),
            project: None,
            dashboard_projects: None,
        };
        let resolution = resolve(
            (&None, &None, &None),
            &env,
            Some(&context),
            Some(&profile()),
        );
        assert_eq!(resolution.host, "https://profile.host");
        assert_eq!(resolution.host_source, Source::Profile);
        assert_eq!(resolution.organization.value.as_deref(), Some("ctx-org"));
        assert_eq!(resolution.organization.source, Source::ContextFile);
        assert_eq!(resolution.project.value.as_deref(), Some("PRO"));
        assert_eq!(resolution.project.source, Source::Profile);
    }

    #[test]
    fn default_host_when_nothing_set() {
        let resolution = resolve((&None, &None, &None), &MapEnvironment::new(), None, None);
        assert_eq!(resolution.host, "https://hamstik.com");
        assert_eq!(resolution.host_source, Source::Default);
        assert_eq!(resolution.organization.value, None);
    }

    #[test]
    fn env_wins_over_context() {
        let env = MapEnvironment::new().with_var("HAMSTIK_ORG", "env-org");
        let context = ContextFile {
            version: 1,
            host: None,
            organization: Some("ctx-org".into()),
            project: None,
            dashboard_projects: None,
        };
        let resolution = resolve((&None, &None, &None), &env, Some(&context), None);
        assert_eq!(resolution.organization.value.as_deref(), Some("env-org"));
        assert_eq!(resolution.organization.source, Source::Env);
    }

    #[test]
    fn drift_warning_names_configured_and_actual_values() {
        let organization = ResolvedField {
            value: Some("ctx-org".to_string()),
            source: Source::ContextFile,
        };
        let memberships = vec!["acme".to_string(), "beta".to_string()];
        let warning = membership_drift_warning(&organization, &memberships).unwrap();
        assert!(warning.contains("ctx-org"), "{warning}");
        assert!(warning.contains("acme"), "{warning}");
        assert!(warning.contains("beta"), "{warning}");
        assert!(warning.contains(".hamstik.toml"), "{warning}");
    }

    #[test]
    fn drift_warning_silent_when_organization_matches() {
        let organization = ResolvedField {
            value: Some("acme".to_string()),
            source: Source::ContextFile,
        };
        let memberships = vec!["acme".to_string(), "beta".to_string()];
        assert!(membership_drift_warning(&organization, &memberships).is_none());
    }

    #[test]
    fn drift_warning_silent_without_membership_data_or_organization() {
        let unset = ResolvedField {
            value: None,
            source: Source::Default,
        };
        let configured = ResolvedField {
            value: Some("ctx-org".to_string()),
            source: Source::ContextFile,
        };
        assert!(membership_drift_warning(&unset, &["acme".to_string()]).is_none());
        assert!(membership_drift_warning(&configured, &[]).is_none());
    }

    #[test]
    fn discovers_nearest_context() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let nested = root.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(CONTEXT_FILENAME), "version = 1\n").unwrap();
        fs::write(root.join(CONTEXT_FILENAME), "version = 1\n").unwrap();
        let found = discover(&nested).unwrap();
        assert_eq!(found, nested.join(CONTEXT_FILENAME));
    }

    /// Runs git in `dir`, asserting success. Commit identity is supplied via
    /// the environment so the developer's global git config is not required.
    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "hamstik-test")
            .env("GIT_AUTHOR_EMAIL", "hamstik-test@example.com")
            .env("GIT_COMMITTER_NAME", "hamstik-test")
            .env("GIT_COMMITTER_EMAIL", "hamstik-test@example.com")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    /// Creates a primary checkout plus one linked worktree and returns their
    /// paths. The primary checkout gets a commit so `worktree add` can resolve
    /// `HEAD`.
    fn git_worktree_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let linked = dir.path().join("linked");
        fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q"]);
        fs::write(main.join("tracked.txt"), "x\n").unwrap();
        git(&main, &["add", "tracked.txt"]);
        git(&main, &["commit", "-qm", "init"]);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                linked.to_str().unwrap(),
                "-b",
                "feature",
            ],
        );
        (dir, main, linked)
    }

    #[test]
    fn discovers_context_through_linked_worktree() {
        let (_dir, main, linked) = git_worktree_fixture();
        // The config exists only in the primary checkout, untracked, so it is
        // absent from the linked worktree and the walk-up cannot see it.
        fs::write(
            main.join(CONTEXT_FILENAME),
            "version = 1\norganization = \"main-org\"\n",
        )
        .unwrap();
        let nested = linked.join("sub").join("dir");
        fs::create_dir_all(&nested).unwrap();
        let found = discover(&nested).unwrap();
        let expected = fs::canonicalize(&main).unwrap().join(CONTEXT_FILENAME);
        assert_eq!(found, expected);
    }

    #[test]
    fn worktree_context_wins_over_main_checkout() {
        let (_dir, main, linked) = git_worktree_fixture();
        fs::write(
            main.join(CONTEXT_FILENAME),
            "version = 1\norganization = \"main-org\"\n",
        )
        .unwrap();
        fs::write(
            linked.join(CONTEXT_FILENAME),
            "version = 1\norganization = \"worktree-org\"\n",
        )
        .unwrap();
        let nested = linked.join("sub");
        fs::create_dir_all(&nested).unwrap();
        let found = discover(&nested).unwrap();
        assert_eq!(found, linked.join(CONTEXT_FILENAME));
    }

    #[test]
    fn context_roundtrip_and_rejects_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONTEXT_FILENAME);
        let context = ContextFile {
            version: 1,
            host: Some("https://h".into()),
            organization: Some("org".into()),
            project: None,
            dashboard_projects: Some(vec!["HAM".into(), "WEB".into()]),
        };
        save(&path, &context).unwrap();
        assert_eq!(load(&path).unwrap(), context);
        fs::write(&path, "version = 1\nbogus = 1\n").unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn select_profile_accepts_existing_legacy_name() {
        let mut config = ConfigFile::default();
        config
            .profiles
            .insert("localhost:3000-test".to_string(), profile());
        config.active_profile = Some("localhost:3000-test".to_string());
        let selected = select_profile(&config, None, &MapEnvironment::new()).unwrap();
        assert_eq!(selected.as_deref(), Some("localhost:3000-test"));
    }

    #[test]
    fn select_profile_rejects_invalid_missing_name() {
        let config = ConfigFile::default();
        let err = select_profile(&config, Some("bad name"), &MapEnvironment::new()).unwrap_err();
        assert!(err.message.contains("invalid profile name"));
    }
}
