// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Application wiring: dependency seams, session assembly, and the run loop.
//!
//! Everything the commands need (environment, credential store, config store,
//! API factory, prompts, output writers, working directory) is injected through
//! [`Services`] so the whole CLI can be exercised in tests with fakes. The real
//! process entrypoint in `main.rs` assembles the production services.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use secrecy::SecretString;

use hamstik_api_client::{ClientConfig, HamstikApi, HamstikClient, Host};

use crate::args::{Cli, GlobalOptions};
use crate::commands;
use crate::config::{ConfigStore, Profile};
use crate::context::{self, ResolvedField, Source};
use crate::credentials::{self, CredentialError, CredentialStore};
use crate::environment::Environment;
use crate::error::CliError;
use crate::exit::SUCCESS;
use crate::input::Prompt;
use crate::output::{Mode, Output, OutputOptions};

/// Builds API clients behind a seam so tests can inject fakes.
pub trait ApiFactory: Send + Sync {
    /// Builds one client for the resolved request.
    fn build(&self, request: &ClientRequest) -> Result<Arc<dyn HamstikApi>, CliError>;

    /// Builds an unauthenticated client for public contract metadata.
    fn build_public(&self, request: &PublicClientRequest) -> Result<Arc<dyn HamstikApi>, CliError>;
}

/// The largest CA bundle accepted (2 MiB) — a handful of PEM certificates plus
/// slack; anything larger is not a trust store.
const MAX_CA_BUNDLE_BYTES: usize = 2 * 1024 * 1024;

/// Reads a PEM CA bundle, capped at [`MAX_CA_BUNDLE_BYTES`].
fn read_ca_bundle(path: &Path) -> Result<String, CliError> {
    let metadata = std::fs::metadata(path)
        .map_err(|err| CliError::config(format!("cannot read CA bundle: {err}")))?;
    if metadata.len() > MAX_CA_BUNDLE_BYTES as u64 {
        return Err(CliError::config(format!(
            "CA bundle at {} is too large (exceeds the {} byte limit)",
            path.display(),
            MAX_CA_BUNDLE_BYTES
        )));
    }
    let pem = std::fs::read_to_string(path)
        .map_err(|err| CliError::config(format!("cannot read CA bundle: {err}")))?;
    if !pem.contains("-----BEGIN CERTIFICATE-----") {
        return Err(CliError::config(format!(
            "CA bundle at {} contains no PEM certificates",
            path.display()
        )));
    }
    Ok(pem)
}

/// Parameters for constructing one API client.
pub struct ClientRequest<'a> {
    /// The validated host origin.
    pub host: Host,
    /// The bearer token for the request.
    pub token: SecretString,
    /// Disable automatic retries (`--no-retry`).
    pub no_retry: bool,
    /// Additional PEM root certificate bundle path.
    pub ca_bundle: Option<&'a Path>,
    /// The `User-Agent` value.
    pub user_agent: String,
    /// Optional total request timeout override for interactive operations.
    pub request_timeout: Option<Duration>,
    /// Whether successful rate-limit exhaustion may delay the next request.
    pub wait_on_depleted_rate_limit: bool,
}

/// Parameters for constructing an unauthenticated Public API client.
pub struct PublicClientRequest<'a> {
    /// The validated host origin.
    pub host: Host,
    /// Disable automatic retries (`--no-retry`).
    pub no_retry: bool,
    /// Additional PEM root certificate bundle path.
    pub ca_bundle: Option<&'a Path>,
    /// The `User-Agent` value.
    pub user_agent: String,
}

/// Production factory producing real [`HamstikClient`] instances.
pub struct ProductionApiFactory;

impl ApiFactory for ProductionApiFactory {
    fn build(&self, request: &ClientRequest) -> Result<Arc<dyn HamstikApi>, CliError> {
        let config = client_config(
            request.user_agent.clone(),
            request.no_retry,
            request.ca_bundle,
            request.request_timeout,
            request.wait_on_depleted_rate_limit,
        )?;
        let client = HamstikClient::new(request.host.clone(), request.token.clone(), config)
            .map_err(CliError::from_client)?;
        Ok(Arc::new(client))
    }

    fn build_public(&self, request: &PublicClientRequest) -> Result<Arc<dyn HamstikApi>, CliError> {
        let config = client_config(
            request.user_agent.clone(),
            request.no_retry,
            request.ca_bundle,
            None,
            true,
        )?;
        // Keep the same concrete type and configuration as authenticated
        // clients; only the bearer credential is absent.
        let client = HamstikClient::new_public(request.host.clone(), config)
            .map_err(CliError::from_client)?;
        Ok(Arc::new(client))
    }
}

fn client_config(
    user_agent: String,
    no_retry: bool,
    ca_bundle: Option<&Path>,
    request_timeout: Option<Duration>,
    wait_on_depleted_rate_limit: bool,
) -> Result<ClientConfig, CliError> {
    let mut config = ClientConfig {
        user_agent,
        wait_on_depleted_rate_limit,
        ..ClientConfig::default()
    };
    if no_retry {
        config.retry = hamstik_api_client::RetryPolicy::none();
    }
    if let Some(timeout) = request_timeout {
        config.request_timeout = timeout;
    }
    if let Some(path) = ca_bundle {
        config.ca_pem.push(read_ca_bundle(path)?);
    }
    Ok(config)
}

/// External dependencies injected into [`run`].
pub struct Services<'a> {
    /// Environment variable lookups.
    pub env: &'a dyn Environment,
    /// OS credential store.
    pub store: &'a dyn CredentialStore,
    /// Global configuration file access.
    pub config: ConfigStore,
    /// API client construction.
    pub factory: &'a dyn ApiFactory,
    /// Interactive prompts.
    pub prompt: &'a mut dyn Prompt,
    /// The process working directory (context discovery root).
    pub cwd: PathBuf,
    /// Success output stream.
    pub stdout: Box<dyn Write>,
    /// Diagnostic output stream.
    pub stderr: Box<dyn Write>,
}

/// The resolved selection for a single command invocation.
pub struct Selection {
    /// The effective host origin.
    pub host: Host,
    /// Where the host value came from.
    pub host_source: Source,
    /// The active profile name, when one is selected.
    pub profile: Option<String>,
    /// The active profile's stored metadata, when configured.
    pub profile_meta: Option<Profile>,
    /// The effective organization slug and its source.
    pub organization: ResolvedField,
    /// The effective project key and its source.
    pub project: ResolvedField,
    /// The `.hamstik.toml` in effect, when one was discovered.
    pub context_path: Option<PathBuf>,
    /// True when the token comes from `HAMSTIK_TOKEN` (never persisted).
    pub ephemeral_token: bool,
}

/// A command execution context bundling services with resolved state.
pub struct Session<'a> {
    /// Output renderer.
    pub out: Output,
    /// Parsed global options.
    pub global: GlobalOptions,
    /// Environment variable lookups.
    pub env: &'a dyn Environment,
    /// OS credential store.
    pub store: &'a dyn CredentialStore,
    /// Global configuration file access.
    pub config: ConfigStore,
    /// API client construction.
    pub factory: &'a dyn ApiFactory,
    /// Interactive prompts.
    pub prompt: &'a mut dyn Prompt,
    /// The process working directory (context discovery root).
    pub cwd: PathBuf,
    /// Exit code used when a command succeeds but wants a non-zero status
    /// (e.g. `doctor` reporting a failed check).
    pub exit_code: i32,
}

impl Session<'_> {
    /// Loads config, selects the profile, resolves context, and parses the host.
    pub fn selection(&self) -> Result<Selection, CliError> {
        let config = self.config.load()?;
        let profile = context::select_profile(&config, self.global.profile.as_deref(), self.env)?;
        let profile_meta = profile
            .as_ref()
            .and_then(|name| config.profiles.get(name))
            .cloned();

        let context_path = context::discover(&self.cwd);
        let context_file = match &context_path {
            Some(path) => Some(context::load(path)?),
            None => None,
        };

        let resolution = context::resolve(
            (&self.global.host, &self.global.org, &self.global.project),
            self.env,
            context_file.as_ref(),
            profile_meta.as_ref(),
        );

        let host = Host::parse(&resolution.host)
            .map_err(|err| CliError::config(format!("invalid host: {err}")))?;

        Ok(Selection {
            host,
            host_source: resolution.host_source,
            profile,
            profile_meta,
            organization: resolution.organization,
            project: resolution.project,
            context_path,
            ephemeral_token: self.env.var("HAMSTIK_TOKEN").is_some(),
        })
    }

    /// Returns the resolved organization slug or a usage error.
    pub fn require_org(&self, selection: &Selection) -> Result<String, CliError> {
        selection.organization.value.clone().ok_or_else(|| {
            CliError::usage(
                "no organization selected; pass --org, set HAMSTIK_ORG, or run `hamstik org use <slug>`",
            )
        })
    }

    /// Returns the resolved project key or a usage error.
    pub fn require_project(&self, selection: &Selection) -> Result<String, CliError> {
        selection.project.value.clone().ok_or_else(|| {
            CliError::usage(
                "no project selected; pass --project, set HAMSTIK_PROJECT, or run `hamstik project use <key>`",
            )
        })
    }

    /// Resolves the authentication token (SPEC §25, §34).
    pub fn token_for(&self, selection: &Selection) -> Result<SecretString, CliError> {
        if let Some(token) = self.env.var("HAMSTIK_TOKEN") {
            return crate::input::token_to_secret(&token).map_err(CliError::usage);
        }
        let profile = selection.profile_meta.as_ref().ok_or_else(|| {
            CliError::usage("not authenticated; run `hamstik auth login` or set HAMSTIK_TOKEN")
        })?;
        // The profile's credential was stored under the host recorded at login
        // time. A `--host` override re-addresses requests but must not hide the
        // profile's stored credential, so try the overridden host first and
        // fall back to the profile host. Credentials for the same user on a
        // genuinely different host stay isolated (SPEC §24).
        let lookup_hosts = if selection.host.as_str() == profile.host.as_str() {
            vec![selection.host.as_str()]
        } else {
            vec![selection.host.as_str(), profile.host.as_str()]
        };
        for host in &lookup_hosts {
            let account = credentials::account_key(host, &profile.user_id);
            if let Some(secret) = self.store.get(&account).map_err(map_credential_error)? {
                return Ok(secret);
            }
        }
        let host_note = if lookup_hosts.len() > 1 {
            format!(
                " (credential lookup tried both --host {} and profile host {})",
                selection.host.as_str(),
                profile.host.as_str()
            )
        } else {
            String::new()
        };
        Err(CliError::usage(format!(
            "no stored credential for profile {:?}{}; run `hamstik auth login`",
            selection.profile.as_deref().unwrap_or_default(),
            host_note
        )))
    }

    /// Builds an API client for the resolved selection.
    pub fn api(&self, selection: &Selection) -> Result<Arc<dyn HamstikApi>, CliError> {
        let token = self.token_for(selection)?;
        self.build_client(selection.host.clone(), token)
    }

    /// Builds the bounded, no-retry client used by dynamic completion.
    pub fn completion_api(
        &self,
        selection: &Selection,
        request_timeout: Duration,
    ) -> Result<Arc<dyn HamstikApi>, CliError> {
        let token = self.token_for(selection)?;
        self.build_client_with_options(
            selection.host.clone(),
            token,
            true,
            Some(request_timeout),
            false,
        )
    }

    /// Builds an unauthenticated client for the public OpenAPI operation.
    pub fn public_api(&self, selection: &Selection) -> Result<Arc<dyn HamstikApi>, CliError> {
        let env_ca_bundle = self
            .env
            .var("HAMSTIK_CA_BUNDLE")
            .map(std::path::PathBuf::from);
        let ca_bundle = self
            .global
            .ca_bundle
            .as_deref()
            .or(env_ca_bundle.as_deref());
        self.factory.build_public(&PublicClientRequest {
            host: selection.host.clone(),
            no_retry: self.global.no_retry,
            ca_bundle,
            user_agent: user_agent(),
        })
    }

    /// Builds an API client with an explicit token (used by `auth login`).
    pub fn build_client(
        &self,
        host: Host,
        token: SecretString,
    ) -> Result<Arc<dyn HamstikApi>, CliError> {
        self.build_client_with_options(host, token, self.global.no_retry, None, true)
    }

    fn build_client_with_options(
        &self,
        host: Host,
        token: SecretString,
        no_retry: bool,
        request_timeout: Option<Duration>,
        wait_on_depleted_rate_limit: bool,
    ) -> Result<Arc<dyn HamstikApi>, CliError> {
        let env_ca_bundle = self
            .env
            .var("HAMSTIK_CA_BUNDLE")
            .map(std::path::PathBuf::from);
        let ca_bundle = self
            .global
            .ca_bundle
            .as_deref()
            .or(env_ca_bundle.as_deref());
        let request = ClientRequest {
            host,
            token,
            no_retry,
            ca_bundle,
            user_agent: user_agent(),
            request_timeout,
            wait_on_depleted_rate_limit,
        };
        self.factory.build(&request)
    }

    /// Returns true when interactive prompting is permitted (SPEC §41).
    ///
    /// Both streams must be interactive, and neither `--no-input` nor `--json`
    /// may be set; otherwise a missing required value is a deterministic usage
    /// error instead of a prompt (PRD §30).
    #[must_use]
    pub fn can_prompt(&self) -> bool {
        !self.global.no_input
            && !self.global.json
            && self.env.stdin_is_terminal()
            && self.env.stdout_is_terminal()
    }

    /// The configured editor preference (`[settings] editor`), when set.
    ///
    /// Environment overrides (`$VISUAL`, `$EDITOR`) are applied by
    /// [`crate::editor`] on top of this value; a config that cannot be read is
    /// an error rather than a silent fall back to defaults (SPEC §23).
    pub fn configured_editor(&self) -> Result<Option<String>, CliError> {
        Ok(self.config.load()?.settings.and_then(|s| s.editor))
    }

    /// Whether ANSI colors may decorate human output this invocation.
    ///
    /// Uses the shared [`color_probe`] so `doctor`'s report and every other
    /// command's decorations agree on the decision.
    #[must_use]
    pub fn color_enabled(&self) -> bool {
        crate::terminal::color_probe(
            self.env,
            self.global.no_color,
            self.env.stdout_is_terminal(),
        )
        .ok
    }

    /// Whether the terminal is likely to understand 24-bit color.
    ///
    /// `COLORTERM=truecolor` is the convention; unknown terminals get the
    /// safer 256-color palette for swatches.
    #[must_use]
    pub fn truecolor_enabled(&self) -> bool {
        self.color_enabled()
            && matches!(
                self.env.var("COLORTERM").as_deref(),
                Some("truecolor") | Some("24bit")
            )
    }

    /// Whether a machine-readable document is expected on stdout.
    ///
    /// True for `--json`, `--jsonl`, and `--tsv`: every command that could emit
    /// a JSON document must emit it (through [`crate::output::Output::json`],
    /// which renders per mode) rather than fall back to human prose.
    #[must_use]
    pub fn json(&self) -> bool {
        self.out.is_structured()
    }

    /// Output-mode options derived from global flags (columns, jq, etc.).
    #[must_use]
    pub fn output_options(&self) -> OutputOptions {
        OutputOptions {
            columns: self.global.columns.clone().or_else(|| {
                // `--fields` is the server-side sparse fieldset on the Work
                // Item list commands and a projection alias elsewhere; it only
                // projects in table modes, so `--json` stays server-shaped.
                if self.out.mode().is_table() {
                    self.global.fields.as_deref().map(parse_field_list)
                } else {
                    None
                }
            }),
            no_header: self.global.no_header,
        }
    }
}

fn map_credential_error(err: CredentialError) -> CliError {
    CliError::credential(format!("credential store unavailable: {err}"))
}

/// Splits a comma-separated `--fields` value into ordered field names.
fn parse_field_list(fields: &str) -> Vec<String> {
    fields
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

/// The default `User-Agent` for API requests.
#[must_use]
pub fn user_agent() -> String {
    format!("hamstik-cli/{}", env!("CARGO_PKG_VERSION"))
}

/// Runs one parsed command to completion, returning the process exit code.
///
/// Validates conflicting global options, assembles the [`Session`], and maps
/// command errors to exit codes.
pub async fn run(cli: Cli, services: Services<'_>) -> i32 {
    let Services {
        env,
        store,
        config,
        factory,
        prompt,
        cwd,
        stdout,
        stderr,
    } = services;

    let global = cli.global.clone();
    let mode = if global.json {
        Mode::Json
    } else if global.jsonl {
        Mode::JsonLines
    } else if global.tsv {
        Mode::Tsv
    } else if global.quiet {
        Mode::Quiet
    } else if let Some(format) = global.format {
        format.mode()
    } else {
        Mode::Human
    };
    let mut out = Output::new(mode, global.verbose, stdout, stderr);

    // Reject mutually exclusive output modes before any network call.
    if global.json && global.jsonl {
        let err = CliError::usage("cannot combine --json and --jsonl");
        let _ = out.error(&err);
        return err.exit_code();
    }
    if global.json && global.tsv {
        let err = CliError::usage("cannot combine --json and --tsv");
        let _ = out.error(&err);
        return err.exit_code();
    }
    if global.jsonl && global.tsv {
        let err = CliError::usage("cannot combine --jsonl and --tsv");
        let _ = out.error(&err);
        return err.exit_code();
    }
    if (global.json || global.jsonl || global.tsv) && global.quiet {
        let err = CliError::usage("cannot combine a structured output mode with --quiet");
        let _ = out.error(&err);
        return err.exit_code();
    }
    if global.format.is_some() && (global.json || global.jsonl || global.tsv || global.quiet) {
        let err = CliError::usage("--format cannot be combined with another output-mode flag");
        let _ = out.error(&err);
        return err.exit_code();
    }
    if global.quiet && global.verbose {
        let err = CliError::usage("cannot combine --quiet and --verbose");
        let _ = out.error(&err);
        return err.exit_code();
    }
    if global.jq.is_some() && !mode.is_structured() {
        let err = CliError::usage("--jq requires --json, --jsonl, or --tsv");
        let _ = out.error(&err);
        return err.exit_code();
    }
    if global.jq.is_some() && global.columns.is_some() {
        let err = CliError::usage("cannot combine --jq and --columns");
        let _ = out.error(&err);
        return err.exit_code();
    }
    // Table projection is a table feature: in --json/--jsonl the server field
    // set is the contract, and --quiet prints identifiers only. Reject the
    // combination instead of silently ignoring what the caller asked for.
    // (`--fields` is deliberately not rejected: on the Work Item list commands
    // it is the server-side sparse fieldset, which `--json` honours.)
    if (global.columns.is_some() || global.no_header)
        && matches!(mode, Mode::Json | Mode::JsonLines | Mode::Quiet)
    {
        let err = CliError::usage(
            "--columns/--no-header apply to human, TSV, CSV, and Markdown table output",
        );
        let _ = out.error(&err);
        return err.exit_code();
    }
    // Compile the filter up front: an invalid expression is a usage error that
    // must fail before any network call, not after the command has run.
    if let Some(expr) = global.jq.as_deref()
        && let Err(message) = crate::output::validate_jq(expr)
    {
        let err = CliError::usage(format!("--jq filter error: {message}"));
        let _ = out.error(&err);
        return err.exit_code();
    }
    out.set_jq(global.jq.clone());

    let mut session = Session {
        out,
        global,
        env,
        store,
        config,
        factory,
        prompt,
        cwd,
        exit_code: SUCCESS,
    };

    // `--dry-run` is meaningful only for mutation commands; reads
    // have nothing to preview, so reject it with a precise explanation.
    if session.global.dry_run && !crate::commands::supports_dry_run(&cli.command) {
        let err = CliError::usage(
            "--dry-run applies only to mutation commands (create/edit/transition/archive/unarchive/delete, labels, attachments, comments, links, bulk, project/sprint/label mutations)",
        );
        let _ = session.out.error(&err);
        return err.exit_code();
    }

    match commands::dispatch(&mut session, &cli.command).await {
        Ok(()) => session.exit_code,
        Err(err) => {
            let _ = session.out.error(&err);
            err.exit_code()
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::context::{ResolvedField, Source};
    use crate::credentials::MemoryCredentialStore;
    use hamstik_api_client::Host;
    use std::collections::BTreeMap;

    struct NullPrompt;

    impl crate::input::Prompt for NullPrompt {
        fn read_line(&mut self, _prompt: &str) -> std::io::Result<String> {
            Err(std::io::Error::other("no interactive input in tests"))
        }
        fn read_secret(&mut self, _prompt: &str) -> std::io::Result<String> {
            Err(std::io::Error::other("no interactive input in tests"))
        }
    }

    /// Prompt that returns one canned answer (for consent-prompt tests).
    struct ScriptedPrompt {
        answer: String,
    }

    impl crate::input::Prompt for ScriptedPrompt {
        fn read_line(&mut self, _prompt: &str) -> std::io::Result<String> {
            Ok(self.answer.clone())
        }
        fn read_secret(&mut self, _prompt: &str) -> std::io::Result<String> {
            Ok(self.answer.clone())
        }
    }

    struct StaticEnvironment {
        vars: BTreeMap<String, String>,
        terminals: bool,
    }

    impl StaticEnvironment {
        fn new() -> Self {
            Self {
                vars: BTreeMap::new(),
                terminals: false,
            }
        }

        fn with_terminals(mut self) -> Self {
            self.terminals = true;
            self
        }
    }

    impl crate::environment::Environment for StaticEnvironment {
        fn var(&self, key: &str) -> Option<String> {
            self.vars.get(key).cloned()
        }
        fn stdout_is_terminal(&self) -> bool {
            self.terminals
        }
        fn stdin_is_terminal(&self) -> bool {
            self.terminals
        }
    }

    struct NullFactory;

    impl ApiFactory for NullFactory {
        fn build(&self, _request: &ClientRequest) -> Result<Arc<dyn HamstikApi>, CliError> {
            Err(CliError::protocol("no api in credential tests"))
        }

        fn build_public(
            &self,
            _request: &PublicClientRequest,
        ) -> Result<Arc<dyn HamstikApi>, CliError> {
            Err(CliError::protocol("no api in credential tests"))
        }
    }

    /// Builds a session whose `store` is a leaked `MemoryCredentialStore`;
    /// the test seeds credentials through the same leaked reference via the
    /// trait's `set` (writes are visible to the session's reads). Profile
    /// metadata rides on the selection, not the session.
    fn test_session() -> Session<'static> {
        test_session_with(test_global(), false)
    }

    fn test_global() -> crate::args::GlobalOptions {
        crate::args::GlobalOptions {
            host: None,
            profile: None,
            org: None,
            project: None,
            json: false,
            jsonl: false,
            tsv: false,
            format: None,
            quiet: false,
            jq: None,
            columns: None,
            fields: None,
            no_header: false,
            verbose: false,
            no_color: false,
            no_input: false,
            confirm_destructive: false,
            yes: false,
            no_retry: false,
            dry_run: false,
            ca_bundle: None,
        }
    }

    fn test_session_with(global: crate::args::GlobalOptions, terminals: bool) -> Session<'static> {
        let prompt: &'static mut dyn crate::input::Prompt = Box::leak(Box::new(NullPrompt));
        test_session_with_prompt(global, terminals, prompt)
    }

    fn test_session_with_prompt(
        global: crate::args::GlobalOptions,
        terminals: bool,
        prompt: &'static mut dyn crate::input::Prompt,
    ) -> Session<'static> {
        let mut env = StaticEnvironment::new();
        if terminals {
            env = env.with_terminals();
        }
        let env = Box::new(env);
        let config_store = crate::config::ConfigStore::new("/tmp/kilo/cred-test-config.toml");
        let out = Output::new(
            Mode::Human,
            false,
            Box::new(Vec::new()),
            Box::new(Vec::new()),
        );
        let leaked: &'static MemoryCredentialStore =
            Box::leak(Box::new(MemoryCredentialStore::new()));
        let leaked_dyn: &'static dyn CredentialStore = leaked;
        let env_ref: &'static dyn crate::environment::Environment = Box::leak(env);
        let factory: &'static dyn ApiFactory = Box::leak(Box::new(NullFactory));
        Session {
            out,
            global,
            env: env_ref,
            store: leaked_dyn,
            config: config_store,
            factory,
            prompt,
            cwd: std::path::PathBuf::from("/tmp/kilo"),
            exit_code: 0,
        }
    }

    fn seed(session: &Session<'_>, host: &str, secret: &str) {
        session
            .store
            .set(
                &credentials::account_key(host, "user-1"),
                &SecretString::from(secret.to_string()),
            )
            .unwrap();
    }

    fn selection_with_host(host: &str, profile_host: &str) -> Selection {
        Selection {
            host: Host::parse(host).unwrap(),
            host_source: Source::Cli,
            profile: Some("test-profile".to_string()),
            profile_meta: Some(Profile {
                host: profile_host.to_string(),
                user_id: "user-1".to_string(),
                email: "u@x".to_string(),
                default_organization: None,
                default_project: None,
            }),
            organization: ResolvedField {
                value: None,
                source: Source::Default,
            },
            project: ResolvedField {
                value: None,
                source: Source::Default,
            },
            context_path: None,
            ephemeral_token: false,
        }
    }

    #[test]
    fn token_for_uses_overridden_host_credential_when_present() {
        let session = test_session();
        let selection = selection_with_host("https://a.example", "https://b.example");
        seed(&session, "https://a.example", "override-token");
        let token = session.token_for(&selection).unwrap();
        use secrecy::ExposeSecret;
        assert_eq!(token.expose_secret(), "override-token");
    }

    #[test]
    fn token_for_falls_back_to_profile_host_credential() {
        let session = test_session();
        let selection = selection_with_host("https://a.example", "https://b.example");
        seed(&session, "https://b.example", "profile-token");
        let token = session.token_for(&selection).unwrap();
        use secrecy::ExposeSecret;
        assert_eq!(token.expose_secret(), "profile-token");
    }

    #[test]
    fn token_for_reports_both_hosts_when_neither_matches() {
        let session = test_session();
        let selection = selection_with_host("https://a.example", "https://b.example");
        let err = session.token_for(&selection).unwrap_err();
        assert!(err.message.contains("https://a.example"), "{err}");
        assert!(err.message.contains("https://b.example"), "{err}");
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn can_prompt_requires_interactive_streams() {
        assert!(test_session_with(test_global(), true).can_prompt());
        assert!(!test_session_with(test_global(), false).can_prompt());
    }

    #[test]
    fn destructive_consent_requires_flag_or_interactive_yes() {
        // Interactive "yes" consents without a flag.
        let mut session = test_session_with_prompt(
            test_global(),
            true,
            Box::leak(Box::new(ScriptedPrompt {
                answer: "yes".to_string(),
            })),
        );
        assert!(
            crate::commands::require_destructive_consent(&mut session, "delete a work item")
                .is_ok()
        );

        // Interactive "no" is a usage error naming the flag.
        let mut session = test_session_with_prompt(
            test_global(),
            true,
            Box::leak(Box::new(ScriptedPrompt {
                answer: "n".to_string(),
            })),
        );
        let err = crate::commands::require_destructive_consent(&mut session, "delete a work item")
            .unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.message.contains("--confirm-destructive"));

        // Non-interactive without consent fails before prompting.
        let mut session = test_session();
        let err = crate::commands::require_destructive_consent(&mut session, "delete a work item")
            .unwrap_err();
        assert_eq!(err.exit_code(), 2);

        // `--confirm-destructive` and `--yes` both consent without a prompt.
        for global in [
            crate::args::GlobalOptions {
                confirm_destructive: true,
                ..test_global()
            },
            crate::args::GlobalOptions {
                yes: true,
                ..test_global()
            },
        ] {
            let mut session = test_session_with(global, false);
            assert!(
                crate::commands::require_destructive_consent(&mut session, "delete a work item")
                    .is_ok()
            );
        }

        // `--dry-run` never mutates and needs no consent.
        let mut session = test_session_with(
            crate::args::GlobalOptions {
                dry_run: true,
                ..test_global()
            },
            false,
        );
        assert!(
            crate::commands::require_destructive_consent(&mut session, "delete a work item")
                .is_ok()
        );
    }

    #[test]
    fn destructive_action_maps_the_gated_commands() {
        use clap::Parser;
        let parse = |args: &[&str]| {
            let mut full = vec!["hamstik"];
            full.extend_from_slice(args);
            crate::args::Cli::try_parse_from(full).unwrap().command
        };
        assert_eq!(
            crate::commands::destructive_action(&parse(&["work", "delete", "HAM-1"])),
            Some("delete a work item")
        );
        assert_eq!(
            crate::commands::destructive_action(&parse(&["project", "archive", "WEB"])),
            Some("archive a project")
        );
        assert_eq!(
            crate::commands::destructive_action(&parse(&[
                "sprint",
                "transition",
                "11111111-1111-1111-1111-111111111111",
                "done"
            ])),
            Some("complete a sprint")
        );
        // Not gated: other status transitions, work archive, project unarchive.
        assert_eq!(
            crate::commands::destructive_action(&parse(&[
                "sprint",
                "transition",
                "11111111-1111-1111-1111-111111111111",
                "active"
            ])),
            None
        );
        assert_eq!(
            crate::commands::destructive_action(&parse(&["work", "archive", "HAM-1"])),
            None
        );
        assert_eq!(
            crate::commands::destructive_action(&parse(&["project", "unarchive", "WEB"])),
            None
        );
    }

    #[test]
    fn can_prompt_is_disabled_by_json_and_no_input() {
        let json_global = crate::args::GlobalOptions {
            json: true,
            ..test_global()
        };
        assert!(!test_session_with(json_global, true).can_prompt());

        let no_input_global = crate::args::GlobalOptions {
            no_input: true,
            ..test_global()
        };
        assert!(!test_session_with(no_input_global, true).can_prompt());
    }
}
