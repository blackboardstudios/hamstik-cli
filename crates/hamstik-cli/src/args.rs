// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Command-line argument definitions (clap).
//!
//! Global options are declared once and propagate to every subcommand
//! (SPEC §37). Enum-valued options are validated here on the request side; the
//! server remains authoritative for whether a value is actually accepted.

use std::path::PathBuf;

use crate::time_arg::TimeArg;
use clap::{Args, Parser, Subcommand, ValueEnum};

/// Top-level CLI definition.
#[derive(Parser, Debug)]
#[command(
    name = "hamstik",
    version,
    about = "Official command-line interface for Hamstik",
    propagate_version = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Options shared by every subcommand.
    #[command(flatten)]
    pub global: GlobalOptions,

    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Global options available to every command.
#[derive(Args, Debug, Clone)]
pub struct GlobalOptions {
    /// Hamstik host origin (overrides profile/context/default).
    #[arg(long, global = true, value_name = "URL")]
    pub host: Option<String>,

    /// Profile name to use for credentials and defaults.
    #[arg(long, global = true, value_name = "NAME")]
    pub profile: Option<String>,

    /// Organization slug override.
    #[arg(long, global = true, value_name = "SLUG")]
    pub org: Option<String>,

    /// Project key override.
    #[arg(long, global = true, value_name = "KEY")]
    pub project: Option<String>,

    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    pub json: bool,

    /// Emit JSON Lines (one JSON object per line, NDJSON).
    #[arg(long, global = true, conflicts_with_all = ["json", "tsv"])]
    pub jsonl: bool,

    /// Emit tab-separated values.
    #[arg(long, global = true, conflicts_with_all = ["json", "jsonl"])]
    pub tsv: bool,

    /// Select the output format by name. `ndjson`/`jsonl` stream one JSON
    /// resource per line, `tsv` and `csv` render the command's table, `table`
    /// (or `human`) is the aligned human table, `json` is one pretty-printed
    /// document, and `markdown` renders a GitHub-flavored table for list-shaped
    /// output. Conflicts with the dedicated output-mode flags.
    #[arg(
        long = "format",
        global = true,
        value_name = "FORMAT",
        value_enum,
        conflicts_with_all = ["json", "jsonl", "tsv", "quiet"]
    )]
    pub format: Option<OutputFormatArg>,

    /// Emit only essential identifiers.
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Apply a jq filter expression to structured output (requires --json, --jsonl, or --tsv).
    #[arg(long, global = true, value_name = "EXPR", value_parser = clap::builder::NonEmptyStringValueParser::new())]
    pub jq: Option<String>,

    /// Restrict list output to these columns (header names), in order.
    #[arg(long, global = true, value_name = "NAME", value_parser = clap::builder::NonEmptyStringValueParser::new(), num_args = 1..)]
    pub columns: Option<Vec<String>>,

    /// Restrict output to these comma-separated fields, in order. An alias for
    /// `--columns` on commands that do not take a server-side sparse fieldset;
    /// on the Work Item list commands the value is the server sparse fieldset.
    /// Unknown names fail as a usage error listing the valid names.
    #[arg(
        long = "fields",
        global = true,
        value_name = "FIELDS",
        value_parser = clap::builder::NonEmptyStringValueParser::new(),
        conflicts_with_all = ["columns", "jq"]
    )]
    pub fields: Option<String>,

    /// Suppress the header row in list output (TSV and human table modes).
    #[arg(long, global = true)]
    pub no_header: bool,

    /// Show diagnostic details on stderr.
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Disable colored output. Equivalent to `--color=never`.
    #[arg(long, global = true, conflicts_with = "color")]
    pub no_color: bool,

    /// Control colored output. `auto` (the default) honors `NO_COLOR`,
    /// `HAMSTIK_NO_COLOR`, `CLICOLOR_FORCE`, and terminal detection; `always`
    /// forces ANSI color; `never` disables it. Conflicts with `--no-color`.
    #[arg(
        long = "color",
        global = true,
        value_name = "WHEN",
        value_enum,
        conflicts_with = "no_color"
    )]
    pub color: Option<ColorChoice>,

    /// Never prompt interactively; fail instead.
    #[arg(long, global = true)]
    pub no_input: bool,

    /// Consent to the destructive operation this invocation performs
    /// (`work delete`, `project archive`, or completing a Sprint). Required
    /// when interactive confirmation is unavailable; `--yes` is the
    /// scripting override.
    #[arg(long = "confirm-destructive", global = true)]
    pub confirm_destructive: bool,

    /// Scripting override: consent to the destructive operation this
    /// invocation performs without prompting. Prefer `--confirm-destructive`
    /// in interactive sessions.
    #[arg(long, global = true)]
    pub yes: bool,

    /// Disable automatic retries.
    #[arg(long, global = true)]
    pub no_retry: bool,

    /// Preview the mutation instead of sending it: resolves identifiers and
    /// validates local input, prints a versioned preview (or `--json`
    /// envelope), and sends no request. Supported by mutation commands only.
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Additional PEM root certificate bundle.
    #[arg(long, global = true, value_name = "PATH")]
    pub ca_bundle: Option<PathBuf>,
}

/// `--format` umbrella values mapped onto the output modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormatArg {
    /// Aligned human table (the default).
    Table,
    /// Aligned human table (alias for `table`).
    Human,
    /// One pretty-printed JSON document.
    Json,
    /// One compact JSON resource per line (JSON Lines / NDJSON).
    Ndjson,
    /// One compact JSON resource per line (alias for `ndjson`).
    Jsonl,
    /// Tab-separated table rows.
    Tsv,
    /// Comma-separated table rows.
    Csv,
    /// GitHub-flavored Markdown table for list-shaped output.
    Markdown,
}

impl OutputFormatArg {
    /// The output mode this format selects.
    #[must_use]
    pub fn mode(self) -> crate::output::Mode {
        match self {
            Self::Table | Self::Human => crate::output::Mode::Human,
            Self::Json => crate::output::Mode::Json,
            Self::Ndjson | Self::Jsonl => crate::output::Mode::JsonLines,
            Self::Tsv => crate::output::Mode::Tsv,
            Self::Csv => crate::output::Mode::Csv,
            Self::Markdown => crate::output::Mode::Markdown,
        }
    }
}

/// `--color` values mapped onto the terminal color mode (SPEC §40).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    /// Honor the environment and terminal detection (the default).
    Auto,
    /// Force ANSI color on, even when stdout is not a terminal.
    Always,
    /// Disable ANSI color, even on a capable terminal.
    Never,
}

impl From<ColorChoice> for crate::terminal::ColorMode {
    fn from(value: ColorChoice) -> Self {
        match value {
            ColorChoice::Auto => Self::Auto,
            ColorChoice::Always => Self::Always,
            ColorChoice::Never => Self::Never,
        }
    }
}

impl GlobalOptions {
    /// The effective color mode after folding in the legacy `--no-color` flag.
    ///
    /// `--no-color` and `--color` are mutually exclusive at the parser, so at
    /// most one of the two is set.
    #[must_use]
    pub fn color_mode(&self) -> crate::terminal::ColorMode {
        if self.no_color {
            crate::terminal::ColorMode::Never
        } else {
            self.color
                .map_or(crate::terminal::ColorMode::Auto, Into::into)
        }
    }
}

/// Every CLI subcommand.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show the authenticated user and credential context.
    Me,
    /// Manage authentication and profiles.
    Auth(AuthArgs),
    /// Inspect and manage the working context.
    Context(ContextArgs),
    /// Inspect and modify global CLI configuration.
    Config(ConfigArgs),
    /// Work with organizations.
    Org(OrgArgs),
    /// Work with projects.
    Project(ProjectArgs),
    /// Work with saved Advanced Reports.
    Report(AdvancedReportArgs),
    /// View and run Advanced Dashboards.
    Dashboard(AdvancedDashboardArgs),
    /// Work with sprints.
    Sprint(SprintArgs),
    /// Read-only board (kanban) view composed from existing reads.
    Board(BoardArgs),
    /// Work with Project release versions, announcements, and audit packages.
    Release(ReleaseArgs),
    /// Work with Organization Milestones.
    Milestone(MilestoneArgs),
    /// Work with labels.
    Label(LabelArgs),
    /// Discover and administer Organization Attributes.
    Attribute(AttributeArgs),
    /// Work with work items.
    Work(Box<WorkArgs>),
    /// View user profiles, work, activity, and avatars.
    User(UserArgs),
    /// Validate SqueakQL expressions.
    Squeakql(SqueakQlArgs),
    /// Inspect the Public API contract.
    Api(ApiArgs),
    /// Agent automation: manage the bundled Agent Skill and validate the
    /// local agent harness.
    Agent(AgentArgs),
    /// Verify configuration, credentials, connectivity, API compatibility,
    /// and selected Organization/Project context.
    Doctor(DoctorArgs),
    /// Generate a shell completion script.
    Completion(CompletionArgs),
    /// Internal: dynamic shell completion for live values.
    #[command(name = "_hamstik_dyn_complete", hide = true)]
    Complete(CompleteArgs),
    /// Bootstrap the working directory for Hamstik.
    Init,
    /// Print the machine-readable command manifest derived from the real
    /// command tree.
    Commands(CommandsArgs),
    /// Print the CLI version.
    Version,
    /// External plugin subcommand (`hamstik-<name>` on PATH). Captured by
    /// clap as an `external_subcommand` — any unrecognized first word after
    /// `hamstik` falls here. The plugin executable is invoked with the
    /// resolved context as environment variables (never as argv).
    #[command(external_subcommand)]
    External(Vec<String>),
}

/// Arguments for the `report` command group.
#[derive(Args, Debug)]
pub struct AdvancedReportArgs {
    /// Advanced Report subcommand to run.
    #[command(subcommand)]
    pub command: AdvancedReportCommand,
}

/// Advanced Report subcommands.
#[derive(Subcommand, Debug)]
pub enum AdvancedReportCommand {
    /// List Advanced Reports visible to the current user.
    List {
        /// Visibility to include.
        #[arg(long, default_value = "all", value_parser = ["all", "personal", "organization"])]
        visibility: String,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// View one Advanced Report by UUID.
    View {
        /// Advanced Report UUID.
        id: String,
    },
    /// Create an Advanced Report from a complete JSON definition.
    Create {
        /// JSON file containing an AdvancedReportInput (`-` for stdin).
        #[arg(long, value_name = "PATH")]
        file: String,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Replace an Advanced Report from a complete JSON definition.
    Edit {
        /// Advanced Report UUID.
        id: String,
        /// JSON file containing an AdvancedReportInput (`-` for stdin).
        #[arg(long, value_name = "PATH")]
        file: String,
        /// Bypass optimistic-concurrency protection (`If-Match: *`).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Delete an Advanced Report.
    Delete {
        /// Advanced Report UUID.
        id: String,
        /// Bypass optimistic-concurrency protection (`If-Match: *`).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Evaluate an Advanced Report at its current revision.
    Run {
        /// Advanced Report UUID.
        id: String,
    },
    /// List Work Items captured by one report-run result cell.
    SelectionItems {
        /// Advanced Report run UUID.
        run_id: String,
        /// Result cell UUID.
        cell_id: String,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
}

/// Arguments for the `dashboard` command group.
#[derive(Args, Debug)]
pub struct AdvancedDashboardArgs {
    /// Advanced Dashboard subcommand to run.
    #[command(subcommand)]
    pub command: AdvancedDashboardCommand,
}

/// Advanced Dashboard subcommands.
#[derive(Subcommand, Debug)]
pub enum AdvancedDashboardCommand {
    /// List Advanced Dashboards visible to the current user.
    List {
        /// Visibility to include.
        #[arg(long, default_value = "all", value_parser = ["all", "personal", "organization"])]
        visibility: String,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// View one Advanced Dashboard by UUID.
    View {
        /// Advanced Dashboard UUID.
        id: String,
    },
    /// Evaluate an Advanced Dashboard at its current revision.
    Run {
        /// Advanced Dashboard UUID.
        id: String,
        /// Optional JSON file containing AdvancedDashboardFilters (`-` for stdin).
        #[arg(long = "filters-file", value_name = "PATH")]
        filters_file: Option<String>,
    },
}

/// Arguments for the `squeakql` command group.
#[derive(Args, Debug)]
pub struct SqueakQlArgs {
    /// The SqueakQL subcommand to run.
    #[command(subcommand)]
    pub command: SqueakQlCommand,
}

/// SqueakQL subcommands.
#[derive(Subcommand, Debug)]
pub enum SqueakQlCommand {
    /// Validate an expression without executing it.
    Validate {
        /// SqueakQL expression (inline).
        #[arg(value_name = "QUERY")]
        query: Option<String>,
        /// Read the expression from a file (`-` for stdin) instead of inline.
        #[arg(long, value_name = "PATH", conflicts_with = "query")]
        file: Option<String>,
        /// Run a saved query by name instead of an inline expression.
        #[arg(long, value_name = "NAME", conflicts_with_all = ["query", "file"])]
        saved: Option<String>,
    },
    /// List saved SqueakQL queries.
    List,
    /// Show a saved SqueakQL query's expression.
    Show {
        /// Saved query name.
        name: String,
    },
    /// Save an expression as a named query for reuse.
    Save {
        /// Saved query name (letters, digits, `-`, `_`; 1–64 chars).
        name: String,
        /// SqueakQL expression (inline), or provide it with --file/stdin.
        #[arg(value_name = "QUERY")]
        query: Option<String>,
        /// Read the expression from a file (`-` for stdin) instead of inline.
        #[arg(long, value_name = "PATH", conflicts_with = "query")]
        file: Option<String>,
        /// Replace an existing saved query of the same name.
        #[arg(long)]
        force: bool,
    },
    /// Delete a saved SqueakQL query.
    Delete {
        /// Saved query name.
        name: String,
    },
}

/// Arguments for the `api` command group.
#[derive(Args, Debug)]
pub struct ApiArgs {
    /// The API subcommand to run.
    #[command(subcommand)]
    pub command: ApiCommand,
}

/// Arguments for the `agent` command group.
#[derive(Args, Debug)]
pub struct AgentArgs {
    /// The agent subcommand to run.
    #[command(subcommand)]
    pub command: AgentCommand,
}

/// Arguments for the `doctor` command.
#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Check only local configuration, context, credential-store access,
    /// terminal behavior, and bundled compatibility metadata; remote
    /// checks are marked skipped and no network traffic is generated.
    #[arg(long)]
    pub local_only: bool,
    /// Write a versioned support bundle to the given path. The bundle is a
    /// ZIP containing: `bundle-manifest.json` (layout version and file list),
    /// `doctor-report.json` (redacted diagnostic results),
    /// `context-explain.json` (redacted context resolution chains),
    /// `cli-info.json` (version, target, build profile),
    /// `api-compatibility.json` (required/additive operation counts),
    /// and `config-metadata.json` (safe config summary, no secrets).
    ///
    /// Redaction rules: no PAT values, no Authorization headers, no
    /// credential-store contents, no token values, no proxy URLs with
    /// embedded credentials, and no arbitrary environment dumps. The bundle
    /// is not telemetry: it is written locally and never uploaded.
    ///
    /// Compatible with `--local-only`; remote checks are safely skipped
    /// when that flag is active and only safe, local data is included.
    ///
    /// Layout version is `1.0` and is declared in the manifest so future
    /// support workflows can parse it reliably.
    ///
    /// See also: `doctor --help`.
    #[arg(long, value_name = "PATH")]
    pub bundle: Option<PathBuf>,
}

/// Agent automation subcommands.
#[derive(Subcommand, Debug)]
pub enum AgentCommand {
    /// Manage the canonical bundled Agent Skill.
    Skill(SkillArgs),
    /// Validate the local agent harness offline: installed skill metadata
    /// against the running CLI version, credential-shaped content in the
    /// config directory (reported without displaying values), and bundled
    /// OpenAPI snapshot freshness when online. Also available as `agent
    /// doctor`.
    #[command(visible_alias = "doctor")]
    Validate(ValidateArgs),
}

/// Arguments for `hamstik agent validate`.
#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// Explicit path to a `SKILL.md` to validate instead of the default
    /// installed-location lookup.
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,
    /// Skip the live OpenAPI snapshot-freshness comparison and make no
    /// network requests.
    #[arg(long)]
    pub offline: bool,
}

/// Arguments for the `agent skill` command group.
#[derive(Args, Debug)]
pub struct SkillArgs {
    /// The skill subcommand to run.
    #[command(subcommand)]
    pub command: SkillCommand,
}

/// Agent Skill management subcommands.
#[derive(Subcommand, Debug)]
pub enum SkillCommand {
    /// Install the canonical bundled Agent Skill into a location agents
    /// discover. Defaults to the current project's portable
    /// `.agents/skills/hamstik/` location; `--global` installs into the
    /// user-level portable location. A locally modified installed skill is
    /// never silently overwritten; `--force` is required for replacement.
    Install {
        /// Install into the user-level portable Agent Skills location
        /// instead of the default current-project location.
        #[arg(long)]
        global: bool,
        /// Replace an existing locally modified installed skill.
        #[arg(long)]
        force: bool,
    },
    /// Validate an Agent Skill against this binary's command surface and
    /// compatibility metadata. Defaults to the installed skill; an explicit
    /// PATH validates any skill file (this is what CI runs).
    Check {
        /// Explicit path to a `SKILL.md` to validate instead of the default
        /// installed-location lookup.
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
    },
}

/// Public API metadata subcommands.
#[derive(Subcommand, Debug)]
pub enum ApiCommand {
    /// Print the live Public API OpenAPI document.
    Openapi,
    /// Perform one cheap authenticated Public API read and print the
    /// current rate-limit snapshot (`Limit`, `Remaining`, `ResetIn`).
    ///
    /// The snapshot mirrors the `meta.rateLimit` fields of
    /// `api request --json` and is a point-in-time observation: limits can
    /// change between calls, so it is not a guarantee against future 429s.
    RateLimit,
    /// Call a documented Public API v1 route through the CLI.
    Request(RequestArgs),
    /// A bare Public API v1 path (`hamstik api /api/v1/...`) — forwarded to
    /// `api request`.
    #[command(external_subcommand)]
    Passthrough(Vec<String>),
}

/// Arguments for `api request`: a generic Public API v1 passthrough
/// that reuses the typed commands' transport (auth, TLS, retries,
/// idempotency, redaction) without ALM-semantics knowledge.
#[derive(Args, Debug)]
pub struct RequestArgs {
    /// The Public API v1 path (must start with `/api/v1/`).
    #[arg(value_name = "PATH")]
    pub path: String,

    /// HTTP method (GET is the default; the route's documented methods apply).
    #[arg(long, value_name = "METHOD", default_value = "GET",
        value_parser = ["GET", "POST", "PATCH", "PUT", "DELETE"])]
    pub method: String,

    /// Read the JSON request body from this file; `-` reads stdin.
    #[arg(long, value_name = "FILE", conflicts_with = "field")]
    pub body_file: Option<String>,

    /// Structured JSON field as `key=value`; repeatable. Value is parsed as
    /// JSON when it parses, else a JSON string.
    #[arg(long, value_name = "KEY=VALUE")]
    pub field: Vec<String>,

    /// Query parameter as `key=value`; repeatable (allowlisted, ordered).
    #[arg(long, value_name = "KEY=VALUE")]
    pub query: Vec<String>,

    /// Header override as `Name: value`; restricted to a safe allowlist
    /// (`Accept`, `Content-Type`, `If-Match`). `Authorization` and any
    /// credential-bearing header are rejected.
    #[arg(long, value_name = "NAME:VALUE")]
    pub header: Vec<String>,

    /// Idempotency key override; mutations get a fresh strong key otherwise
    /// (generated once and reused across internal retries).
    #[arg(long, value_name = "KEY")]
    pub idempotency_key: Option<String>,
}

/// Arguments for `work context`: the one-invocation read bundle.
#[derive(Args, Debug)]
pub struct WorkContextArgs {
    /// Work item key (e.g. HAM-42).
    #[arg(value_name = "KEY")]
    pub key: String,

    /// Maximum comments included (oldest first, server-capped). 0 omits the
    /// comments section with an explicit marker.
    #[arg(long, value_name = "N", default_value_t = 10)]
    pub comments: u32,

    /// Maximum activity events included (newest first). 0 omits the
    /// activity section with an explicit marker.
    #[arg(long, value_name = "N", default_value_t = 10)]
    pub activity: u32,

    /// Omit long text bodies (description, comment bodies) — each replaced
    /// by an explicit truncated marker.
    #[arg(long)]
    pub compact: bool,
}

/// Arguments for the `auth` command group.
#[derive(Args, Debug)]
pub struct AuthArgs {
    /// The auth subcommand to run.
    #[command(subcommand)]
    pub command: AuthCommand,
}

/// Authentication and profile management subcommands.
#[derive(Subcommand, Debug)]
pub enum AuthCommand {
    /// Authenticate with a Personal Access Token.
    ///
    /// Interactive login prompts for the token; `--with-token` reads it from
    /// stdin. `HAMSTIK_TOKEN` is ephemeral and is never persisted, so it is not
    /// accepted as a login source.
    Login {
        /// Read the token from stdin instead of prompting.
        #[arg(long)]
        with_token: bool,
    },
    /// Show the current authentication state.
    Status,
    /// List configured profiles.
    List,
    /// Select the active profile.
    Switch {
        /// Profile name to activate.
        #[arg(value_name = "PROFILE")]
        profile: String,
    },
    /// Remove the local credential (does not revoke the server-side PAT).
    Logout,
    /// Log out and forget a profile: remove its stored credential and its
    /// config entry (does not revoke the server-side PAT).
    ///
    /// The profile is named positionally: `hamstik auth forget NAME`, or run
    /// with no argument to forget the selected profile. Unlike `logout`, this
    /// runs even while HAMSTIK_TOKEN is set: forget is explicit about the
    /// profile being removed, not about the credential currently in use.
    Forget {
        /// Profile to forget (defaults to the selected profile).
        #[arg(value_name = "PROFILE")]
        profile: Option<String>,
    },
}

/// Arguments for the `context` command group.
#[derive(Args, Debug)]
pub struct ContextArgs {
    /// The context subcommand to run.
    #[command(subcommand)]
    pub command: ContextCommand,
}

/// Working-context subcommands.
#[derive(Subcommand, Debug)]
pub enum ContextCommand {
    /// Show the resolved context.
    Show {
        /// Show the source of each value.
        #[arg(long)]
        explain: bool,
    },
    /// Set context values in the nearest .hamstik.toml.
    Set {
        /// Organization slug.
        #[arg(long)]
        org: Option<String>,
        /// Project key.
        #[arg(long)]
        project: Option<String>,
    },
    /// Remove the current directory's .hamstik.toml values.
    Clear,
    /// Create a .hamstik.toml in the current directory.
    Init,
    /// Explain how every resolved setting won precedence, fully offline.
    Explain,
}

/// Arguments for the `config` command group.
#[derive(Args, Debug)]
pub struct ConfigArgs {
    /// The config subcommand to run.
    #[command(subcommand)]
    pub command: ConfigCommand,
}

/// Configuration subcommands.
#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Print the path to the active configuration file.
    Path,
    /// List all effective non-secret configuration values.
    List,
    /// Get a single configuration value.
    Get {
        /// Configuration key: `profile`, `organization`, `project`, `editor`,
        /// `pager`, `output`, `git_branch_template`, or `audit_log`.
        key: String,
    },
    /// Set a configuration value.
    Set {
        /// Configuration key: `profile`, `organization`, `project`, `editor`,
        /// `pager`, `output`, `git_branch_template`, or `audit_log`.
        key: String,
        /// New value for the key: `true`/`false` for `audit_log`, an existing
        /// profile name for `profile`, a defined output mode (`human`, `json`,
        /// `jsonl`, `tsv`, `quiet`) for `output`. Credentials are never
        /// accepted: they live in the OS credential store or `HAMSTIK_TOKEN`.
        value: String,
    },
    /// Remove a configuration value.
    Unset {
        /// Configuration key to remove: `profile`, `organization`, `project`,
        /// `editor`, `pager`, `output`, `git_branch_template`, or `audit_log`.
        key: String,
    },
}

/// Arguments for the `org` command group.
#[derive(Args, Debug)]
pub struct OrgArgs {
    /// The org subcommand to run.
    #[command(subcommand)]
    pub command: OrgCommand,
}

/// Organization subcommands.
#[derive(Subcommand, Debug)]
pub enum OrgCommand {
    /// List organizations you belong to.
    List(PaginationArgs),
    /// View an organization.
    View {
        /// Organization slug.
        slug: String,
    },
    /// List active members of an organization.
    Members {
        /// Organization slug.
        slug: String,
        /// Literal substring filter over name or username.
        #[arg(long, value_name = "TEXT")]
        search: Option<String>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// List work items across the organization.
    Work(Box<OrgWorkListArgs>),
    /// Set the default organization for the active profile.
    Use {
        /// Organization slug.
        slug: String,
    },
}

/// Arguments for `org work`.
#[derive(Args, Debug)]
pub struct OrgWorkListArgs {
    /// Filter by project key (repeatable).
    #[arg(long)]
    pub project: Vec<String>,
    /// Shared Work Item filters.
    #[command(flatten)]
    pub filters: WorkFilters,
    /// Pagination options.
    #[command(flatten)]
    pub pagination: PaginationArgs,
}

/// Arguments for the `project` command group.
#[derive(Args, Debug)]
pub struct ProjectArgs {
    /// The project subcommand to run.
    #[command(subcommand)]
    pub command: ProjectCommand,
}

/// Project subcommands.
#[derive(Subcommand, Debug)]
pub enum ProjectCommand {
    /// List projects in the organization.
    List {
        /// List only archived projects (true) or only unarchived (false);
        /// when omitted, unarchived projects are listed. Listing both states
        /// in one result requires separate queries.
        #[arg(long, value_name = "true|false")]
        archived: Option<bool>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// View a project.
    View {
        /// Project key.
        key: String,
    },
    /// Create a project (Organization administrators only).
    Create(ProjectCreateArgs),
    /// Edit a project (Organization administrators only).
    Edit(ProjectEditArgs),
    /// Archive a project (Organization administrators only).
    Archive {
        /// Project key.
        key: String,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Unarchive a project (Organization administrators only).
    Unarchive {
        /// Project key.
        key: String,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Show a project's activity feed (newest first).
    Activity {
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Only events strictly after this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
        /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
        /// is read in the host's local time zone.
        #[arg(long, value_name = "DATE")]
        since: Option<TimeArg>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// Read a server project report (velocity, ageing-wip, epic-progress, …).
    Report(Box<ProjectReportArgs>),
    /// Compute a client-side aggregate summary of Work Items in the project.
    Stats(ProjectStatsArgs),
    /// Set the default project for the active profile.
    Use {
        /// Project key.
        key: String,
    },
}

/// Arguments for `project report`.
///
/// The report type and every shaping/filter option are forwarded verbatim; the
/// server owns report semantics, including which type names and option
/// combinations exist.
#[derive(Args, Debug)]
pub struct ProjectReportArgs {
    /// Report type (for example `velocity`, `cumulative-flow`, `control-chart`,
    /// `ageing-wip`, `created-vs-resolved`, `distribution`, `epic-progress`);
    /// passed to the server unchanged.
    pub report_type: String,
    /// Project key (overrides context).
    #[arg(long)]
    pub project: Option<String>,
    /// Recent-Sprint count or calendar-day window (the server caps the range
    /// per report type).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=366))]
    pub range: Option<u32>,
    /// Inclusive ISO calendar date starting the window.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub start: Option<String>,
    /// Inclusive ISO calendar date ending the window.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub end: Option<String>,
    /// IANA time zone the calendar window is evaluated in.
    #[arg(long, value_name = "ZONE")]
    pub time_zone: Option<String>,
    /// Measurement unit.
    #[arg(long, value_name = "UNIT", value_parser = ["count", "points", "items"])]
    pub unit: Option<String>,
    /// Sampling interval.
    #[arg(long, value_parser = ["day", "week", "month"])]
    pub interval: Option<String>,
    /// Time measure for cycle-time reports.
    #[arg(long, value_parser = ["cycle", "lead"])]
    pub measure: Option<String>,
    /// Canonical status whose first entry starts cycle time.
    #[arg(
        long = "cycle-start-status",
        value_name = "STATUS",
        value_parser = ["backlog", "todo", "in_progress", "in_review", "done"]
    )]
    pub cycle_start_status: Option<String>,
    /// Rolling window size for moving measures (1-100).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..=100))]
    pub window: Option<u32>,
    /// Grouping dimension.
    #[arg(
        long = "group-by",
        value_name = "DIM",
        value_parser = ["status", "type", "priority", "assignee", "label"]
    )]
    pub group_by: Option<String>,
    /// Which items count toward the report.
    #[arg(long, value_parser = ["open", "all"])]
    pub scope: Option<String>,
    /// Restrict the report to one Sprint (UUID).
    #[arg(long, value_name = "SPRINT")]
    pub sprint: Option<String>,
    /// Result ordering key.
    #[arg(
        long,
        value_name = "KEY",
        value_parser = [
            "name",
            "progress",
            "targetDate",
            "age",
            "workItem",
            "status",
            "assignee",
            "since"
        ]
    )]
    pub sort: Option<String>,
    /// Free-text filter on Work Items in scope.
    #[arg(long = "q", value_name = "TEXT")]
    pub query: Option<String>,
    /// SqueakQL filter expression restricting the items in scope.
    #[arg(long, value_name = "QUERY")]
    pub squeakql: Option<String>,
    /// Only count items in this status.
    #[arg(
        long,
        value_name = "STATUS",
        value_parser = ["backlog", "todo", "in_progress", "in_review", "done"]
    )]
    pub status: Option<String>,
    /// Only count items of this type.
    #[arg(
        long = "type",
        value_name = "TYPE",
        value_parser = ["task", "bug", "story", "feature", "epic"]
    )]
    pub work_type: Option<String>,
    /// Only count items at this priority.
    #[arg(
        long,
        value_name = "PRIORITY",
        value_parser = ["low", "medium", "high", "urgent"]
    )]
    pub priority: Option<String>,
    /// Only count items assigned to this user (`me` or a `usr_` public ID).
    #[arg(long, value_name = "ME|ID")]
    pub assignee: Option<String>,
    /// Only count items carrying this label.
    #[arg(long, value_name = "LABEL")]
    pub label: Option<String>,
    /// Bucket count or explicit boundaries for distribution reports.
    #[arg(long, value_name = "BUCKETS")]
    pub buckets: Option<String>,
    /// Report page options.
    #[command(flatten)]
    pub page: ReportPageArgs,
}

/// Page options for report commands.
///
/// A report document carries its series and rollups in one response; only the
/// report's `items` collection is paginated, so `--all` is intentionally
/// absent and a cursor resumes that collection and nothing else.
#[derive(Args, Debug)]
pub struct ReportPageArgs {
    /// Results per page (1-100; the server default is 50).
    #[arg(
        long,
        value_name = "N",
        value_parser = clap::value_parser!(u32).range(1..=100)
    )]
    pub limit: Option<u32>,
    /// Opaque cursor returned by a preceding page; `--since-cursor` is the
    /// pipeline-checkpoint spelling of the same option. Cursors are forwarded
    /// verbatim.
    #[arg(
        long,
        visible_alias = "since-cursor",
        value_name = "CURSOR",
        value_parser = clap::builder::NonEmptyStringValueParser::new()
    )]
    pub cursor: Option<String>,
}

/// Arguments for `project create`.
#[derive(Args, Debug)]
pub struct ProjectCreateArgs {
    /// Project name.
    #[arg(long)]
    pub name: Option<String>,
    /// Project key (defaults to the server's canonical suggestion from the name).
    #[arg(long, value_name = "KEY")]
    pub key: Option<String>,
    /// Description text.
    #[arg(long, conflicts_with_all = ["description_file", "description_editor"])]
    pub description: Option<String>,
    /// Description source (path, or - for stdin).
    #[arg(
        long = "description-file",
        value_name = "PATH",
        conflicts_with_all = ["description", "description_editor"]
    )]
    pub description_file: Option<String>,
    /// Author the description in $VISUAL/$EDITOR instead of passing text.
    #[arg(long = "description-editor", conflicts_with_all = ["description", "description_file"])]
    pub description_editor: bool,
    /// Display color as a hex string (e.g. #3b82f6).
    #[arg(long, value_name = "HEX")]
    pub color: Option<String>,
    /// Explicit idempotency key.
    #[arg(long = "idempotency-key", value_name = "KEY")]
    pub idempotency_key: Option<String>,
}

/// Arguments for `project edit`.
#[derive(Args, Debug)]
pub struct ProjectEditArgs {
    /// Project key.
    pub key: String,
    /// New name.
    #[arg(long)]
    pub name: Option<String>,
    /// New description text.
    #[arg(long, conflicts_with_all = ["clear_description", "description_file", "description_editor"])]
    pub description: Option<String>,
    /// New description source (path, or - for stdin).
    #[arg(
        long = "description-file",
        value_name = "PATH",
        conflicts_with_all = ["description", "clear_description", "description_editor"]
    )]
    pub description_file: Option<String>,
    /// Author the description in $VISUAL/$EDITOR instead of passing text.
    #[arg(
        long = "description-editor",
        conflicts_with_all = ["description", "description_file", "clear_description"]
    )]
    pub description_editor: bool,
    /// New display color as a hex string.
    #[arg(long, value_name = "HEX")]
    pub color: Option<String>,
    /// Clear the description.
    #[arg(long = "clear-description")]
    pub clear_description: bool,
    /// Bypass revision conflict protection (If-Match: *).
    #[arg(long)]
    pub force: bool,
    /// Explicit idempotency key.
    #[arg(long = "idempotency-key", value_name = "KEY")]
    pub idempotency_key: Option<String>,
}

/// Arguments for the `board` command group.
#[derive(Args, Debug)]
pub struct BoardArgs {
    /// The board subcommand to run.
    #[command(subcommand)]
    pub command: BoardCommand,
}

/// Board subcommands.
#[derive(Subcommand, Debug)]
pub enum BoardCommand {
    /// Render a read-only kanban board for a Sprint or Project.
    View(BoardViewArgs),
}

/// Arguments for `board view`.
///
/// A board is scoped to a Sprint (`--sprint <id>`), a Project
/// (`--project <KEY>`), or a Sprint within a Project. The Project resolves
/// from `--project`, the resolved context, or the selected profile, exactly
/// like every other Work Item read. The board is a read-only composition of
/// the existing Work Item list read; the CLI offers no board mutation because
/// the Public API exposes no board write.
#[derive(Args, Debug)]
pub struct BoardViewArgs {
    /// Sprint id (UUID) to scope the board to.
    #[arg(long, value_name = "SPRINT_ID")]
    pub sprint: Option<String>,
    /// Project key (overrides context).
    #[arg(long, value_name = "KEY")]
    pub project: Option<String>,
    /// Pagination options.
    #[command(flatten)]
    pub pagination: PaginationArgs,
}

/// Arguments for the `sprint` command group.
#[derive(Args, Debug)]
pub struct SprintArgs {
    /// The sprint subcommand to run.
    #[command(subcommand)]
    pub command: SprintCommand,
}

/// Sprint subcommands.
#[derive(Subcommand, Debug)]
pub enum SprintCommand {
    /// List sprints in a project.
    List {
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// View a sprint.
    View {
        /// Sprint id (UUID).
        id: String,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
    },
    /// Create a sprint.
    Create {
        /// Sprint name.
        #[arg(long)]
        name: Option<String>,
        /// Planned start date. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
        /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
        /// is read in the host's local time zone.
        #[arg(long = "start-date", value_name = "DATE")]
        start_date: Option<TimeArg>,
        /// Planned end date. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
        /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
        /// is read in the host's local time zone.
        #[arg(long = "end-date", value_name = "DATE")]
        end_date: Option<TimeArg>,
        /// Sprint goal.
        #[arg(long, value_name = "TEXT")]
        goal: Option<String>,
        /// Target story-point total.
        #[arg(long = "target-points", value_name = "N")]
        target_points: Option<i64>,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Read a server Sprint delivery report (commitment, scope changes,
    /// carryover, and burndown).
    Report(Box<SprintReportArgs>),
    /// Compute a client-side aggregate summary of Work Items in the sprint.
    Stats(SprintStatsArgs),
    /// List allowed sprint state transitions.
    Transitions {
        /// Sprint id (UUID).
        id: String,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
    },
    /// Archive a sprint.
    Archive {
        /// Sprint id (UUID).
        id: String,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Unarchive a sprint.
    Unarchive {
        /// Sprint id (UUID).
        id: String,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Transition a sprint to a target state.
    Transition {
        /// Sprint id (UUID).
        id: String,
        /// Target state.
        #[arg(value_enum)]
        target: SprintStateArg,
        /// Move unfinished work items back to the backlog when completing.
        #[arg(long = "move-to-backlog")]
        move_to_backlog: bool,
        /// Move unfinished work items to this future sprint.
        #[arg(long = "move-to-sprint", value_name = "SPRINT_ID")]
        move_to_sprint: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
}

/// Arguments for `sprint report`.
#[derive(Args, Debug)]
pub struct SprintReportArgs {
    /// Sprint id (UUID).
    pub id: String,
    /// Project key (overrides context).
    #[arg(long)]
    pub project: Option<String>,
    /// Report page options.
    #[command(flatten)]
    pub page: ReportPageArgs,
}

/// Arguments for `project stats`: client-side aggregate summary of Work Items.
#[derive(Args, Debug)]
pub struct ProjectStatsArgs {
    /// Project key (positional; overrides context).
    pub project: String,
    /// Free-text search over titles and descriptions.
    #[arg(long = "search", value_name = "TEXT")]
    pub search: Option<String>,
    /// Filter by status (repeatable).
    #[arg(long, value_enum)]
    pub status: Vec<StatusArg>,
    /// Status scope. When omitted, all statuses are included.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeArg>,
    /// Filter by type (repeatable).
    #[arg(long = "type", value_enum)]
    pub item_type: Vec<TypeArg>,
    /// Filter by priority (repeatable).
    #[arg(long, value_enum)]
    pub priority: Vec<PriorityArg>,
    /// Filter by assignee: me, none, a user UUID, or a public ID (usr_...).
    #[arg(long, value_name = "ME|NONE|ID")]
    pub assignee: Option<String>,
    /// Filter by sprint: none or a sprint UUID.
    #[arg(long, value_name = "NONE|UUID")]
    pub sprint: Option<String>,
    /// Filter by label UUID (repeatable).
    #[arg(long)]
    pub label: Vec<String>,
    /// Filter by label name (repeatable).
    #[arg(long = "label-name")]
    pub label_name: Vec<String>,
    /// Filter by parent work item key.
    #[arg(long)]
    pub parent: Option<String>,
    /// Filter by top-level state (a bare flag means true).
    #[arg(
        long = "top-level",
        value_name = "true|false",
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    pub top_level: Option<bool>,
    /// Only items updated at or after this time.
    #[arg(long = "updated-after", value_name = "DATE")]
    pub updated_after: Option<TimeArg>,
    /// Only overdue items (true) or only on-track items (false).
    #[arg(long, value_name = "true|false")]
    pub overdue: Option<bool>,
    /// Only items due strictly before this time.
    #[arg(long = "due-before", value_name = "DATE")]
    pub due_before: Option<TimeArg>,
    /// Only items due strictly after this time.
    #[arg(long = "due-after", value_name = "DATE")]
    pub due_after: Option<TimeArg>,
    /// Only archived (true) or only unarchived (false) items.
    #[arg(long, value_name = "true|false")]
    pub archived: Option<bool>,
    /// Follow all pages (default: one page only).
    #[arg(long)]
    pub all: bool,
    /// Maximum total items to aggregate (distinct from the server page size).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: Option<u32>,
    /// Opaque cursor returned by a preceding page.
    #[arg(
        long,
        visible_alias = "since-cursor",
        value_name = "CURSOR",
        value_parser = clap::builder::NonEmptyStringValueParser::new()
    )]
    pub cursor: Option<String>,
}

/// Arguments for `sprint stats`: client-side aggregate summary of Work Items.
#[derive(Args, Debug)]
pub struct SprintStatsArgs {
    /// Sprint id (UUID; positional).
    pub sprint: String,
    /// Project key (overrides context).
    #[arg(long)]
    pub project: Option<String>,
    /// Follow all pages (default: one page only).
    #[arg(long)]
    pub all: bool,
    /// Maximum total items to aggregate (distinct from the server page size).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: Option<u32>,
    /// Opaque cursor returned by a preceding page.
    #[arg(
        long,
        visible_alias = "since-cursor",
        value_name = "CURSOR",
        value_parser = clap::builder::NonEmptyStringValueParser::new()
    )]
    pub cursor: Option<String>,
}

/// Arguments for the `label` command group.
#[derive(Args, Debug)]
pub struct LabelArgs {
    /// The label subcommand to run.
    #[command(subcommand)]
    pub command: LabelCommand,
}

/// Label subcommands.
#[derive(Subcommand, Debug)]
pub enum LabelCommand {
    /// List labels in a project.
    List {
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// Create a project label (Organization administrators only).
    Create {
        /// Label name (stored lowercase).
        #[arg(long)]
        name: Option<String>,
        /// Display color as a hex string.
        #[arg(long, value_name = "HEX")]
        color: Option<String>,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
}

/// Arguments for the `attribute` command group.
#[derive(Args, Debug)]
pub struct AttributeArgs {
    /// The Attribute subcommand to run.
    #[command(subcommand)]
    pub command: AttributeCommand,
}

/// Organization Attribute, option, and Project enablement commands.
#[derive(Subcommand, Debug)]
pub enum AttributeCommand {
    /// List Organization definitions and product limits.
    List {
        /// Include retired definitions.
        #[arg(long)]
        include_retired: bool,
    },
    /// View one definition and its ordered options.
    View {
        /// Stable Attribute key.
        key: String,
    },
    /// Create a governed Attribute definition.
    Create {
        /// Stable machine key (immutable after creation).
        #[arg(long)]
        key: String,
        /// Display name.
        #[arg(long)]
        name: String,
        /// Supported Attribute type.
        #[arg(long = "type", value_enum)]
        attribute_type: AttributeTypeArg,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Rename a definition without changing its stable key or type.
    Rename {
        /// Stable Attribute key.
        key: String,
        /// New display name.
        #[arg(long)]
        name: String,
        /// Optional audit reason.
        #[arg(long)]
        reason: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Change a definition's lifecycle state.
    Transition {
        /// Stable Attribute key.
        key: String,
        /// Target lifecycle state.
        #[arg(long = "state", value_enum)]
        target: AttributeStateArg,
        /// Optional audit reason.
        #[arg(long)]
        reason: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Manage select options.
    Option(AttributeOptionArgs),
    /// View or change enablement for one Project only.
    Project(AttributeProjectArgs),
}

/// Arguments for `attribute option`.
#[derive(Args, Debug)]
pub struct AttributeOptionArgs {
    /// The option subcommand to run.
    #[command(subcommand)]
    pub command: AttributeOptionCommand,
}

/// Select-option operations, addressed by stable keys.
#[derive(Subcommand, Debug)]
pub enum AttributeOptionCommand {
    /// Add an option to a select definition.
    Add {
        /// Stable Attribute key.
        attribute_key: String,
        /// Stable option key (immutable after creation).
        #[arg(long)]
        key: String,
        /// Display label.
        #[arg(long)]
        label: String,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Rename an option's display label without changing its key.
    Rename {
        /// Stable Attribute key.
        attribute_key: String,
        /// Stable option key.
        option_key: String,
        /// New display label.
        #[arg(long)]
        label: String,
        /// Optional audit reason.
        #[arg(long)]
        reason: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Set the complete option order, including retired options.
    Reorder {
        /// Stable Attribute key.
        attribute_key: String,
        /// Complete comma-separated sequence of stable option keys.
        #[arg(long = "option-keys", value_delimiter = ',', num_args = 1..)]
        option_keys: Vec<String>,
        /// Optional audit reason.
        #[arg(long)]
        reason: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Retire an option while preserving already assigned values.
    Retire {
        /// Stable Attribute key.
        attribute_key: String,
        /// Stable option key.
        option_key: String,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
}

/// Arguments for `attribute project`.
#[derive(Args, Debug)]
pub struct AttributeProjectArgs {
    /// The Project enablement subcommand to run.
    #[command(subcommand)]
    pub command: AttributeProjectCommand,
}

/// Project-scoped Attribute discovery and enablement.
#[derive(Subcommand, Debug)]
pub enum AttributeProjectCommand {
    /// List definitions available to, and enabled on, the selected Project.
    List,
    /// Enable one Attribute for the selected Project.
    Enable {
        /// Stable Attribute key.
        key: String,
        /// Optional audit reason.
        #[arg(long)]
        reason: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Opt out of one Attribute for the selected Project; existing assignments remain.
    Disable {
        /// Stable Attribute key.
        key: String,
        /// Optional audit reason.
        #[arg(long)]
        reason: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
}

/// Public API v1 Attribute types.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributeTypeArg {
    /// Single-select Attribute.
    #[value(name = "single_select")]
    SingleSelect,
    /// Multi-select Attribute.
    #[value(name = "multi_select")]
    MultiSelect,
    /// Boolean Attribute.
    Boolean,
}

/// Public API v1 Attribute lifecycle states.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributeStateArg {
    /// Accept assignments when enabled on a Project.
    Active,
    /// Temporarily prevent new assignments.
    Disabled,
    /// Permanently retire this definition.
    Retired,
}

/// Shared list pagination and pipeline options.
///
/// `--limit` bounds the *result* (total items emitted), `--cursor` / its
/// `--since-cursor` alias names the opaque server cursor the result starts
/// after, and `--all` follows cursors to the end. They compose: a bounded,
/// resumable stream is `--since-cursor <checkpoint> --all --limit <N>`.
#[derive(Args, Debug, Clone)]
pub struct PaginationArgs {
    /// Maximum total items to emit, counted across pages (distinct from the
    /// server page size). Without `--all` at most one page is returned, holding
    /// at most this many items; a page is never requested larger than the
    /// endpoint allows (200 items, or 100 on organization, project, sprint,
    /// label, and link endpoints). With `--all` successive pages are followed
    /// until this cap or the end of the collection. A cap that cuts through a
    /// server page ends the run with `page.nextCursor: null`, because the Public
    /// API has no cursor for a position partway through a page.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: Option<u32>,
    /// Opaque cursor returned by a preceding page; the result starts after it.
    /// `--since-cursor` is the pipeline-checkpoint spelling of the same option:
    /// resume an interrupted stream from the `page.nextCursor` a previous run
    /// reported. Cursors are opaque and are always forwarded verbatim.
    #[arg(
        long,
        visible_alias = "since-cursor",
        value_name = "CURSOR",
        value_parser = clap::builder::NonEmptyStringValueParser::new()
    )]
    pub cursor: Option<String>,
    /// Follow all pages.
    #[arg(long)]
    pub all: bool,
}

impl PaginationArgs {
    /// Largest page size the Public API accepts on list endpoints.
    pub const MAX_PAGE_SIZE: u32 = 200;

    /// Per-request page size for these options.
    ///
    /// `--limit` is a result cap, so the page size never exceeds the endpoint
    /// maximum; below it the two coincide and one request satisfies the cap.
    #[must_use]
    pub fn page_size(&self) -> Option<u32> {
        self.limit.map(|n| n.clamp(1, Self::MAX_PAGE_SIZE))
    }

    /// Largest page size the organization-scoped directory endpoints accept
    /// (organizations, projects, sprints, labels, links), which the contract
    /// caps below [`Self::MAX_PAGE_SIZE`].
    pub const DIRECTORY_PAGE_SIZE: u32 = 100;

    /// Per-request page size for the directory endpoints, which are capped
    /// lower than [`Self::MAX_PAGE_SIZE`]. A larger `--limit` is still honored
    /// as a total cap by paging.
    #[must_use]
    pub fn directory_page_size(&self) -> Option<u32> {
        self.limit.map(|n| n.clamp(1, Self::DIRECTORY_PAGE_SIZE))
    }

    /// Total result cap requested by `--limit`, if any.
    #[must_use]
    pub fn max_items(&self) -> Option<usize> {
        self.limit.map(|n| usize::try_from(n).unwrap_or(usize::MAX))
    }
}

/// Arguments for the `work` command group.
#[derive(Args, Debug)]
pub struct WorkArgs {
    /// The work subcommand to run.
    #[command(subcommand)]
    pub command: WorkCommand,
}

/// Arguments for the `user` command group.
#[derive(Args, Debug)]
pub struct UserArgs {
    /// The user subcommand to run.
    #[command(subcommand)]
    pub command: UserCommand,
}

/// User profile subcommands.
#[derive(Subcommand, Debug)]
pub enum UserCommand {
    /// View a user's profile summary.
    View {
        /// The user's public ID (usr_...).
        public_id: String,
    },
    /// List work items involving a user.
    Work(Box<UserWorkArgs>),
    /// Show a user's visible activity (newest first).
    Activity {
        /// The user's public ID (usr_...).
        public_id: String,
        /// Only events strictly after this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
        /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
        /// is read in the host's local time zone.
        #[arg(long, value_name = "DATE")]
        since: Option<TimeArg>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// Download a user's avatar.
    Avatar {
        /// The user's public ID (usr_...).
        public_id: String,
        /// Output path (defaults to the public ID with a content-type
        /// derived extension).
        #[arg(long = "output", value_name = "PATH", short = 'o')]
        output: Option<String>,
        /// Opaque avatar version cache selector.
        #[arg(long = "avatar-version", value_name = "VALUE")]
        avatar_version: Option<String>,
        /// Opaque avatar format selector.
        #[arg(id = "image_format", long = "image-format", value_name = "VALUE")]
        format: Option<String>,
        /// Opaque avatar revision cache selector.
        #[arg(long = "revision", value_name = "VALUE")]
        revision: Option<String>,
    },
}

/// Complete filters for `user work`.
#[derive(Args, Debug)]
pub struct UserWorkArgs {
    /// The user's public ID (usr_...).
    pub public_id: String,
    /// Restrict to involvement kind (repeatable).
    #[arg(long, value_enum)]
    pub involvement: Vec<InvolvementArg>,
    /// Filter by organization slug (repeatable).
    #[arg(long)]
    pub org: Vec<String>,
    /// Filter by project key (repeatable).
    #[arg(long)]
    pub project: Vec<String>,
    /// Free-text search.
    #[arg(long = "search", value_name = "TEXT")]
    pub search: Option<String>,
    /// Filter by status (repeatable).
    #[arg(long, value_enum)]
    pub status: Vec<StatusArg>,
    /// Status scope.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeArg>,
    /// Filter by type (repeatable).
    #[arg(long = "type", value_enum)]
    pub item_type: Vec<TypeArg>,
    /// Filter by priority (repeatable).
    #[arg(long, value_enum)]
    pub priority: Vec<PriorityArg>,
    /// Filter by sprint id or `none`.
    #[arg(long, value_name = "NONE|UUID")]
    pub sprint: Option<String>,
    /// Filter by label id (repeatable).
    #[arg(long)]
    pub label: Vec<String>,
    /// Filter by label name (repeatable).
    #[arg(long = "label-name")]
    pub label_name: Vec<String>,
    /// Filter by parent Work Item key.
    #[arg(long)]
    pub parent: Option<String>,
    /// Filter by top-level state (a bare flag means true).
    #[arg(
        long = "top-level",
        value_name = "true|false",
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    pub top_level: Option<bool>,
    /// Only items updated after this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
    /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
    /// is read in the host's local time zone.
    #[arg(long = "updated-after", value_name = "DATE")]
    pub updated_after: Option<TimeArg>,
    /// Filter to overdue/on-track Work Items.
    #[arg(long, value_name = "true|false")]
    pub overdue: Option<bool>,
    /// Only items due strictly before this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
    /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
    /// is read in the host's local time zone.
    #[arg(long = "due-before", value_name = "DATE")]
    pub due_before: Option<TimeArg>,
    /// Only items due strictly after this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
    /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
    /// is read in the host's local time zone.
    #[arg(long = "due-after", value_name = "DATE")]
    pub due_after: Option<TimeArg>,
    /// Result ordering: updated, dueDate, priority, or rank, optionally with
    /// a :asc/:desc direction (for example `--sort dueDate:desc`).
    #[arg(long, value_name = "KEY[:DIR]", value_parser = SortValueParser)]
    pub sort: Option<SortArg>,
    /// Only archived (true) or only unarchived (false) Work Items; when
    /// omitted, unarchived Work Items are listed. Listing both states in one
    /// result requires separate queries.
    #[arg(long, value_name = "true|false")]
    pub archived: Option<bool>,
    /// Comma-separated sparse summary fields.
    #[arg(long = "fields", value_name = "FIELDS")]
    pub fields: Option<String>,
    /// Pagination options.
    #[command(flatten)]
    pub pagination: PaginationArgs,
}

/// Profile work involvement kinds.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum InvolvementArg {
    /// Items currently assigned to the user.
    Assigned,
    /// Items created (reported) by the user.
    Created,
    /// Items the user commented on.
    Commented,
}

impl InvolvementArg {
    /// The wire value for this involvement kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            InvolvementArg::Assigned => "assigned",
            InvolvementArg::Created => "created",
            InvolvementArg::Commented => "commented",
        }
    }
}

/// Arguments for `work view`.
#[derive(Args, Debug)]
pub struct WorkViewArgs {
    /// Work item keys (e.g. HAM-42). Accepts multiple keys for one
    /// batch read (up to 500 per invocation).
    #[arg(value_name = "KEY", num_args = 1..)]
    pub keys: Vec<String>,

    /// Read keys from a file (`-` for stdin), one key per line. Blank
    /// lines and lines starting with `#` are ignored. Up to 500 keys;
    /// input is capped at 1 MiB.
    #[arg(long, value_name = "PATH", conflicts_with = "keys")]
    pub file: Option<String>,

    /// Maximum comments included per item (0 omits the section).
    /// Batch reads only (two or more keys); use `work context` for one
    /// item with sections.
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub comments: u32,

    /// Maximum activity events included per item (0 omits the section).
    /// Batch reads only (two or more keys); use `work context` for one
    /// item with sections.
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub activity: u32,

    /// Maximum links included per item (0 omits the section). Batch reads
    /// only (two or more keys); use `work context` for one item with
    /// sections.
    #[arg(long, value_name = "N", default_value_t = 0)]
    pub links: u32,

    /// Omit long text bodies (description, comment bodies) — each replaced
    /// by an explicit truncated marker.
    #[arg(long)]
    pub compact: bool,
}

/// Default `work tree --depth` (descendant levels below the root).
pub const WORK_TREE_DEFAULT_DEPTH: u32 = 3;
/// Hard cap on `work tree --depth`; deeper requests are a usage error.
pub const WORK_TREE_MAX_DEPTH: u32 = 10;
/// Default `work tree --max-nodes` (total nodes rendered).
pub const WORK_TREE_DEFAULT_MAX_NODES: u32 = 200;
/// Hard cap on `work tree --max-nodes`; larger requests are a usage error.
pub const WORK_TREE_MAX_NODES: u32 = 2000;

/// Arguments for `work tree`.
///
/// The JSON document is stable and versioned:
///
/// ```json
/// {
///   "treeVersion": 1,
///   "organization": "acme",
///   "project": "HAM",
///   "depth": 3,
///   "maxNodes": 200,
///   "truncated": false,
///   "root": {
///     "key": "HAM-42", "title": "…", "type": "epic",
///     "status": "backlog", "priority": "medium",
///     "parent": null,
///     "children": [ …nodes… ],
///     "truncated": []
///   }
/// }
///
/// Each node carries the server-reported `status`/`priority` verbatim (the
/// CLI computes no workflow meaning) and a `truncated` array whose entries
/// are `{"reason": "depth"|"nodes"|"cycle", "message": "…"}`. When
/// `--links` is passed each node also carries a `links` array of the raw
/// server link objects; the key is absent otherwise.
#[derive(Args, Debug)]
pub struct WorkTreeArgs {
    /// Root work item key (e.g. HAM-42).
    #[arg(value_name = "KEY")]
    pub key: String,

    /// Maximum descendant levels to expand below the root (1 shows direct
    /// children only). Deeper structures end with an explicit depth-limit
    /// marker; the value is capped at 10.
    #[arg(
        long,
        value_name = "N",
        default_value_t = WORK_TREE_DEFAULT_DEPTH,
        value_parser = clap::value_parser!(u32).range(1..=WORK_TREE_MAX_DEPTH as i64)
    )]
    pub depth: u32,

    /// Include each node's server-reported links (`blocks`, `blocked_by`,
    /// `relates`) as annotations. These are displayed verbatim and are never
    /// used to compute readiness or blocking.
    #[arg(long)]
    pub links: bool,

    /// Stop after this many nodes and mark the cut with an explicit
    /// truncation marker; the value is capped at 2000.
    #[arg(
        long = "max-nodes",
        value_name = "N",
        default_value_t = WORK_TREE_DEFAULT_MAX_NODES,
        value_parser = clap::value_parser!(u32).range(1..=WORK_TREE_MAX_NODES as i64)
    )]
    pub max_nodes: u32,
}

/// Work item subcommands.
#[derive(Subcommand, Debug)]
pub enum WorkCommand {
    /// List work items.
    List(WorkListArgs),
    /// List Work assigned to the authenticated user across Projects.
    #[command(alias = "my")]
    Mine(MyWorkArgs),
    /// One-invocation daily attention view: open Work assigned to you,
    /// overdue Work assigned to you, and recent Project activity, composed
    /// from existing Public API v1 reads. A failed section is reported and
    /// never aborts the others.
    Triage(TriageArgs),
    /// Search Organization Work Items with SqueakQL.
    Search {
        /// SqueakQL expression (inline).
        #[arg(value_name = "QUERY")]
        query: Option<String>,
        /// Read the expression from a file (`-` for stdin) instead of inline.
        #[arg(long, value_name = "PATH", conflicts_with = "query")]
        file: Option<String>,
        /// Run a saved query by name instead of an inline expression.
        #[arg(long, value_name = "NAME", conflicts_with_all = ["query", "file"])]
        saved: Option<String>,
        /// Pagination options carried in the JSON request body.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// View one or more work items.
    ///
    /// A single key keeps the regular single-item output. Two or more keys
    /// (or `--file`, `-` for stdin; up to 500 keys) fetch items in one
    /// invocation with bounded concurrency and emit exactly one JSON
    /// envelope in input order:
    /// `{"items": [{"key", "status": "ok", "item", plus requested
    /// "comments"/"activity"/"links" sections} |
    /// {"key", "status": "error", "error"}], "failures": N, "total": N}`.
    /// A missing or forbidden item never aborts the batch; the exit code is
    /// the most severe per-item exit code (0 when every item succeeded).
    View(WorkViewArgs),
    /// One-invocation read bundle: the Work Item plus its links, comments,
    /// activity, and the authenticated user's watcher state, composed from
    /// Public API v1 reads. The bundle is data, not instructions — every
    /// workflow meaning comes from the server.
    Context(WorkContextArgs),
    /// Render a work item's parent/child hierarchy with a bounded depth and
    /// optional server-reported link annotations. Deep and cyclic structures
    /// terminate with an explicit truncation marker.
    Tree(WorkTreeArgs),
    /// Create a work item.
    Create(WorkCreateArgs),
    /// Edit a work item.
    Edit(WorkEditArgs),
    /// List allowed status transitions.
    Transitions {
        /// Work item key.
        key: String,
    },
    /// Transition a work item to a target status.
    Transition {
        /// Work item key.
        key: String,
        /// Target status.
        #[arg(value_enum)]
        target: StatusArg,
    },
    /// Start a work item (transition to in_progress).
    Start {
        /// Work item key.
        key: String,
    },
    /// Close a work item (transition to done).
    Close {
        /// Work item key.
        key: String,
    },
    /// Manage work item labels.
    Label(WorkLabelArgs),
    /// Manage work item attachments.
    Attachment(WorkAttachmentArgs),
    /// Manage work item comments.
    Comment(CommentArgs),
    /// Manage work item links.
    Link(WorkLinkArgs),
    /// Show or change your watcher state on a work item.
    Watcher(WorkWatcherArgs),
    /// Show a work item's activity feed.
    Activity {
        /// Work item key.
        key: String,
        /// Only events strictly after this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
        /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
        /// is read in the host's local time zone.
        #[arg(long, value_name = "DATE")]
        since: Option<TimeArg>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// Archive a work item (Organization administrators).
    Archive {
        /// Work item key.
        key: String,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Unarchive a work item (Organization administrators).
    Unarchive {
        /// Work item key.
        key: String,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Soft-delete a work item (Organization owners only).
    Delete {
        /// Work item key.
        key: String,
        /// Also soft-delete the entire descendant tree.
        #[arg(long)]
        cascade: bool,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Wait for a work item to reach a server-reported condition.
    ///
    /// `await` blocks until a single condition holds and then exits. To follow
    /// an item's activity and comments over time instead, use `work watch`.
    Await(WorkAwaitArgs),
    /// Follow a work item's activity and comments, rendering new entries as
    /// they appear.
    ///
    /// Unlike `work await`, which waits for a single server-reported condition
    /// and exits, `watch` polls the existing activity and comment endpoints and
    /// streams every new entry until interrupted (Ctrl-C). Network failures
    /// back off and reconnect; authentication and authorization failures stop
    /// the watch.
    Watch(WorkWatchArgs),
    /// Create, update, or transition many work items in one request.
    Bulk(WorkBulkArgs),
    /// Export a work item as a portable Markdown document.
    ///
    /// The document is YAML frontmatter (`key`, `title`, `type`, `status`,
    /// `priority`, `labels`, and optionally `links`/`comments`) plus the
    /// description as the Markdown body. It is suitable for pasting into a
    /// GitHub/GitLab issue or handing work to another tracker. `--format
    /// markdown` (the default) writes the document; `--json` emits the same
    /// fields as a structured envelope.
    Export(WorkExportArgs),
    /// Create or update work items from an exported Markdown document.
    ///
    /// Reads the format `work export` writes and maps its fields onto the
    /// existing create/edit operations. When the embedded `key` already
    /// resolves, the item is updated in place; otherwise a new item is created
    /// with an idempotency key derived from the embedded key, so re-importing
    /// the same document does not duplicate items. `--dry-run` previews the
    /// mapped operations without sending them.
    Import(WorkImportArgs),
}

/// Arguments for `work export`.
#[derive(Args, Debug)]
pub struct WorkExportArgs {
    /// Work item key.
    pub key: String,
    /// Include the item's comments in the exported frontmatter.
    #[arg(long)]
    pub comments: bool,
    /// Write the document to a file instead of stdout.
    #[arg(long = "output", value_name = "PATH", short = 'o')]
    pub output: Option<String>,
}

/// Arguments for `work import`.
#[derive(Args, Debug)]
pub struct WorkImportArgs {
    /// Exported Markdown document (path, or `-` for stdin).
    #[arg(long, value_name = "PATH")]
    pub file: String,
    /// Explicit idempotency key for the create. Overrides the key derived from
    /// the document's embedded `key`; the derived key keeps re-imports from
    /// duplicating items.
    #[arg(long = "idempotency-key", value_name = "KEY")]
    pub idempotency_key: Option<String>,
}

/// Filters accepted by the authenticated user's My Work endpoint.
#[derive(Args, Debug)]
pub struct MyWorkArgs {
    /// Filter by Project key (repeatable).
    #[arg(long)]
    pub project: Vec<String>,
    /// Filter by status (repeatable).
    #[arg(long, value_enum)]
    pub status: Vec<StatusArg>,
    /// Status scope.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeArg>,
    /// Filter by type (repeatable).
    #[arg(long = "type", value_enum)]
    pub item_type: Vec<TypeArg>,
    /// Filter by priority (repeatable).
    #[arg(long, value_enum)]
    pub priority: Vec<PriorityArg>,
    /// Filter by label id (repeatable).
    #[arg(long)]
    pub label: Vec<String>,
    /// Filter by label name (repeatable).
    #[arg(long = "label-name")]
    pub label_name: Vec<String>,
    /// Filter to overdue/on-track Work Items.
    #[arg(long, value_name = "true|false")]
    pub overdue: Option<bool>,
    /// Only items due strictly before this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
    /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
    /// is read in the host's local time zone.
    #[arg(long = "due-before", value_name = "DATE")]
    pub due_before: Option<TimeArg>,
    /// Only items due strictly after this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
    /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
    /// is read in the host's local time zone.
    #[arg(long = "due-after", value_name = "DATE")]
    pub due_after: Option<TimeArg>,
    /// Result ordering: updated, dueDate, priority, or rank, optionally with
    /// a :asc/:desc direction (for example `--sort dueDate:desc`).
    #[arg(long, value_name = "KEY[:DIR]", value_parser = SortValueParser)]
    pub sort: Option<SortArg>,
    /// Only archived (true) or only unarchived (false) Work Items; when
    /// omitted, unarchived Work Items are listed. Listing both states in one
    /// result requires separate queries.
    #[arg(long, value_name = "true|false")]
    pub archived: Option<bool>,
    /// Comma-separated sparse summary fields.
    #[arg(long = "fields", value_name = "FIELDS")]
    pub fields: Option<String>,
    /// Pagination options.
    #[command(flatten)]
    pub pagination: PaginationArgs,
}

/// Arguments for `work triage`.
///
/// The Work Item sections (`assignedOpen`, `overdue`) use the shared
/// pagination flags; the `activity` section is bounded separately by
/// `--activity` and optionally windowed by `--since`.
#[derive(Args, Debug)]
pub struct TriageArgs {
    /// Pagination options for the Work Item sections (assigned and overdue).
    #[command(flatten)]
    pub pagination: PaginationArgs,
    /// Maximum recent Project activity events to include (0 disables the
    /// section; the Project feed is the only activity source Public API v1
    /// exposes).
    #[arg(long = "activity", value_name = "N", default_value_t = 10)]
    pub activity: u32,
    /// Only activity strictly after this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
    /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
    /// is read in the host's local time zone.
    #[arg(long, value_name = "DATE")]
    pub since: Option<TimeArg>,
}

/// Arguments for `work await`.
#[derive(Args, Debug)]
pub struct WorkAwaitArgs {
    /// Work item key (e.g. HAM-42).
    #[arg(value_name = "KEY")]
    pub key: String,
    /// Target status to wait for (repeatable; all specified conditions must
    /// match). When omitted, the command returns immediately once the item is
    /// reachable (any status satisfies the condition).
    #[arg(long = "status", value_name = "STATUS", value_enum)]
    pub status: Vec<StatusArg>,
    /// Maximum time to wait (e.g. 10m, 1h30m). Defaults to 5m; upper-bounded
    /// at 1h to prevent accidental infinite waits.
    #[arg(long = "timeout", value_name = "DURATION", default_value = "5m")]
    pub timeout: String,
}

/// Arguments for `work watch`.
///
/// `watch` polls existing Public API v1 activity and comment reads; it is not
/// a server push channel and never synthesizes events. Every rendered entry is
/// the server's own payload.
#[derive(Args, Debug)]
#[command(
    long_about = "Follow a Work Item's activity and comments until interrupted.\n\nUnlike `work await`, which blocks until a single server-reported condition is\nmet and exits, `watch` polls the existing activity and comment endpoints and\nrenders every new entry incrementally. Polling uses a fixed interval with\ngraceful backoff on network failures and rate limits; authentication and\nauthorization failures stop the watch cleanly. With `--json`/`--jsonl` the\ncommand emits one JSON object per line as an event stream."
)]
pub struct WorkWatchArgs {
    /// Work item key (e.g. HAM-42).
    #[arg(value_name = "KEY")]
    pub key: String,
    /// Only stream entries strictly after this time. Accepts RFC 3339,
    /// `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a relative offset such
    /// as `7d`, `2w`, or `+3h`; input without a UTC offset is read in the
    /// host's local time zone. When omitted, the watch starts at the current
    /// instant and only new entries are streamed.
    #[arg(long, value_name = "DATE")]
    pub since: Option<TimeArg>,
    /// Poll interval (e.g. 2s, 500ms). Defaults to 2s; bounded to 100ms–1h.
    #[arg(long, value_name = "DURATION", default_value = "2s")]
    pub interval: String,
    /// Shell command to run once for each new entry. The entry is exposed to
    /// the command through `HAMSTIK_WATCH_ITEM`, `HAMSTIK_WATCH_TYPE`,
    /// `HAMSTIK_WATCH_ID`, `HAMSTIK_WATCH_ACTION` (activity only), and
    /// `HAMSTIK_WATCH_JSON` environment variables. A failing hook is reported
    /// and never stops the watch.
    #[arg(long, value_name = "COMMAND")]
    pub notify: Option<String>,
}

/// Work Item filter options shared by `work list` and `org work`.
#[derive(Args, Debug)]
pub struct WorkFilters {
    /// Free-text search.
    #[arg(long = "search", value_name = "TEXT")]
    pub search: Option<String>,
    /// Filter by status (repeatable).
    #[arg(long, value_enum)]
    pub status: Vec<StatusArg>,
    /// Status scope.
    #[arg(long, value_enum)]
    pub scope: Option<ScopeArg>,
    /// Filter by type (repeatable).
    #[arg(long = "type", value_enum)]
    pub item_type: Vec<TypeArg>,
    /// Filter by priority (repeatable).
    #[arg(long, value_enum)]
    pub priority: Vec<PriorityArg>,
    /// Filter by assignee: me, none, a user UUID, or a public ID (usr_...).
    #[arg(long, value_name = "ME|NONE|ID")]
    pub assignee: Option<String>,
    /// Filter by sprint: none or a sprint UUID.
    #[arg(long, value_name = "NONE|UUID")]
    pub sprint: Option<String>,
    /// Filter by label UUID (repeatable).
    #[arg(long)]
    pub label: Vec<String>,
    /// Filter by label name (repeatable).
    #[arg(long = "label-name")]
    pub label_name: Vec<String>,
    /// Filter by parent work item key.
    #[arg(long)]
    pub parent: Option<String>,
    /// Filter by top-level state (a bare flag means true).
    #[arg(
        long = "top-level",
        value_name = "true|false",
        num_args = 0..=1,
        default_missing_value = "true"
    )]
    pub top_level: Option<bool>,
    /// Only items updated at or after this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
    /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
    /// is read in the host's local time zone.
    #[arg(long = "updated-after", value_name = "DATE")]
    pub updated_after: Option<TimeArg>,
    /// Only overdue items (true) or only on-track items (false).
    #[arg(long, value_name = "true|false")]
    pub overdue: Option<bool>,
    /// Only items due strictly before this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
    /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
    /// is read in the host's local time zone.
    #[arg(long = "due-before", value_name = "DATE")]
    pub due_before: Option<TimeArg>,
    /// Only items due strictly after this time. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
    /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
    /// is read in the host's local time zone.
    #[arg(long = "due-after", value_name = "DATE")]
    pub due_after: Option<TimeArg>,
    /// Result ordering: updated, dueDate, priority, or rank, optionally with
    /// a :asc/:desc direction (for example `--sort dueDate:desc`).
    #[arg(long, value_name = "KEY[:DIR]", value_parser = SortValueParser)]
    pub sort: Option<SortArg>,
    /// Only archived (true) or only unarchived (false) items; when omitted,
    /// unarchived items are listed. Listing both states in one result
    /// requires separate queries.
    #[arg(long, value_name = "true|false")]
    pub archived: Option<bool>,
    /// Comma-separated summary fields; an empty value selects all fields.
    #[arg(long = "fields", value_name = "FIELDS")]
    pub fields: Option<String>,
    /// Shorthand for --assignee me.
    #[arg(long)]
    pub mine: bool,
}

impl WorkFilters {
    /// Copies the filter selections into a [`ListWorkItemsQuery`]-shaped
    /// destination closure-wise; used by collection commands.
    pub fn assignee_query(&self) -> Option<String> {
        self.assignee.clone().or_else(|| {
            if self.mine {
                Some("me".to_string())
            } else {
                None
            }
        })
    }
}

/// Arguments for `work list`.
#[derive(Args, Debug)]
pub struct WorkListArgs {
    /// Shared Work Item filters.
    #[command(flatten)]
    pub filters: WorkFilters,
    /// Pagination options.
    #[command(flatten)]
    pub pagination: PaginationArgs,
}

/// Arguments for `work create`.
#[derive(Args, Debug)]
pub struct WorkCreateArgs {
    /// Work item title. Required unless `--from`/`--template` supplies one.
    #[arg(long)]
    pub title: Option<String>,
    /// Copy title/type/priority/description/labels from an existing Work Item
    /// as a starting point; explicit flags override the copied values.
    #[arg(long = "from", value_name = "KEY", conflicts_with = "template")]
    pub from: Option<String>,
    /// Read a local Markdown file with YAML frontmatter as a reusable starting
    /// point; explicit flags override the template's values (`-` reads stdin).
    #[arg(long = "template", value_name = "FILE", conflicts_with = "from")]
    pub template: Option<String>,
    /// Description text.
    #[arg(long, conflicts_with_all = ["description_file", "description_editor"])]
    pub description: Option<String>,
    /// Description source (path, or - for stdin).
    #[arg(
        long = "description-file",
        value_name = "PATH",
        conflicts_with_all = ["description", "description_editor"]
    )]
    pub description_file: Option<String>,
    /// Author the description in $VISUAL/$EDITOR instead of passing text.
    #[arg(long = "description-editor", conflicts_with_all = ["description", "description_file"])]
    pub description_editor: bool,
    /// Work item type.
    #[arg(long = "type", value_enum)]
    pub item_type: Option<TypeArg>,
    /// Initial status.
    #[arg(long, value_enum)]
    pub status: Option<StatusArg>,
    /// Priority.
    #[arg(long, value_enum)]
    pub priority: Option<PriorityArg>,
    /// Assignee user id (or `me`).
    #[arg(long, value_name = "PUBLIC_ID|me")]
    pub assignee: Option<String>,
    /// Sprint id.
    #[arg(long, value_name = "SPRINT")]
    pub sprint: Option<String>,
    /// Parent work item key (or parent UUID).
    #[arg(long, value_name = "KEY")]
    pub parent: Option<String>,
    /// Story point estimate.
    #[arg(long = "story-points", value_name = "N")]
    pub story_points: Option<i64>,
    /// Due date. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
    /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
    /// is read in the host's local time zone.
    #[arg(long = "due-date", value_name = "DATE")]
    pub due_date: Option<TimeArg>,
    /// Assign a select option as ATTRIBUTE_KEY=OPTION_KEY (repeat the key for
    /// multi-select values).
    #[arg(long = "attribute-option", value_name = "ATTRIBUTE_KEY=OPTION_KEY")]
    pub attribute_options: Vec<String>,
    /// Assign a boolean Attribute as ATTRIBUTE_KEY=true|false.
    #[arg(long = "attribute-boolean", value_name = "ATTRIBUTE_KEY=true|false")]
    pub attribute_booleans: Vec<String>,
    /// Explicit idempotency key.
    #[arg(long = "idempotency-key", value_name = "KEY")]
    pub idempotency_key: Option<String>,
}

/// Arguments for `work edit`.
#[derive(Args, Debug)]
pub struct WorkEditArgs {
    /// Work item key.
    pub key: String,
    /// New title.
    #[arg(long)]
    pub title: Option<String>,
    /// New description text.
    #[arg(
        long,
        conflicts_with_all = ["clear_description", "description_file", "description_editor"]
    )]
    pub description: Option<String>,
    /// New description source (path, or - for stdin).
    #[arg(
        long = "description-file",
        value_name = "PATH",
        conflicts_with_all = ["description", "clear_description", "description_editor"]
    )]
    pub description_file: Option<String>,
    /// Author the description in $VISUAL/$EDITOR instead of passing text.
    #[arg(
        long = "description-editor",
        conflicts_with_all = ["description", "description_file", "clear_description"]
    )]
    pub description_editor: bool,
    /// New work item type.
    #[arg(long = "type", value_enum)]
    pub item_type: Option<TypeArg>,
    /// New priority.
    #[arg(long, value_enum)]
    pub priority: Option<PriorityArg>,
    /// New assignee (or `me`).
    #[arg(long, value_name = "PUBLIC_ID|me", conflicts_with = "clear_assignee")]
    pub assignee: Option<String>,
    /// Sprint id.
    #[arg(long, value_name = "SPRINT", conflicts_with = "clear_sprint")]
    pub sprint: Option<String>,
    /// New parent key (or parent UUID).
    #[arg(long, value_name = "KEY", conflicts_with = "clear_parent")]
    pub parent: Option<String>,
    #[arg(
        long = "story-points",
        value_name = "N",
        conflicts_with = "clear_story_points"
    )]
    /// New story point estimate.
    pub story_points: Option<i64>,
    #[arg(
        long = "due-date",
        value_name = "DATE",
        conflicts_with = "clear_due_date"
    )]
    /// New due date. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
    /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
    /// is read in the host's local time zone.
    pub due_date: Option<TimeArg>,
    /// Set a select option as ATTRIBUTE_KEY=OPTION_KEY (repeat the key for
    /// multi-select values).
    #[arg(long = "attribute-option", value_name = "ATTRIBUTE_KEY=OPTION_KEY")]
    pub attribute_options: Vec<String>,
    /// Set a boolean Attribute as ATTRIBUTE_KEY=true|false.
    #[arg(long = "attribute-boolean", value_name = "ATTRIBUTE_KEY=true|false")]
    pub attribute_booleans: Vec<String>,
    /// Explicitly clear an existing Attribute assignment.
    #[arg(long = "clear-attribute", value_name = "ATTRIBUTE_KEY")]
    pub clear_attributes: Vec<String>,
    /// Clear the description.
    #[arg(long = "clear-description")]
    pub clear_description: bool,
    /// Unassign the item.
    #[arg(long = "clear-assignee")]
    pub clear_assignee: bool,
    /// Remove the sprint.
    #[arg(long = "clear-sprint")]
    pub clear_sprint: bool,
    /// Detach the parent.
    #[arg(long = "clear-parent")]
    pub clear_parent: bool,
    /// Clear the story point estimate.
    #[arg(long = "clear-story-points")]
    pub clear_story_points: bool,
    /// Clear the due date.
    #[arg(long = "clear-due-date")]
    pub clear_due_date: bool,
    /// Bypass revision conflict protection (If-Match: *).
    #[arg(long)]
    pub force: bool,
    /// Explicit idempotency key (used for Attribute-bearing updates).
    #[arg(long = "idempotency-key", value_name = "KEY")]
    pub idempotency_key: Option<String>,
}

/// Arguments for the `work comment` command group.
#[derive(Args, Debug)]
pub struct CommentArgs {
    /// The comment subcommand to run.
    #[command(subcommand)]
    pub command: CommentCommand,
}

/// Comment subcommands.
#[derive(Subcommand, Debug)]
pub enum CommentCommand {
    /// List comments on a work item.
    List {
        /// Work item key.
        key: String,
        /// Hide soft-deleted comments instead of rendering `(deleted)`
        /// placeholders.
        #[arg(long = "exclude-deleted")]
        exclude_deleted: bool,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// Add a comment to a work item.
    Add {
        /// Work item key.
        key: String,
        /// Comment body.
        #[arg(long, conflicts_with_all = ["body_file", "body_editor"])]
        body: Option<String>,
        /// Comment body source (path, or - for stdin).
        #[arg(
            long = "body-file",
            value_name = "PATH",
            conflicts_with_all = ["body", "body_editor"]
        )]
        body_file: Option<String>,
        /// Author the body in $VISUAL/$EDITOR instead of passing text.
        #[arg(long = "body-editor", conflicts_with_all = ["body", "body_file"])]
        body_editor: bool,
        /// Parent comment UUID (for replies).
        #[arg(long, value_name = "UUID")]
        parent: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Edit your own comment.
    Edit {
        /// Work item key.
        key: String,
        /// Comment id (UUID).
        comment_id: String,
        /// Replacement body.
        #[arg(long, conflicts_with_all = ["body_file", "body_editor"])]
        body: Option<String>,
        /// Replacement body source (path, or - for stdin).
        #[arg(
            long = "body-file",
            value_name = "PATH",
            conflicts_with_all = ["body", "body_editor"]
        )]
        body_file: Option<String>,
        /// Author the body in $VISUAL/$EDITOR instead of passing text.
        #[arg(long = "body-editor", conflicts_with_all = ["body", "body_file"])]
        body_editor: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Delete your own comment (only when it has no replies).
    Delete {
        /// Work item key.
        key: String,
        /// Comment id (UUID).
        comment_id: String,
    },
}

/// Arguments for the `work label` command group.
#[derive(Args, Debug)]
pub struct WorkLabelArgs {
    /// The label subcommand to run.
    #[command(subcommand)]
    pub command: WorkLabelCommand,
}

/// Work item label subcommands.
#[derive(Subcommand, Debug)]
pub enum WorkLabelCommand {
    /// Attach a label to a work item.
    Add {
        /// Work item key.
        key: String,
        /// Label id (UUID) or label name.
        #[arg(long)]
        label: String,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Detach a label from a work item.
    Remove {
        /// Work item key.
        key: String,
        /// Label id (UUID) or label name.
        #[arg(long)]
        label: String,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
}

/// Arguments for the `work attachment` command group.
#[derive(Args, Debug)]
pub struct WorkAttachmentArgs {
    /// The attachment subcommand to run.
    #[command(subcommand)]
    pub command: WorkAttachmentCommand,
}

/// Work item attachment subcommands.
#[derive(Subcommand, Debug)]
pub enum WorkAttachmentCommand {
    /// List attachments on a work item.
    List {
        /// Work item key.
        key: String,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// Upload an attachment.
    Upload {
        /// Work item key.
        key: String,
        /// File to upload, or - for stdin.
        file: String,
        /// File name recorded with the attachment (defaults to the file name).
        #[arg(long = "file-name", value_name = "NAME")]
        file_name: Option<String>,
        /// Content type (defaults to a guess or application/octet-stream).
        #[arg(long = "content-type", value_name = "TYPE")]
        content_type: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Download an attachment to a file.
    Download {
        /// Work item key.
        key: String,
        /// Attachment id (UUID).
        attachment_id: String,
        /// Output path (defaults to the attachment's file name in the current directory).
        #[arg(long = "output", value_name = "PATH", short = 'o')]
        output: Option<String>,
    },
    /// Preview an image attachment inline on a capable terminal.
    ///
    /// When no inline protocol is detected, or under `--json`, `--quiet`,
    /// `--no-input`, or non-TTY output, this falls back to the `download`
    /// behavior and writes the bytes to a file instead.
    View {
        /// Work item key.
        key: String,
        /// Attachment id (UUID).
        attachment_id: String,
        /// Output path used when inline preview is unavailable (defaults to the attachment's file name in the current directory).
        #[arg(long = "output", value_name = "PATH", short = 'o')]
        output: Option<String>,
    },
    /// Delete an attachment (creator or Organization administrator).
    Delete {
        /// Work item key.
        key: String,
        /// Attachment id (UUID).
        attachment_id: String,
    },
}

/// Arguments for the `work link` command group.
#[derive(Args, Debug)]
pub struct WorkLinkArgs {
    /// The link subcommand to run.
    #[command(subcommand)]
    pub command: WorkLinkCommand,
}

/// Work item watcher subcommands (the authenticated user's own state).
#[derive(Subcommand, Debug)]
pub enum WorkWatcherCommand {
    /// Show your watcher state on a work item.
    Show {
        /// Work item key.
        key: String,
    },
    /// Watch a work item (start receiving notifications).
    Watch {
        /// Work item key.
        key: String,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Stop watching a work item.
    Unwatch {
        /// Work item key.
        key: String,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Keep watching but suppress notifications.
    Mute {
        /// Work item key.
        key: String,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Stop suppressing notifications.
    Unmute {
        /// Work item key.
        key: String,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
}

/// Work item watcher arguments.
#[derive(Args, Debug)]
pub struct WorkWatcherArgs {
    /// The watcher subcommand to run.
    #[command(subcommand)]
    pub command: WorkWatcherCommand,
}

/// Work item link subcommands.
#[derive(Subcommand, Debug)]
pub enum WorkLinkCommand {
    /// List links from a work item.
    List {
        /// Work item key.
        key: String,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// Link this work item to another.
    Add {
        /// Work item key.
        key: String,
        /// Target work item key (e.g. HAM-43).
        #[arg(long, value_name = "KEY", conflicts_with = "target_id")]
        target_key: Option<String>,
        /// Target work item id (UUID).
        #[arg(long = "target-id", value_name = "UUID", conflicts_with = "target_key")]
        target_id: Option<String>,
        /// Relation kind.
        #[arg(long, value_enum)]
        relation: RelationArg,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Delete a link by id.
    Delete {
        /// Work item key.
        key: String,
        /// Link id (UUID).
        link_id: String,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
}

/// Arguments for `work bulk`.
#[derive(Args, Debug)]
pub struct WorkBulkArgs {
    /// The bulk subcommand to run.
    #[command(subcommand)]
    pub command: WorkBulkCommand,
}

/// Bulk work item subcommands.
#[derive(Subcommand, Debug)]
pub enum WorkBulkCommand {
    /// Create up to 50 work items in one request.
    Create {
        /// Operations as a JSON array file (path, or - for stdin).
        #[arg(long = "operations-file", value_name = "PATH")]
        operations_file: String,
        /// Project key (overrides context; used to validate access).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Update up to 50 work items in one request.
    Update {
        /// Operations as a JSON array file (path, or - for stdin).
        #[arg(long = "operations-file", value_name = "PATH")]
        operations_file: String,
        /// Concurrency mode: require-revision (default) or last-write-wins.
        #[arg(long, value_enum)]
        concurrency: Option<ConcurrencyArg>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Transition up to 50 work items in one request.
    Transition {
        /// Operations as a JSON array file (path, or - for stdin).
        #[arg(long = "operations-file", value_name = "PATH")]
        operations_file: String,
        /// Concurrency mode: require-revision (default) or last-write-wins.
        #[arg(long, value_enum)]
        concurrency: Option<ConcurrencyArg>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Run a large operation set in resumable batches of at most 50.
    ///
    /// Reads a JSON array or JSON-lines operations file (use `-` for stdin),
    /// preflights every operation, and sends batches of at most 50 in sequence.
    /// Each batch's request body and idempotency key are recorded in a local
    /// journal before any request is sent, so an interrupted run resumes from
    /// the journal without redoing completed batches or duplicating creates.
    /// Failed batches are never retried automatically; pass `--retry-failed`
    /// after reviewing the journal. Separate batches are separate API requests
    /// and are not one atomic transaction.
    Run(WorkBulkRunArgs),
    /// Convert a CSV file into the `work bulk` operations JSON array.
    ///
    /// The first row is a header; every later row becomes one operation, so the
    /// emitted JSON array can be piped straight into
    /// `work bulk create|update --operations-file -`. Fields use RFC 4180
    /// quoting (comma-separated, `"` quoting, `""` for an embedded quote);
    /// free-text values are not trimmed, while enum and identifier cells are
    /// interpreted (and emitted) trimmed. `--project` supplies `projectKey`
    /// for every operation.
    ///
    /// `--op update` columns: `workItemKey` (required), `revision`, `title`,
    /// `description`, `type`, `priority`, `assignee`, `sprint`, `parent`,
    /// `storyPoints`, `dueDate` (the same date/time expressions as
    /// `--due-date`). At least one change column is required. An empty cell
    /// means the field is omitted.
    ///
    /// `--op create` column: `title` (required). The frozen Public API v1 bulk
    /// create envelope carries only `projectKey` and `title`, so no other
    /// column is accepted.
    ///
    /// Unknown columns, bad enum spellings, and malformed rows fail locally
    /// with the CSV row number and column name before any network call.
    FromCsv {
        /// CSV file (path, or - for stdin). The first row must be a header.
        file: String,
        /// Operation kind the rows convert to.
        #[arg(long, value_enum)]
        op: BulkOpArg,
        /// Project key applied to every converted operation (defaults to the
        /// global `--project`, then normal project selection such as
        /// HAMSTIK_PROJECT or `hamstik project use`).
        #[arg(long, value_name = "KEY")]
        project: Option<String>,
        /// Write the operations JSON to a file instead of stdout.
        #[arg(long = "output", value_name = "PATH", short = 'o')]
        output: Option<String>,
    },
}

/// Arguments for `work bulk run`.
#[derive(Args, Debug)]
pub struct WorkBulkRunArgs {
    /// Operation kind the batches perform.
    #[arg(long, value_enum)]
    pub op: BulkRunOpArg,
    /// Operations as a JSON array or JSON-lines file (path, or - for stdin).
    ///
    /// Required unless resuming an existing `--journal` (or using `--restart`).
    #[arg(long = "operations-file", value_name = "PATH")]
    pub operations_file: Option<String>,
    /// Local journal recording per-batch plans and results (created or resumed).
    #[arg(long, value_name = "PATH")]
    pub journal: String,
    /// Concurrency mode for update/transition batches.
    #[arg(long, value_enum)]
    pub concurrency: Option<ConcurrencyArg>,
    /// Re-send batches that previously failed (explicit review action).
    #[arg(long = "retry-failed")]
    pub retry_failed: bool,
    /// Start a new run, replacing any journal already at `--journal`.
    #[arg(long)]
    pub restart: bool,
}

/// Bulk runner operation kind.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulkRunOpArg {
    /// Create Work Items.
    #[value(name = "create")]
    Create,
    /// Update Work Items.
    #[value(name = "update")]
    Update,
    /// Transition Work Items.
    #[value(name = "transition")]
    Transition,
}

/// Bulk CSV operation kind.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BulkOpArg {
    /// Convert rows into `BulkCreateWorkItemOperation` objects.
    #[value(name = "create")]
    Create,
    /// Convert rows into `BulkUpdateWorkItemOperation` objects.
    #[value(name = "update")]
    Update,
}

/// Bulk concurrency mode.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConcurrencyArg {
    /// Every operation must carry a positive revision.
    #[value(name = "require-revision")]
    RequireRevision,
    /// Revisions are ignored; last write wins.
    #[value(name = "last-write-wins")]
    LastWriteWins,
}

impl ConcurrencyArg {
    /// The wire value for this mode.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ConcurrencyArg::RequireRevision => "require-revision",
            ConcurrencyArg::LastWriteWins => "last-write-wins",
        }
    }
}

/// Work item link relations.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationArg {
    /// This item blocks the target.
    #[value(name = "blocks")]
    Blocks,
    /// This item is blocked by the target.
    #[value(name = "blocked_by")]
    BlockedBy,
    /// The items are related.
    #[value(name = "relates")]
    Relates,
}

impl RelationArg {
    /// The wire value for this relation.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            RelationArg::Blocks => "blocks",
            RelationArg::BlockedBy => "blocked_by",
            RelationArg::Relates => "relates",
        }
    }
}

/// Work item collection ordering: a documented key plus an optional
/// direction (`--sort dueDate:desc`).
///
/// The Public API `sort` parameter accepts the key only, so the key is sent
/// verbatim and the direction is applied by the CLI to the fetched (bounded)
/// result, together with a deterministic tie-break so the same query produces
/// byte-identical ordering across runs. See `commands::work::sort`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortArg {
    key: SortKeyArg,
    dir: Option<SortDirArg>,
}

impl SortArg {
    /// Parses `KEY` or `KEY:<asc|desc>` for a `value_parser`.
    ///
    /// # Errors
    ///
    /// Returns a human-readable message when the key or direction is not one
    /// of the documented values.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let (key, dir) = match raw.split_once(':') {
            Some((key, dir)) => (key, Some(dir)),
            None => (raw, None),
        };
        let key = SortKeyArg::parse(key).ok_or_else(|| {
            format!(
                "invalid sort key {key:?}; expected one of {}",
                SortKeyArg::as_list()
            )
        })?;
        let dir =
            match dir {
                None => None,
                Some("") => return Err("invalid sort direction: expected asc or desc".to_string()),
                Some(dir) => Some(SortDirArg::parse(dir).ok_or_else(|| {
                    format!("invalid sort direction {dir:?}; expected asc or desc")
                })?),
            };
        Ok(Self { key, dir })
    }

    /// The documented key this sort orders on.
    #[must_use]
    pub fn key(self) -> SortKeyArg {
        self.key
    }

    /// The requested direction, if the caller named one.
    #[must_use]
    pub fn dir(self) -> Option<SortDirArg> {
        self.dir
    }

    /// The wire value for this sort (the `sort` query parameter).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.key.as_str()
    }
}

/// Sortable work item keys, matching the Public API `sort` enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKeyArg {
    /// Most recently updated first (default).
    Updated,
    /// Earliest due date first, undated last.
    DueDate,
    /// Urgent → high → medium → low.
    Priority,
    /// Overdue first, then by status bucket and priority.
    Rank,
}

impl SortKeyArg {
    /// Parses one of the documented key spellings.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "updated" => Some(Self::Updated),
            "dueDate" => Some(Self::DueDate),
            "priority" => Some(Self::Priority),
            "rank" => Some(Self::Rank),
            _ => None,
        }
    }

    /// The wire value for this key.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Updated => "updated",
            Self::DueDate => "dueDate",
            Self::Priority => "priority",
            Self::Rank => "rank",
        }
    }

    /// Help text listing the documented key set.
    #[must_use]
    pub fn as_list() -> String {
        [
            Self::Updated.as_str(),
            Self::DueDate.as_str(),
            Self::Priority.as_str(),
            Self::Rank.as_str(),
        ]
        .join(", ")
    }
}

/// Every accepted `--sort` spelling: the documented key plus its hidden
/// `:asc` / `:desc` forms. Kept as `&'static str` so clap can advertise the
/// keys in help, completion, and the command manifest while accepting the
/// directional spellings; `sort_spellings_match_the_documented_keys` keeps this
/// table and [`SortKeyArg`] from drifting apart.
const SORT_SPELLINGS: [(&str, &[&str]); 4] = [
    ("updated", &["updated:asc", "updated:desc"]),
    ("dueDate", &["dueDate:asc", "dueDate:desc"]),
    ("priority", &["priority:asc", "priority:desc"]),
    ("rank", &["rank:asc", "rank:desc"]),
];

/// Clap parser for `--sort`, which also publishes the documented key set so
/// help, shell completion, and the command manifest keep listing it.
#[derive(Clone, Copy, Debug)]
pub struct SortValueParser;

impl clap::builder::TypedValueParser for SortValueParser {
    type Value = SortArg;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        _arg: Option<&clap::Arg>,
        value: &std::ffi::OsStr,
    ) -> Result<Self::Value, clap::Error> {
        let text = value.to_string_lossy();
        SortArg::parse(&text).map_err(|message| {
            clap::Error::raw(
                clap::error::ErrorKind::InvalidValue,
                format!("invalid --sort value {text:?}: {message}"),
            )
            .with_cmd(cmd)
        })
    }

    fn possible_values(
        &self,
    ) -> Option<Box<dyn Iterator<Item = clap::builder::PossibleValue> + '_>> {
        Some(Box::new(SORT_SPELLINGS.iter().map(|(key, directions)| {
            let mut value = clap::builder::PossibleValue::new(*key);
            for dir in *directions {
                value = value.alias(*dir);
            }
            value
        })))
    }
}

/// Sort direction applied by the CLI to a fetched result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirArg {
    /// Lowest/earliest first.
    Asc,
    /// Highest/latest first.
    Desc,
}

impl SortDirArg {
    /// Parses `asc` or `desc`.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "asc" => Some(Self::Asc),
            "desc" => Some(Self::Desc),
            _ => None,
        }
    }
}

/// Arguments for `completion`.
#[derive(Args, Debug)]
pub struct CompletionArgs {
    /// Target shell.
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

/// Completion candidate types for `__complete`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum CompleteType {
    /// Organization slug.
    Org,
    /// Project key.
    Project,
    /// Work item key.
    WorkItemKey,
    /// Work item status.
    Status,
    /// Work item type.
    Type,
    /// Label name.
    Label,
}

/// Arguments for `__complete` (internal, hidden).
#[derive(Args, Debug)]
pub struct CompleteArgs {
    /// Completion candidate type.
    #[arg(value_enum)]
    pub typ: CompleteType,
    /// Prefix to filter candidates.
    pub prefix: String,
}

/// Arguments for `commands`: the machine-readable command manifest.
#[derive(Args, Debug)]
pub struct CommandsArgs {
    /// Output format for the manifest.
    #[arg(
        id = "manifest_format",
        long = "manifest-format",
        value_enum,
        default_value = "json"
    )]
    pub format: ManifestFormatArg,
}

/// Manifest output format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ManifestFormatArg {
    /// Stable deterministic JSON.
    Json,
}

// ---- Request-side value enums ---------------------------------------------

/// Work item statuses (request-side validation).
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusArg {
    /// Not started.
    #[value(name = "backlog")]
    Backlog,
    /// Ready to work.
    #[value(name = "todo")]
    Todo,
    /// Actively being worked.
    #[value(name = "in_progress")]
    InProgress,
    /// Awaiting review.
    #[value(name = "in_review")]
    InReview,
    /// Completed.
    #[value(name = "done")]
    Done,
}

impl StatusArg {
    /// Every accepted wire spelling, in CLI order.
    pub const ALL: &[Self] = &[
        Self::Backlog,
        Self::Todo,
        Self::InProgress,
        Self::InReview,
        Self::Done,
    ];

    /// The wire value for this status.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            StatusArg::Backlog => "backlog",
            StatusArg::Todo => "todo",
            StatusArg::InProgress => "in_progress",
            StatusArg::InReview => "in_review",
            StatusArg::Done => "done",
        }
    }
}

/// Work item types (request-side validation).
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeArg {
    /// A unit of work.
    Task,
    /// A defect.
    Bug,
    /// A user story.
    Story,
    /// A feature request.
    Feature,
    /// A large body of work.
    Epic,
}

impl TypeArg {
    /// Every accepted wire spelling, in CLI order.
    pub const ALL: &[Self] = &[
        Self::Task,
        Self::Bug,
        Self::Story,
        Self::Feature,
        Self::Epic,
    ];

    /// The wire value for this type.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TypeArg::Task => "task",
            TypeArg::Bug => "bug",
            TypeArg::Story => "story",
            TypeArg::Feature => "feature",
            TypeArg::Epic => "epic",
        }
    }
}

/// Priorities (request-side validation).
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PriorityArg {
    /// Low priority.
    Low,
    /// Medium priority.
    Medium,
    /// High priority.
    High,
    /// Drop everything.
    Urgent,
}

impl PriorityArg {
    /// Every accepted wire spelling, in CLI order.
    pub const ALL: &[Self] = &[Self::Low, Self::Medium, Self::High, Self::Urgent];

    /// The wire value for this priority.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PriorityArg::Low => "low",
            PriorityArg::Medium => "medium",
            PriorityArg::High => "high",
            PriorityArg::Urgent => "urgent",
        }
    }
}

/// List scope filter.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeArg {
    /// Every work item.
    All,
    /// Items not in a terminal status.
    Open,
    /// Items in a terminal status.
    Closed,
}

impl ScopeArg {
    /// The wire value for this scope.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeArg::All => "all",
            ScopeArg::Open => "open",
            ScopeArg::Closed => "closed",
        }
    }
}

/// Sprint lifecycle states (request-side validation).
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SprintStateArg {
    /// Active (in progress).
    #[value(name = "active")]
    Active,
    /// Completed.
    #[value(name = "done")]
    Done,
}

impl SprintStateArg {
    /// The wire value for this sprint state.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SprintStateArg::Active => "active",
            SprintStateArg::Done => "done",
        }
    }
}

/// Release Version lifecycle states (request-side validation).
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseStateArg {
    /// Planned.
    #[value(name = "planned")]
    Planned,
    /// In progress.
    #[value(name = "in_progress")]
    InProgress,
    /// Released.
    #[value(name = "released")]
    Released,
    /// Archived.
    #[value(name = "archived")]
    Archived,
}

impl ReleaseStateArg {
    /// The wire value for this release state.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ReleaseStateArg::Planned => "planned",
            ReleaseStateArg::InProgress => "in_progress",
            ReleaseStateArg::Released => "released",
            ReleaseStateArg::Archived => "archived",
        }
    }
}

/// Organization Milestone lifecycle states (request-side validation).
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MilestoneStateArg {
    /// Planned.
    #[value(name = "planned")]
    Planned,
    /// In progress.
    #[value(name = "in_progress")]
    InProgress,
    /// Completed.
    #[value(name = "completed")]
    Completed,
    /// Archived.
    #[value(name = "archived")]
    Archived,
}

impl MilestoneStateArg {
    /// The wire value for this milestone state.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MilestoneStateArg::Planned => "planned",
            MilestoneStateArg::InProgress => "in_progress",
            MilestoneStateArg::Completed => "completed",
            MilestoneStateArg::Archived => "archived",
        }
    }
}

/// Arguments for the `release` command group.
#[derive(Args, Debug)]
pub struct ReleaseArgs {
    /// The release subcommand to run.
    #[command(subcommand)]
    pub command: ReleaseCommand,
}

/// Release subcommands.
#[derive(Subcommand, Debug)]
pub enum ReleaseCommand {
    /// List release versions in a project.
    List {
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Only releases in this lifecycle state.
        #[arg(long, value_enum)]
        state: Option<ReleaseStateArg>,
        /// Include archived releases in the results.
        #[arg(
            long = "include-archived",
            value_name = "true|false",
            num_args = 0..=1,
            default_missing_value = "true"
        )]
        include_archived: Option<bool>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// View a release version.
    View {
        /// Release version id (UUID).
        id: String,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
    },
    /// Create a release version.
    Create {
        /// Release name.
        #[arg(long)]
        name: Option<String>,
        /// Separate display version label.
        #[arg(long = "display-version", value_name = "LABEL")]
        display_version: Option<String>,
        /// Description text.
        #[arg(long, conflicts_with_all = ["description_file", "description_editor"])]
        description: Option<String>,
        /// Description source (path, or - for stdin).
        #[arg(
            long = "description-file",
            value_name = "PATH",
            conflicts_with_all = ["description", "description_editor"]
        )]
        description_file: Option<String>,
        /// Author the description in $VISUAL/$EDITOR instead of passing text.
        #[arg(
            long = "description-editor",
            conflicts_with_all = ["description", "description_file"]
        )]
        description_editor: bool,
        /// Release owner: me, a user UUID, or a public ID (usr_...).
        #[arg(long, value_name = "ME|ID")]
        owner: Option<String>,
        /// Target date. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
        /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
        /// is read in the host's local time zone.
        #[arg(long = "target-date", value_name = "DATE")]
        target_date: Option<TimeArg>,
        /// Release date. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
        /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
        /// is read in the host's local time zone.
        #[arg(long = "release-date", value_name = "DATE")]
        release_date: Option<TimeArg>,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Edit a release version.
    Edit {
        /// Release version id (UUID).
        id: String,
        /// New name.
        #[arg(long)]
        name: Option<String>,
        /// New display version label.
        #[arg(
            long = "display-version",
            value_name = "LABEL",
            conflicts_with_all = ["clear_display_version"]
        )]
        display_version: Option<String>,
        /// Clear the display version label.
        #[arg(long = "clear-display-version")]
        clear_display_version: bool,
        /// New description text.
        #[arg(long, conflicts_with_all = ["description_file", "description_editor", "clear_description"])]
        description: Option<String>,
        /// New description source (path, or - for stdin).
        #[arg(
            long = "description-file",
            value_name = "PATH",
            conflicts_with_all = ["description", "clear_description", "description_editor"]
        )]
        description_file: Option<String>,
        /// Author the description in $VISUAL/$EDITOR instead of passing text.
        #[arg(
            long = "description-editor",
            conflicts_with_all = ["description", "description_file", "clear_description"]
        )]
        description_editor: bool,
        /// Clear the description.
        #[arg(long = "clear-description")]
        clear_description: bool,
        /// New release owner: me, a user UUID, a public ID (usr_...), or `none`.
        #[arg(long, value_name = "ME|NONE|ID")]
        owner: Option<String>,
        /// New target date.
        #[arg(long = "target-date", value_name = "DATE")]
        target_date: Option<TimeArg>,
        /// Clear the target date.
        #[arg(
            long = "clear-target-date",
            conflicts_with_all = ["target_date"]
        )]
        clear_target_date: bool,
        /// New release date.
        #[arg(long = "release-date", value_name = "DATE")]
        release_date: Option<TimeArg>,
        /// Clear the release date.
        #[arg(
            long = "clear-release-date",
            conflicts_with_all = ["release_date"]
        )]
        clear_release_date: bool,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// List allowed release state transitions.
    Transitions {
        /// Release version id (UUID).
        id: String,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
    },
    /// Transition a release to a target state.
    Transition {
        /// Release version id (UUID).
        id: String,
        /// Target state.
        #[arg(value_enum)]
        target: ReleaseStateArg,
        /// Confirm releasing a scope that still contains incomplete Work Items.
        #[arg(long = "confirm-incomplete-scope")]
        confirm_incomplete_scope: bool,
        /// Why the release moved.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Archive a release version.
    Archive {
        /// Release version id (UUID).
        id: String,
        /// Why the release was archived.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Restore an archived release version.
    Restore {
        /// Release version id (UUID).
        id: String,
        /// Why the release was restored.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Read a release's scope, progress, and Work Items.
    Scope {
        /// Release version id (UUID).
        id: String,
        /// Free-text search over Work Item titles.
        #[arg(long = "search", value_name = "TEXT")]
        search: Option<String>,
        /// Filter by status (server-defined spelling).
        #[arg(long, value_name = "STATUS")]
        status: Option<String>,
        /// Filter by type (server-defined spelling).
        #[arg(long = "type", value_name = "TYPE")]
        item_type: Option<String>,
        /// Filter by priority (server-defined spelling).
        #[arg(long, value_name = "PRIORITY")]
        priority: Option<String>,
        /// Filter by assignee user UUID or public ID (usr_...).
        #[arg(long, value_name = "ID")]
        assignee: Option<String>,
        /// Sort key (server-defined).
        #[arg(long, value_name = "KEY")]
        sort: Option<String>,
        /// Sort direction (server-defined).
        #[arg(long, value_name = "DIR")]
        direction: Option<String>,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// Manage a Work Item's release memberships.
    Item(ReleaseItemArgs),
    /// Change release memberships for up to 50 Work Items from a JSON file.
    BulkMembership {
        /// JSON file containing `{ "operations": [...] }` (`-` for stdin).
        #[arg(long = "file", value_name = "PATH")]
        file: String,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Read and publish release announcements.
    Announcement(ReleaseAnnouncementArgs),
    /// Generate and download frozen release audit packages.
    Audit(ReleaseAuditArgs),
}

/// Arguments for `release item`: a Work Item's release memberships.
#[derive(Args, Debug)]
pub struct ReleaseItemArgs {
    /// The release item subcommand to run.
    #[command(subcommand)]
    pub command: ReleaseItemCommand,
}

/// Release item subcommands.
#[derive(Subcommand, Debug)]
pub enum ReleaseItemCommand {
    /// List the release versions a Work Item belongs to.
    List {
        /// Work item key.
        key: String,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
    },
    /// Add the Work Item to one or more release versions.
    Add {
        /// Work item key.
        key: String,
        /// Release version ids (UUIDs).
        #[arg(value_name = "RELEASE_ID", num_args = 1..)]
        release_ids: Vec<String>,
        /// Confirm correcting the scope of a released Work Item.
        #[arg(long = "confirm-released-scope-correction")]
        confirm_released_scope_correction: bool,
        /// Why the membership changed.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Remove the Work Item from one or more release versions.
    Remove {
        /// Work item key.
        key: String,
        /// Release version ids (UUIDs).
        #[arg(value_name = "RELEASE_ID", num_args = 1..)]
        release_ids: Vec<String>,
        /// Confirm correcting the scope of a released Work Item.
        #[arg(long = "confirm-released-scope-correction")]
        confirm_released_scope_correction: bool,
        /// Why the membership changed.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Replace the Work Item's release versions with exactly these.
    Replace {
        /// Work item key.
        key: String,
        /// Release version ids (UUIDs).
        #[arg(value_name = "RELEASE_ID", num_args = 1..)]
        release_ids: Vec<String>,
        /// Confirm correcting the scope of a released Work Item.
        #[arg(long = "confirm-released-scope-correction")]
        confirm_released_scope_correction: bool,
        /// Why the membership changed.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
}

/// Arguments for `release announcement`.
#[derive(Args, Debug)]
pub struct ReleaseAnnouncementArgs {
    /// The announcement subcommand to run.
    #[command(subcommand)]
    pub command: ReleaseAnnouncementCommand,
}

/// Release announcement subcommands.
#[derive(Subcommand, Debug)]
pub enum ReleaseAnnouncementCommand {
    /// Read the latest published announcement.
    Current {
        /// Release version id (UUID).
        release_id: String,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
    },
    /// Read one historical published announcement revision.
    Revision {
        /// Release version id (UUID).
        release_id: String,
        /// Announcement revision number.
        revision: i64,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
    },
    /// Work with the editable announcement draft.
    Draft(ReleaseAnnouncementDraftArgs),
    /// Publish the draft as an immutable announcement revision.
    Publish {
        /// Release version id (UUID).
        release_id: String,
        /// Draft revision to publish (defaults to the current draft's).
        #[arg(long, value_name = "N")]
        revision: Option<i64>,
        /// Confirm publishing an announcement with no included Work Items.
        #[arg(long = "confirm-empty")]
        confirm_empty: bool,
        /// Why the announcement was published.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
}

/// Arguments for `release announcement draft`.
#[derive(Args, Debug)]
pub struct ReleaseAnnouncementDraftArgs {
    /// The draft subcommand to run.
    #[command(subcommand)]
    pub command: ReleaseAnnouncementDraftCommand,
}

/// Release announcement draft subcommands.
#[derive(Subcommand, Debug)]
pub enum ReleaseAnnouncementDraftCommand {
    /// Show the active announcement draft.
    Show {
        /// Release version id (UUID).
        release_id: String,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
    },
    /// Generate or refresh the announcement draft from release scope.
    Generate {
        /// Release version id (UUID).
        release_id: String,
        /// Generate from this release revision instead of the current one.
        #[arg(long, value_name = "N")]
        revision: Option<i64>,
        /// Confirm replacing a draft generated from a different Organization view.
        #[arg(long = "confirm-replace-organization")]
        confirm_replace_organization: bool,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Replace the draft narrative, categories, and included Work Items.
    Edit {
        /// Release version id (UUID).
        release_id: String,
        /// JSON file containing `{introduction, highlights, categories,
        /// includedWorkItemIds}` (`-` for stdin).
        #[arg(long = "file", value_name = "PATH")]
        file: String,
        /// Draft revision to replace (defaults to the current draft's).
        #[arg(long, value_name = "N")]
        revision: Option<i64>,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
    },
    /// Discard the active announcement draft.
    Discard {
        /// Release version id (UUID).
        release_id: String,
        /// Draft revision to discard (defaults to the current draft's).
        #[arg(long, value_name = "N")]
        revision: Option<i64>,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
    },
}

/// Arguments for `release audit`.
#[derive(Args, Debug)]
pub struct ReleaseAuditArgs {
    /// The audit subcommand to run.
    #[command(subcommand)]
    pub command: ReleaseAuditCommand,
}

/// Release audit subcommands.
#[derive(Subcommand, Debug)]
pub enum ReleaseAuditCommand {
    /// Generate a frozen dossier or UTC change register.
    Generate {
        /// Package kind: dossier (one release) or register (date range).
        #[arg(long, value_enum)]
        kind: ReleaseAuditKindArg,
        /// Release version id (required for dossier packages).
        #[arg(
            long,
            value_name = "UUID",
            requires_if("dossier", "kind"),
            requires_if("dossier", "kind")
        )]
        release: Option<String>,
        /// Range start (required for register packages).
        #[arg(long, value_name = "DATE")]
        from: Option<TimeArg>,
        /// Range end, inclusive (required for register packages).
        #[arg(long, value_name = "DATE")]
        through: Option<TimeArg>,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Download the exact saved package as JSON or a CSV ZIP.
    Get {
        /// Audit report id (UUID).
        report_id: String,
        /// Package format to download.
        #[arg(
            id = "audit_format",
            long = "audit-format",
            value_enum,
            default_value = "json"
        )]
        format: ReleaseAuditFormatArg,
        /// Write the package bytes to this file instead of stdout (CSV ZIP).
        #[arg(long, value_name = "PATH")]
        output: Option<PathBuf>,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
    },
    /// List saved audit packages in the project.
    List {
        /// Only packages generated for this Release Version.
        #[arg(long, value_name = "UUID")]
        release: Option<String>,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
}

/// Release audit package kinds.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseAuditKindArg {
    /// A frozen dossier for one Release Version.
    #[value(name = "dossier")]
    Dossier,
    /// A frozen UTC change register for a date range.
    #[value(name = "register")]
    Register,
}

/// Release audit download formats.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseAuditFormatArg {
    /// The exact saved JSON package.
    #[value(name = "json")]
    Json,
    /// The CSV ZIP package.
    #[value(name = "csv")]
    Csv,
}

/// Arguments for the `milestone` command group.
#[derive(Args, Debug)]
pub struct MilestoneArgs {
    /// The milestone subcommand to run.
    #[command(subcommand)]
    pub command: MilestoneCommand,
}

/// Milestone subcommands.
#[derive(Subcommand, Debug)]
pub enum MilestoneCommand {
    /// List Organization Milestones.
    List {
        /// Only milestones in this lifecycle state.
        #[arg(long, value_enum)]
        state: Option<MilestoneStateArg>,
        /// Include archived milestones in the results.
        #[arg(
            long = "include-archived",
            value_name = "true|false",
            num_args = 0..=1,
            default_missing_value = "true"
        )]
        include_archived: Option<bool>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// View a milestone and one page of its releases.
    View {
        /// Milestone id (UUID).
        id: String,
        /// The release cursor the release page starts after.
        #[arg(long = "release-after", value_name = "UUID")]
        release_after: Option<String>,
        /// Maximum releases per page.
        #[arg(long = "release-limit", value_name = "N")]
        release_limit: Option<u32>,
    },
    /// Create an Organization Milestone.
    Create {
        /// Milestone name.
        #[arg(long)]
        name: Option<String>,
        /// Description text.
        #[arg(long, conflicts_with_all = ["description_file", "description_editor"])]
        description: Option<String>,
        /// Description source (path, or - for stdin).
        #[arg(
            long = "description-file",
            value_name = "PATH",
            conflicts_with_all = ["description", "description_editor"]
        )]
        description_file: Option<String>,
        /// Author the description in $VISUAL/$EDITOR instead of passing text.
        #[arg(
            long = "description-editor",
            conflicts_with_all = ["description", "description_file"]
        )]
        description_editor: bool,
        /// Milestone owner: me, a user UUID, or a public ID (usr_...).
        #[arg(long, value_name = "ME|ID")]
        owner: Option<String>,
        /// Target date. Accepts RFC 3339, `YYYY-MM-DD`, `today`/`yesterday`/`tomorrow`, or a
        /// relative offset such as `7d`, `2w`, or `+3h`; input without a UTC offset
        /// is read in the host's local time zone.
        #[arg(long = "target-date", value_name = "DATE")]
        target_date: Option<TimeArg>,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Edit a milestone.
    Edit {
        /// Milestone id (UUID).
        id: String,
        /// New name.
        #[arg(long)]
        name: Option<String>,
        /// New description text.
        #[arg(long, conflicts_with_all = ["description_file", "description_editor", "clear_description"])]
        description: Option<String>,
        /// New description source (path, or - for stdin).
        #[arg(
            long = "description-file",
            value_name = "PATH",
            conflicts_with_all = ["description", "clear_description", "description_editor"]
        )]
        description_file: Option<String>,
        /// Author the description in $VISUAL/$EDITOR instead of passing text.
        #[arg(
            long = "description-editor",
            conflicts_with_all = ["description", "description_file", "clear_description"]
        )]
        description_editor: bool,
        /// Clear the description.
        #[arg(long = "clear-description")]
        clear_description: bool,
        /// New milestone owner: me, a user UUID, a public ID (usr_...), or `none`.
        #[arg(long, value_name = "ME|NONE|ID")]
        owner: Option<String>,
        /// New target date.
        #[arg(long = "target-date", value_name = "DATE")]
        target_date: Option<TimeArg>,
        /// Clear the target date.
        #[arg(
            long = "clear-target-date",
            conflicts_with_all = ["target_date"]
        )]
        clear_target_date: bool,
        /// Why the milestone changed (recorded in events).
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// List allowed milestone state transitions.
    Transitions {
        /// Milestone id (UUID).
        id: String,
    },
    /// Transition a milestone to a target state.
    Transition {
        /// Milestone id (UUID).
        id: String,
        /// Target state.
        #[arg(value_enum)]
        target: MilestoneStateArg,
        /// Why the milestone moved.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// List the release versions in a milestone.
    Releases {
        /// Milestone id (UUID).
        id: String,
        /// The release cursor the page starts after.
        #[arg(long = "release-after", value_name = "UUID")]
        release_after: Option<String>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// Add or remove a release version from a milestone.
    Release {
        /// The milestone release subcommand to run.
        #[command(subcommand)]
        command: MilestoneReleaseCommand,
    },
    /// Read the milestone history feed (newest first).
    Events {
        /// Milestone id (UUID).
        id: String,
        /// The event id the (older) page starts before.
        #[arg(long = "before-event-id", value_name = "UUID")]
        before_event_id: Option<String>,
        /// Maximum events per page.
        #[arg(long, value_name = "N")]
        limit: Option<u32>,
    },
}

/// Milestone release subcommands.
#[derive(Subcommand, Debug)]
pub enum MilestoneReleaseCommand {
    /// Add a release version to the milestone.
    Add {
        /// Milestone id (UUID).
        id: String,
        /// Release version id (UUID).
        release_id: String,
        /// Why the release was added.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
    /// Remove a release version from the milestone.
    Remove {
        /// Milestone id (UUID).
        id: String,
        /// Release version id (UUID).
        release_id: String,
        /// Confirm removing the release and its milestone progress.
        #[arg(long = "confirm-remove")]
        confirm_remove: bool,
        /// Why the release was removed.
        #[arg(long, value_name = "TEXT")]
        reason: Option<String>,
        /// Bypass revision conflict protection (If-Match: *).
        #[arg(long)]
        force: bool,
        /// Explicit idempotency key.
        #[arg(long = "idempotency-key", value_name = "KEY")]
        idempotency_key: Option<String>,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn sort_spellings_match_the_documented_keys() {
        let documented = SORT_SPELLINGS
            .iter()
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        assert_eq!(
            documented.join(", "),
            SortKeyArg::as_list(),
            "the advertised --sort spellings must be the documented keys"
        );
        for (key, directions) in SORT_SPELLINGS {
            assert!(SortArg::parse(key).is_ok(), "{key} must parse");
            assert_eq!(SortArg::parse(key).expect("parses").dir(), None);
            for spelling in directions {
                let parsed = SortArg::parse(spelling).unwrap_or_else(|e| panic!("{spelling}: {e}"));
                assert_eq!(parsed.key().as_str(), key);
                assert_eq!(
                    parsed.dir(),
                    Some(if spelling.ends_with(":asc") {
                        SortDirArg::Asc
                    } else {
                        SortDirArg::Desc
                    })
                );
                assert_eq!(parsed.as_str(), key, "only the key goes on the wire");
            }
        }
        assert!(SortArg::parse("assignee:desc").is_err());
        assert!(SortArg::parse("updated:sideways").is_err());
        assert!(SortArg::parse("updated:").is_err());
    }

    #[test]
    fn pagination_flags_separate_the_result_cap_from_the_page_size() {
        let none = PaginationArgs {
            limit: None,
            cursor: None,
            all: false,
        };
        assert_eq!(none.page_size(), None);
        assert_eq!(none.max_items(), None);

        let capped = PaginationArgs {
            limit: Some(40),
            cursor: Some("opaque".to_string()),
            all: true,
        };
        assert_eq!(capped.page_size(), Some(40));
        assert_eq!(capped.max_items(), Some(40));

        let oversized = PaginationArgs {
            limit: Some(500),
            cursor: None,
            all: true,
        };
        assert_eq!(
            oversized.page_size(),
            Some(PaginationArgs::MAX_PAGE_SIZE),
            "the wire page size never exceeds the endpoint maximum"
        );
        assert_eq!(oversized.max_items(), Some(500));

        // Endpoints the contract caps lower still page to the requested total.
        assert_eq!(
            oversized.directory_page_size(),
            Some(PaginationArgs::DIRECTORY_PAGE_SIZE)
        );
        assert_eq!(oversized.directory_page_size(), Some(100));
        assert_eq!(capped.directory_page_size(), Some(40));
    }

    #[test]
    fn parses_work_transition() {
        let cli = Cli::try_parse_from(["hamstik", "work", "transition", "HAM-1", "in_review"])
            .expect("parses");
        match cli.command {
            Command::Work(work) => match work.command {
                WorkCommand::Transition { key, target } => {
                    assert_eq!(key, "HAM-1");
                    assert_eq!(target, StatusArg::InReview);
                }
                _ => panic!("wrong subcommand"),
            },
            _ => panic!("wrong command"),
        }
    }

    #[test]
    fn global_flags_propagate() {
        let cli =
            Cli::try_parse_from(["hamstik", "work", "list", "--json", "--org", "acme"]).unwrap();
        assert!(cli.global.json);
        assert_eq!(cli.global.org.as_deref(), Some("acme"));
    }

    #[test]
    fn rejects_bad_status_value() {
        assert!(Cli::try_parse_from(["hamstik", "work", "transition", "HAM-1", "nope"]).is_err());
    }

    #[test]
    fn edit_conflict_flags_rejected() {
        assert!(
            Cli::try_parse_from([
                "hamstik",
                "work",
                "edit",
                "HAM-1",
                "--assignee",
                "me",
                "--clear-assignee"
            ])
            .is_err()
        );
    }
}
