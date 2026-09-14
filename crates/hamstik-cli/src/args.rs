// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Command-line argument definitions (clap).
//!
//! Global options are declared once and propagate to every subcommand
//! (SPEC §37). Enum-valued options are validated here on the request side; the
//! server remains authoritative for whether a value is actually accepted.

use std::path::PathBuf;

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

    /// Emit only essential identifiers.
    #[arg(long, global = true)]
    pub quiet: bool,

    /// Show diagnostic details on stderr.
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Disable colored output.
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Never prompt interactively; fail instead.
    #[arg(long, global = true)]
    pub no_input: bool,

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

/// Every CLI subcommand.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show the authenticated user and credential context.
    Me,
    /// Manage authentication and profiles.
    Auth(AuthArgs),
    /// Inspect and manage the working context.
    Context(ContextArgs),
    /// Work with organizations.
    Org(OrgArgs),
    /// Work with projects.
    Project(ProjectArgs),
    /// Work with sprints.
    Sprint(SprintArgs),
    /// Work with labels.
    Label(LabelArgs),
    /// Work with work items.
    Work(Box<WorkArgs>),
    /// View user profiles, work, activity, and avatars.
    User(UserArgs),
    /// Validate SqueakQL expressions.
    Squeakql(SqueakQlArgs),
    /// Inspect the Public API contract.
    Api(ApiArgs),
    /// Verify configuration, credentials, connectivity, API compatibility,
    /// and selected Organization/Project context.
    Doctor {
        /// Check only local configuration, context, credential-store access,
        /// terminal behavior, and bundled compatibility metadata; remote
        /// checks are marked skipped and no network traffic is generated.
        #[arg(long)]
        local_only: bool,
    },
    /// Generate a shell completion script.
    Completion(CompletionArgs),
    /// Print the CLI version.
    Version,
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
        /// SqueakQL expression.
        #[arg(value_name = "QUERY")]
        query: String,
    },
}

/// Arguments for the `api` command group.
#[derive(Args, Debug)]
pub struct ApiArgs {
    /// The API subcommand to run.
    #[command(subcommand)]
    pub command: ApiCommand,
}

/// Public API metadata subcommands.
#[derive(Subcommand, Debug)]
pub enum ApiCommand {
    /// Print the live Public API OpenAPI document.
    Openapi,
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
        /// Only events strictly after this RFC 3339 timestamp.
        #[arg(long, value_name = "RFC3339")]
        since: Option<String>,
        /// Pagination options.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// Set the default project for the active profile.
    Use {
        /// Project key.
        key: String,
    },
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
    #[arg(long, conflicts_with = "description_file")]
    pub description: Option<String>,
    /// Description source (path, or - for stdin).
    #[arg(long = "description-file", value_name = "PATH")]
    pub description_file: Option<String>,
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
    #[arg(long, conflicts_with = "clear_description")]
    pub description: Option<String>,
    /// New description source (path, or - for stdin).
    #[arg(
        long = "description-file",
        value_name = "PATH",
        conflicts_with = "clear_description"
    )]
    pub description_file: Option<String>,
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
        /// Planned start date (RFC 3339).
        #[arg(long = "start-date", value_name = "RFC3339")]
        start_date: Option<String>,
        /// Planned end date (RFC 3339).
        #[arg(long = "end-date", value_name = "RFC3339")]
        end_date: Option<String>,
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
    /// List allowed sprint state transitions.
    Transitions {
        /// Sprint id (UUID).
        id: String,
        /// Project key (overrides context).
        #[arg(long)]
        project: Option<String>,
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

/// Shared list pagination options.
#[derive(Args, Debug, Clone)]
pub struct PaginationArgs {
    /// Maximum items per page (endpoint maximum is 100 or 200).
    #[arg(long, value_name = "N")]
    pub limit: Option<u32>,
    /// Opaque continuation cursor.
    #[arg(long, value_name = "CURSOR")]
    pub cursor: Option<String>,
    /// Follow all pages.
    #[arg(long)]
    pub all: bool,
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
        /// Only events strictly after this RFC 3339 timestamp.
        #[arg(long, value_name = "RFC3339")]
        since: Option<String>,
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
        #[arg(long, value_name = "VALUE")]
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
    /// Only items updated after this RFC 3339 timestamp.
    #[arg(long = "updated-after", value_name = "RFC3339")]
    pub updated_after: Option<String>,
    /// Filter to overdue/on-track Work Items.
    #[arg(long, value_name = "true|false")]
    pub overdue: Option<bool>,
    /// Only items due strictly before this RFC 3339 timestamp.
    #[arg(long = "due-before", value_name = "RFC3339")]
    pub due_before: Option<String>,
    /// Only items due strictly after this RFC 3339 timestamp.
    #[arg(long = "due-after", value_name = "RFC3339")]
    pub due_after: Option<String>,
    /// Result ordering.
    #[arg(long, value_enum)]
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

/// Work item subcommands.
#[derive(Subcommand, Debug)]
pub enum WorkCommand {
    /// List work items.
    List(WorkListArgs),
    /// List Work assigned to the authenticated user across Projects.
    #[command(alias = "my")]
    Mine(MyWorkArgs),
    /// Search Organization Work Items with SqueakQL.
    Search {
        /// SqueakQL expression.
        #[arg(value_name = "QUERY")]
        query: String,
        /// Pagination options carried in the JSON request body.
        #[command(flatten)]
        pagination: PaginationArgs,
    },
    /// View a work item.
    View {
        /// Work item key (e.g. HAM-42).
        key: String,
    },
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
    /// Show a work item's activity feed.
    Activity {
        /// Work item key.
        key: String,
        /// Only events strictly after this RFC 3339 timestamp.
        #[arg(long, value_name = "RFC3339")]
        since: Option<String>,
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
    /// Create, update, or transition many work items in one request.
    Bulk(WorkBulkArgs),
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
    /// Only items due strictly before this RFC 3339 timestamp.
    #[arg(long = "due-before", value_name = "RFC3339")]
    pub due_before: Option<String>,
    /// Only items due strictly after this RFC 3339 timestamp.
    #[arg(long = "due-after", value_name = "RFC3339")]
    pub due_after: Option<String>,
    /// Result ordering.
    #[arg(long, value_enum)]
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
    /// Only items updated at/after this RFC 3339 timestamp.
    #[arg(long = "updated-after", value_name = "RFC3339")]
    pub updated_after: Option<String>,
    /// Only overdue items (true) or only on-track items (false).
    #[arg(long, value_name = "true|false")]
    pub overdue: Option<bool>,
    /// Only items due strictly before this RFC 3339 timestamp.
    #[arg(long = "due-before", value_name = "RFC3339")]
    pub due_before: Option<String>,
    /// Only items due strictly after this RFC 3339 timestamp.
    #[arg(long = "due-after", value_name = "RFC3339")]
    pub due_after: Option<String>,
    /// Result ordering: updated, dueDate, priority, or rank.
    #[arg(long, value_enum)]
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
    /// Work item title.
    #[arg(long)]
    pub title: Option<String>,
    /// Description text.
    #[arg(long, conflicts_with = "description_file")]
    pub description: Option<String>,
    /// Description source (path, or - for stdin).
    #[arg(long = "description-file", value_name = "PATH")]
    pub description_file: Option<String>,
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
    /// Due date (RFC 3339).
    #[arg(long = "due-date", value_name = "DATE")]
    pub due_date: Option<String>,
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
    #[arg(long, conflicts_with = "clear_description")]
    pub description: Option<String>,
    #[arg(
        long = "description-file",
        value_name = "PATH",
        conflicts_with = "clear_description"
    )]
    /// New description source (path, or - for stdin).
    pub description_file: Option<String>,
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
    /// New due date (RFC 3339).
    pub due_date: Option<String>,
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
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Comment body source (path, or - for stdin).
        #[arg(long = "body-file", value_name = "PATH")]
        body_file: Option<String>,
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
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Replacement body source (path, or - for stdin).
        #[arg(long = "body-file", value_name = "PATH")]
        body_file: Option<String>,
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

/// Work item collection ordering.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortArg {
    /// Most recently updated first (default).
    Updated,
    /// Earliest due date first, undated last.
    #[value(name = "dueDate")]
    DueDate,
    /// Urgent → high → medium → low.
    Priority,
    /// Overdue first, then by status bucket and priority.
    Rank,
}

impl SortArg {
    /// The wire value for this sort.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SortArg::Updated => "updated",
            SortArg::DueDate => "dueDate",
            SortArg::Priority => "priority",
            SortArg::Rank => "rank",
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
