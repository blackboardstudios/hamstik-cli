// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Wire models mirroring the Hamstik Public API v1 contract.
//!
//! Scalar identifiers are modeled as `String` and timestamps as `String`
//! (RFC 3339) to remain resilient to additive server changes and to avoid
//! coupling the CLI to a date/time crate. Known enum values are validated on the
//! request side (CLI); responses are stored verbatim.
//!
//! User summaries arrive in two v1-compatible shapes — the legacy `{id, name}`
//! projection used by original Work Item/comment/attachment surfaces and the
//! `{publicId, name}` projection used by newer mutation responses — so they are
//! decoded into one tolerant [`UserSummary`] enum.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::pagination::Page;

fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// A paginated list query for endpoints that only take `limit`/`cursor`.
#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    /// Maximum items per page (server-capped).
    pub limit: Option<u32>,
    /// Opaque continuation cursor from a previous page's `nextCursor`.
    pub cursor: Option<String>,
}

/// Query options shared by the Advanced Report and Dashboard directories.
#[derive(Debug, Clone, Default)]
pub struct ListAdvancedOptions {
    /// Maximum items per page (server-capped at 100).
    pub limit: Option<u32>,
    /// Opaque continuation cursor from a previous page's `nextCursor`.
    pub cursor: Option<String>,
    /// Visibility selector (`all`, `personal`, or `organization`).
    pub visibility: Option<String>,
}

/// Query options for `GET .../projects` (which also filters by archive state).
#[derive(Debug, Clone, Default)]
pub struct ListProjectsOptions {
    /// Maximum items per page (server-capped).
    pub limit: Option<u32>,
    /// Opaque continuation cursor from a previous page's `nextCursor`.
    pub cursor: Option<String>,
    /// When true, list only archived Projects; when false or omitted, list
    /// only unarchived Projects. This is an archived-state filter, not a
    /// union: listing both states requires separate queries.
    pub archived: Option<bool>,
}

/// Query options for the Organization member directory.
#[derive(Debug, Clone, Default)]
pub struct ListOrganizationUsersOptions {
    /// Maximum items per page (server-capped).
    pub limit: Option<u32>,
    /// Opaque continuation cursor from a previous page's `nextCursor`.
    pub cursor: Option<String>,
    /// Literal substring filter over member name or Organization username.
    pub q: Option<String>,
}

/// Query options for the activity feeds (`since` is strictly-after).
#[derive(Debug, Clone, Default)]
pub struct ActivityOptions {
    /// Maximum items per page (server-capped).
    pub limit: Option<u32>,
    /// Opaque continuation cursor from a previous page's `nextCursor`.
    pub cursor: Option<String>,
    /// Only events strictly after this RFC 3339 timestamp.
    pub since: Option<String>,
}

/// Query options for a profile avatar request.
///
/// The three values are opaque cache/format selectors defined by the Public
/// API. The client forwards them unchanged and never interprets them.
#[derive(Debug, Clone, Default)]
pub struct AvatarOptions {
    /// Opaque avatar version selector.
    pub version: Option<String>,
    /// Opaque requested format selector.
    pub format: Option<String>,
    /// Opaque avatar revision selector.
    pub revision: Option<String>,
}

/// Filters accepted by the Work Item collections (`.../work-items`,
/// `/organizations/{slug}/work-items`, `/my/work`, and profile Work).
///
/// Every field is optional; each endpoint validates the subset it accepts, so
/// callers must only set the fields allowed for the target collection.
#[derive(Debug, Clone, Default)]
pub struct ListWorkItemsQuery {
    /// Maximum items per page (server-capped).
    pub limit: Option<u32>,
    /// Opaque continuation cursor from a previous page's `nextCursor`.
    pub cursor: Option<String>,
    /// Full-text search over titles and descriptions.
    pub q: Option<String>,
    /// Filter by status; multiple values are OR-ed.
    pub status: Vec<String>,
    /// Hierarchy scope of the result set (`all`, `open`, or `closed`).
    pub scope: Option<String>,
    /// Filter by work item type; multiple values are OR-ed.
    pub item_type: Vec<String>,
    /// Filter by priority; multiple values are OR-ed.
    pub priority: Vec<String>,
    /// Filter by assignee user id (`me` is accepted by the server).
    pub assignee: Option<String>,
    /// Filter by sprint id (or `none`).
    pub sprint: Option<String>,
    /// Filter by label id; multiple values are OR-ed.
    pub label: Vec<String>,
    /// Filter by label name; multiple values are OR-ed.
    pub label_name: Vec<String>,
    /// Only items with this parent work item key.
    pub parent: Option<String>,
    /// Only items without a parent (top-level).
    pub top_level: Option<bool>,
    /// Only items updated after this RFC 3339 timestamp.
    pub updated_after: Option<String>,
    /// Filter by Project key (Organization/My Work/profile collections).
    pub projects: Vec<String>,
    /// Filter by Organization slug (profile Work only).
    pub organizations: Vec<String>,
    /// Restrict to involvement kind: `assigned`, `created`, or `commented`
    /// (profile Work only).
    pub involvement: Vec<String>,
    /// Only items past their due date.
    pub overdue: Option<bool>,
    /// Only items due strictly before this RFC 3339 timestamp.
    pub due_before: Option<String>,
    /// Only items due strictly after this RFC 3339 timestamp.
    pub due_after: Option<String>,
    /// Result ordering: `updated`, `dueDate`, `priority`, or `rank`.
    pub sort: Option<String>,
    /// Only archived (`true`) or only unarchived (`false`) items; the server
    /// treats an omitted value as `false`. This is an archived-state filter,
    /// not a union: listing both states requires separate queries.
    pub archived: Option<bool>,
    /// Sparse fieldset: comma-separated summary field names; an empty value
    /// selects the complete summary.
    pub fields: Option<String>,
}

/// The authenticated user's identity, from `GET /me`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Me {
    /// The user's legacy internal id (retained for v1 compatibility).
    pub id: String,
    /// The user's immutable public identifier (e.g. `usr_...`).
    pub public_id: String,
    /// The user's display name.
    pub name: String,
    /// The user's email address (private; only in the caller's own `/me`).
    pub email: String,
    /// How the request was authenticated.
    pub authentication: AuthenticationContext,
    /// The user's default organization, when one is set.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub default_organization: Option<OrganizationSummary>,
    /// The Organizations available through the credential, each with the
    /// Organization-scoped username.
    pub organizations: Vec<MeOrganization>,
}

/// An Organization available to the authenticated user.
///
/// A username is a membership identity unique only inside its Organization
/// and may be absent when the membership has no username.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeOrganization {
    /// The organization's id.
    pub id: String,
    /// The organization's URL slug.
    pub slug: String,
    /// The organization's display name.
    pub name: String,
    /// The user's username inside this Organization, when one is set.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub username: Option<String>,
}

/// How the current request was authenticated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticationContext {
    /// Credential kind (e.g. `pat`).
    #[serde(rename = "type")]
    pub auth_type: String,
    /// The server-side credential id.
    pub credential_id: String,
    /// The credential's human-readable name.
    pub credential_name: String,
    /// Scopes granted to the credential.
    pub scopes: Vec<String>,
    /// When the credential expires (RFC 3339).
    pub expires_at: String,
}

/// A compact organization reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizationSummary {
    /// The organization's id.
    pub id: String,
    /// The organization's URL slug.
    pub slug: String,
    /// The organization's display name.
    pub name: String,
}

/// A full organization resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Organization {
    /// The organization's id.
    pub id: String,
    /// The organization's URL slug.
    pub slug: String,
    /// The organization's display name.
    pub name: String,
    /// Free-form description, when set.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub description: Option<String>,
    /// Billing plan identifier.
    pub plan: String,
    /// The current user's role in the organization.
    pub role: String,
    /// Whether this is the user's default organization.
    pub is_default: bool,
    /// Whether the organization is suspended.
    pub suspended: bool,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
    /// Last-update timestamp (RFC 3339).
    pub updated_at: String,
}

/// A member of an organization list response.
///
/// The contract allows either a full [`Organization`] or a compact summary, so
/// the richer fields are optional to satisfy both shapes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationListItem {
    /// The organization's id.
    pub id: String,
    /// The organization's URL slug.
    pub slug: String,
    /// The organization's display name.
    pub name: String,
    /// Whether the organization is suspended.
    pub suspended: bool,
    /// Free-form description, when set.
    #[serde(default)]
    pub description: Option<String>,
    /// Billing plan identifier, when included.
    #[serde(default)]
    pub plan: Option<String>,
    /// The current user's role, when included.
    #[serde(default)]
    pub role: Option<String>,
    /// Whether this is the user's default organization, when included.
    #[serde(default)]
    pub is_default: Option<bool>,
    /// Creation timestamp, when included.
    #[serde(default)]
    pub created_at: Option<String>,
    /// Last-update timestamp, when included.
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A paginated organization list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizationList {
    /// One page of organizations.
    pub items: Vec<OrganizationListItem>,
    /// Pagination metadata.
    pub page: Page,
}

/// An active member row from the Organization member directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationUser {
    /// The member's immutable public identifier (`usr_...`).
    pub public_id: String,
    /// The member's display name.
    pub name: String,
    /// The member's username inside this Organization, when one is set.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub username: Option<String>,
}

/// A paginated Organization member directory page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizationUserList {
    /// One page of member rows.
    pub items: Vec<OrganizationUser>,
    /// Pagination metadata.
    pub page: Page,
}

/// An authenticated profile summary (`GET /users/{publicId}`).
///
/// Never contains an email address, an internal UUID, or hidden work data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserProfile {
    /// The profile's immutable public identifier.
    pub public_id: String,
    /// The user's display name.
    pub name: String,
    /// Canonical avatar URL, when an avatar exists.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub avatar_url: Option<String>,
    /// When the account joined (RFC 3339).
    pub joined_at: String,
    /// Whether the profile belongs to the authenticated caller.
    pub is_current_user: bool,
    /// Shared Organizations visible to the viewer, with the target's
    /// Organization-paired usernames.
    pub shared_organizations: Vec<ProfileOrganization>,
    /// Visibility-scoped activity statistics.
    pub stats: ProfileStats,
}

/// One Organization-paired username on a profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileOrganization {
    /// The Organization the username belongs to.
    pub organization: OrganizationSummary,
    /// The target user's username inside that Organization, when set.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub username: Option<String>,
}

/// The five visibility-scoped statistics on a profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStats {
    /// Distinct visible Projects involving the target.
    pub projects: i64,
    /// Visible Work Items currently assigned to the target.
    pub work_items_assigned: i64,
    /// Visible Work Items reported by the target.
    pub work_items_created: i64,
    /// Visible assigned Work Items in the canonical done status.
    pub work_items_completed: i64,
    /// Visible non-deleted comments authored by the target.
    pub comments: i64,
}

/// A project resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// The project's id.
    pub id: String,
    /// The id of the organization the project belongs to.
    pub organization_id: String,
    /// The project's short key (used in paths).
    pub key: String,
    /// The project's display name.
    pub name: String,
    /// Free-form description, when set.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub description: Option<String>,
    /// Display color as a hex string.
    pub color: String,
    /// Optimistic-concurrency revision (matches the `project-N` `ETag`).
    pub revision: i64,
    /// When the Project was archived; null while unarchived.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub archived_at: Option<String>,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
    /// Last-update timestamp (RFC 3339).
    pub updated_at: String,
}

/// A paginated project list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectList {
    /// One page of projects.
    pub items: Vec<Project>,
    /// Pagination metadata.
    pub page: Page,
}

/// A compact user reference.
///
/// The legacy v1 shape is `{id, name}`; newer projections use
/// `{publicId, name}`. Both decode into this enum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum UserSummary {
    /// Legacy v1 summary: `{id, name}`.
    Legacy {
        /// The user's legacy internal id.
        id: String,
        /// The user's display name.
        name: String,
    },
    /// Public summary: `{publicId, name}`.
    Public {
        /// The user's immutable public id (`usr_...`).
        #[serde(rename = "publicId")]
        public_id: String,
        /// The user's display name.
        name: String,
    },
}

impl UserSummary {
    /// The user's display name regardless of projection shape.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Legacy { name, .. } | Self::Public { name, .. } => name,
        }
    }

    /// The immutable public id, when the projection carries one.
    #[must_use]
    pub fn public_id(&self) -> Option<&str> {
        match self {
            Self::Public { public_id, .. } => Some(public_id),
            Self::Legacy { .. } => None,
        }
    }

    /// The legacy internal id, when the projection carries one.
    #[must_use]
    pub fn legacy_id(&self) -> Option<&str> {
        match self {
            Self::Legacy { id, .. } => Some(id),
            Self::Public { .. } => None,
        }
    }
}

/// A compact sprint reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SprintSummary {
    /// The sprint's id.
    pub id: String,
    /// The sprint's display name.
    pub name: String,
    /// The sprint's state (server-defined).
    pub state: String,
}

/// A full Sprint resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sprint {
    /// The sprint's id.
    pub id: String,
    /// The sprint's display name.
    pub name: String,
    /// The sprint's lifecycle state (`future`, `active`, or `done`).
    pub state: String,
    /// Planned start date (RFC 3339), when the Sprint is dated.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub start_date: Option<String>,
    /// Planned end date (RFC 3339), when the Sprint is dated.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub end_date: Option<String>,
    /// The Sprint goal, when set.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub goal: Option<String>,
    /// The target story-point total, when set.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub target_points: Option<i64>,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
    /// Last-update timestamp (RFC 3339).
    pub updated_at: String,
    /// When the Sprint was archived; null while unarchived.
    pub archived_at: Option<String>,
    /// Optimistic-concurrency revision (matches the `sprint-N` `ETag`).
    pub revision: i64,
}

/// A paginated Sprint list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SprintList {
    /// One page of sprints.
    pub items: Vec<Sprint>,
    /// Pagination metadata.
    pub page: Page,
}

/// One permitted Sprint state transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintTransition {
    /// The state this transition moves the Sprint to (`active` or `done`).
    pub target_state: String,
    /// Whether completing this Sprint requires a completion action
    /// (unfinished Work Items remain).
    pub requires_completion_action: bool,
}

/// The transition surface of a Sprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintTransitionList {
    /// The Sprint's current state.
    pub current_state: String,
    /// Transitions permitted from the current state.
    pub transitions: Vec<SprintTransition>,
}

/// A label resource on a work item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    /// The label's id.
    pub id: String,
    /// The label's display name.
    pub name: String,
    /// Display color as a hex string.
    pub color: String,
}

/// A Project label resource (from the Project label list/create endpoints).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectLabel {
    /// The label's id.
    pub id: String,
    /// The label's display name (stored lowercase).
    pub name: String,
    /// Display color as a hex string.
    pub color: String,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
}

/// A paginated Project label list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectLabelList {
    /// One page of labels.
    pub items: Vec<ProjectLabel>,
    /// Pagination metadata.
    pub page: Page,
}

/// A work item in list responses (summary shape).
///
/// Only `id`, `key`, and `revision` are guaranteed when the caller supplies a
/// sparse `fields=` fieldset, so every other summary field is optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemSummary {
    /// The work item's id.
    pub id: String,
    /// The work item's human key (e.g. `HAM-1`).
    pub key: String,
    /// Optimistic-concurrency revision.
    pub revision: i64,
    /// The work item's title.
    #[serde(default)]
    pub title: Option<String>,
    /// The work item type (e.g. `task`).
    #[serde(default, rename = "type")]
    pub item_type: Option<String>,
    /// The current status.
    #[serde(default)]
    pub status: Option<String>,
    /// The current priority.
    #[serde(default)]
    pub priority: Option<String>,
    /// The assignee, when assigned.
    #[serde(default)]
    pub assignee: Option<UserSummary>,
    /// The sprint, when scheduled.
    #[serde(default)]
    pub sprint: Option<SprintSummary>,
    /// The parent work item id, when nested.
    #[serde(default)]
    pub parent_id: Option<String>,
    /// Story points, when estimated.
    #[serde(default)]
    pub story_points: Option<i64>,
    /// Due date (RFC 3339), when set.
    #[serde(default)]
    pub due_date: Option<String>,
    /// When the item was archived; null while unarchived.
    #[serde(default)]
    pub archived_at: Option<String>,
    /// Creation timestamp (RFC 3339), when included.
    #[serde(default)]
    pub created_at: Option<String>,
    /// Last-update timestamp (RFC 3339), when included.
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// A Work Item collection item carrying its Organization and Project context
/// (Organization Work, My Work, and profile Work collections).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemContextSummary {
    /// The summary fields.
    #[serde(flatten)]
    pub summary: WorkItemSummary,
    /// The Project the item belongs to.
    pub project: ProjectContext,
    /// The Organization the Project belongs to.
    pub organization: OrganizationSummary,
}

/// The Project context attached to context-collection rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectContext {
    /// The project's id.
    pub id: String,
    /// The project's short key.
    pub key: String,
    /// The project's display name.
    pub name: String,
    /// Display color as a hex string.
    pub color: String,
}

/// An Organization, My Work, or profile Work Item page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemContextList {
    /// One page of context summaries.
    pub items: Vec<WorkItemContextSummary>,
    /// Pagination metadata.
    pub page: Page,
}

/// A profile Work Item collection row.
///
/// The profile Work projection extends the context summary with the item's
/// reporter. The reporter is present only in the complete projection; a
/// sparse `fields=` selection omits it because `reporter` is not a Work Item
/// summary field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileWorkItemSummary {
    /// The context summary fields.
    #[serde(flatten)]
    pub summary: WorkItemContextSummary,
    /// The reporter, when recorded.
    #[serde(default)]
    pub reporter: Option<UserSummary>,
}

/// A profile Work Item page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileWorkItemList {
    /// One page of profile Work summaries.
    pub items: Vec<ProfileWorkItemSummary>,
    /// Pagination metadata.
    pub page: Page,
}

/// The parent reference on a full work item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemParent {
    /// The parent's id.
    pub id: String,
    /// The parent's human key.
    pub key: String,
    /// The parent's title.
    pub title: String,
    /// The parent's type.
    #[serde(rename = "type")]
    pub item_type: String,
    /// The parent's status.
    pub status: String,
    /// The parent's priority.
    pub priority: String,
}

/// A full work item resource.
///
/// Assignee/reporter summaries keep the legacy `{id, name}` shape on v1 read
/// and update surfaces but arrive as `{publicId, name}` on the archive,
/// unarchive, and label-assignment mutation responses; both decode here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItem {
    /// The work item's id.
    pub id: String,
    /// The work item's human key (e.g. `HAM-1`).
    pub key: String,
    /// The id of the project the item belongs to.
    pub project_id: String,
    /// The work item's title.
    pub title: String,
    /// Free-form description, when set.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub description: Option<String>,
    /// The work item type (e.g. `task`).
    #[serde(rename = "type")]
    pub item_type: String,
    /// The current status.
    pub status: String,
    /// The current priority.
    pub priority: String,
    /// The assignee, when assigned.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub assignee: Option<UserSummary>,
    /// The reporter, when recorded.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub reporter: Option<UserSummary>,
    /// The sprint, when scheduled.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub sprint: Option<SprintSummary>,
    /// The parent work item, when nested.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub parent: Option<WorkItemParent>,
    /// Labels attached to the item.
    pub labels: Vec<Label>,
    /// Story points, when estimated.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub story_points: Option<i64>,
    /// Due date (RFC 3339), when set.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub due_date: Option<String>,
    /// When the item was archived; null while unarchived.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub archived_at: Option<String>,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
    /// Last-update timestamp (RFC 3339).
    pub updated_at: String,
    /// Optimistic-concurrency revision (matches the `ETag`).
    pub revision: i64,
}

/// A paginated work item list (summary shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemList {
    /// One page of work item summaries.
    pub items: Vec<WorkItemSummary>,
    /// Pagination metadata.
    pub page: Page,
}

/// One permitted status transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemTransition {
    /// The status this transition moves the item to.
    pub target_status: String,
}

/// The transition surface of a work item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemTransitionList {
    /// The item's current status.
    pub current_status: String,
    /// Transitions permitted from the current status.
    pub transitions: Vec<WorkItemTransition>,
}

/// The authenticated user's watcher state on a work item.
///
/// Other watchers are never disclosed by the Public API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemWatcher {
    /// The watched work item's id.
    pub work_item_id: String,
    /// Whether the item currently shows up in the user's watch feed.
    pub watched: bool,
    /// Whether the user watches the item explicitly.
    pub manual_watch: bool,
    /// Whether the watch originates from being the assignee.
    pub assignee_origin: bool,
    /// Whether notifications for the item are muted.
    pub muted: bool,
    /// Whether the user is the item's assignee.
    pub assigned: bool,
}

/// A watcher action (`watch`, `unwatch`, `mute`, `unmute`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatcherAction {
    /// Start watching the item.
    #[serde(rename = "watch")]
    Watch,
    /// Stop watching the item.
    #[serde(rename = "unwatch")]
    Unwatch,
    /// Keep watching but suppress notifications.
    #[serde(rename = "mute")]
    Mute,
    /// Stop suppressing notifications.
    #[serde(rename = "unmute")]
    Unmute,
}

impl WatcherAction {
    /// The wire spelling of the action.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Watch => "watch",
            Self::Unwatch => "unwatch",
            Self::Mute => "mute",
            Self::Unmute => "unmute",
        }
    }
}

/// The body of a watcher action request (`action` only; server sets the rest).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemWatcherActionRequest {
    /// The action to apply to the authenticated user's watcher state.
    pub action: WatcherAction,
}

/// A comment resource.
///
/// List/create/delete responses retain the legacy `{id, name}` author
/// summary; the edit response uses the `{publicId, name}` projection. Both
/// decode into [`UserSummary`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    /// The comment's id.
    pub id: String,
    /// The id of the work item the comment belongs to.
    pub work_item_id: String,
    /// The parent comment id, when this is a reply.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub parent_comment_id: Option<String>,
    /// The comment's author.
    pub author: UserSummary,
    /// The comment body; absent when the comment was deleted.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub body: Option<String>,
    /// Whether the comment was deleted.
    pub deleted: bool,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
    /// Last-update timestamp (RFC 3339).
    pub updated_at: String,
    /// When the comment was last edited; creation and deletion leave it unset.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub edited_at: Option<String>,
}

/// A paginated comment list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentList {
    /// One page of comments.
    pub items: Vec<Comment>,
    /// Pagination metadata.
    pub page: Page,
}

/// A Work Item attachment resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// The attachment's id.
    pub id: String,
    /// The id of the work item the attachment belongs to.
    pub work_item_id: String,
    /// The sanitized file name.
    pub file_name: String,
    /// The MIME content type.
    pub content_type: String,
    /// The file size in bytes.
    pub size: i64,
    /// The creator, when recorded (legacy `{id, name}` summary).
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub created_by: Option<UserSummary>,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
}

/// A paginated attachment list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentList {
    /// One page of attachments.
    pub items: Vec<Attachment>,
    /// Pagination metadata.
    pub page: Page,
}

/// A Work Item link entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemLink {
    /// The link's id.
    pub id: String,
    /// The relation as seen from the requested Work Item.
    pub relation: String,
    /// The Work Item on the other side of the link.
    pub other_work_item: LinkOtherWorkItem,
    /// The link creator, when recorded.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub created_by: Option<UserSummary>,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
}

/// The other Work Item referenced by a link.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkOtherWorkItem {
    /// The other work item's id.
    pub id: String,
    /// The other work item's human key.
    pub key: String,
    /// The other work item's Project summary.
    pub project: LinkProjectSummary,
    /// The other work item's title.
    pub title: String,
    /// The other work item's type.
    #[serde(rename = "type")]
    pub item_type: String,
    /// The other work item's status.
    pub status: String,
}

/// The Project summary attached to a link's other Work Item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkProjectSummary {
    /// The project's id.
    pub id: String,
    /// The project's short key.
    pub key: String,
    /// The project's display name.
    pub name: String,
}

/// A paginated Work Item link list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemLinkList {
    /// One page of links.
    pub items: Vec<WorkItemLink>,
    /// Pagination metadata.
    pub page: Page,
}

/// One redacted activity event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    /// The activity event's id.
    pub id: String,
    /// The action kind (e.g. `created`, `status_changed`).
    pub action: String,
    /// The acting user, or null when the actor was deleted.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub actor: Option<UserSummary>,
    /// The action-specific detail projection, or null.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub detail: Option<Value>,
    /// Creation timestamp (RFC 3339).
    pub created_at: String,
}

/// A paginated Work Item activity page (oldest first).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityList {
    /// One page of activity events.
    pub items: Vec<Activity>,
    /// Pagination metadata.
    pub page: Page,
}

/// The Work Item summary attached to Project/profile activity rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItemRefSummary {
    /// The work item's id.
    pub id: String,
    /// The work item's human key.
    pub key: String,
    /// The work item's title.
    pub title: String,
}

/// A Project activity event (newest first), with its Work Item summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectActivity {
    /// The activity fields.
    #[serde(flatten)]
    pub activity: Activity,
    /// The Work Item the event happened on.
    pub work_item: WorkItemRefSummary,
}

/// A paginated Project activity page (newest first).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectActivityList {
    /// One page of activity events.
    pub items: Vec<ProjectActivity>,
    /// Pagination metadata.
    pub page: Page,
}

/// A profile activity event with its authorized context summaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileActivity {
    /// The activity fields.
    #[serde(flatten)]
    pub activity: Activity,
    /// The Organization the event happened in.
    pub organization: OrganizationSummary,
    /// The Project the event happened in.
    pub project: LinkProjectSummary,
    /// The Work Item the event happened on.
    pub work_item: WorkItemRefSummary,
}

/// A paginated profile activity page (newest first).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileActivityList {
    /// One page of activity events.
    pub items: Vec<ProfileActivity>,
    /// Pagination metadata.
    pub page: Page,
}

/// Body for `POST .../work-items`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkItemRequest {
    /// The new item's title (required).
    pub title: String,
    /// The new item's description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The item type, when not the server default.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    /// The initial status, when not the server default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The priority, when not the server default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// The assignee's legacy user id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee_id: Option<String>,
    /// The assignee's immutable public id (`usr_...`); preferred over
    /// `assigneeId` on new bodies. The two fields are mutually exclusive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee_public_id: Option<String>,
    /// The sprint id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sprint_id: Option<String>,
    /// Parent Work Item identifier accepted by the server (key or id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Story points.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub story_points: Option<i64>,
    /// Due date (RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<String>,
}

/// Body for `PATCH .../work-items/{key}`.
///
/// Uses tri-state `Option<Option<T>>` fields: outer `None` omits the field,
/// `Some(None)` sends an explicit `null` (clear), `Some(Some(v))` sets a value.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateWorkItemRequest {
    /// New title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// New description; `Some(None)` clears it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Option<String>>,
    /// New item type.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    /// New priority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// New assignee (legacy id); `Some(None)` unassigns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee_id: Option<Option<String>>,
    /// New assignee (public id, preferred); `Some(None)` unassigns.
    /// Mutually exclusive with `assigneeId`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee_public_id: Option<Option<String>>,
    /// New sprint; `Some(None)` unschedules.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sprint_id: Option<Option<String>>,
    /// New parent identifier; `Some(None)` detaches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<Option<String>>,
    /// New story points; `Some(None)` clears the estimate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub story_points: Option<Option<i64>>,
    /// New due date; `Some(None)` clears it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<Option<String>>,
}

impl UpdateWorkItemRequest {
    /// Returns true when the request would send at least one field.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.item_type.is_none()
            && self.priority.is_none()
            && self.assignee_id.is_none()
            && self.assignee_public_id.is_none()
            && self.sprint_id.is_none()
            && self.parent_id.is_none()
            && self.story_points.is_none()
            && self.due_date.is_none()
    }
}

/// Body for `POST .../transitions`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionRequest {
    /// The status to move the item to (validated by the server).
    pub target_status: String,
}

/// Body for `POST .../comments`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCommentRequest {
    /// The comment body.
    pub body: String,
    /// The parent comment id, when replying.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_comment_id: Option<String>,
}

/// Body for `PATCH .../comments/{commentId}` (author-only edit).
#[derive(Debug, Clone, Serialize)]
pub struct UpdateCommentRequest {
    /// The replacement comment body (same 2,000-character limit as creation).
    pub body: String,
}

/// Body for `POST .../work-items/{key}/links`.
///
/// Exactly one of `targetKey` or `targetId` must be provided.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkItemLinkRequest {
    /// The target Work Item's id (UUID).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    /// The target Work Item's human key (e.g. `HAM-43`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_key: Option<String>,
    /// The relation: `blocks`, `blocked_by`, or `relates`.
    pub relation: String,
}

/// Body for `PATCH .../projects/{key}`.
///
/// At least one field must be present; the Project key cannot change.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectRequest {
    /// New display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// New description; `Some(None)` clears it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<Option<String>>,
    /// New display color as a hex string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl UpdateProjectRequest {
    /// Returns true when the request would send at least one field.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.description.is_none() && self.color.is_none()
    }
}

/// Body for `POST .../projects`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectRequest {
    /// The new project's name (required).
    pub name: String,
    /// The project's short key; omitted to use the server's canonical
    /// suggestion from the name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Free-form description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Display color as a hex string (e.g. `#3b82f6`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Body for `POST .../sprints`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSprintRequest {
    /// The new sprint's name (required).
    pub name: String,
    /// Planned start date (RFC 3339); `Some(None)` sends an explicit null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_date: Option<Option<String>>,
    /// Planned end date (RFC 3339); `Some(None)` sends an explicit null.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_date: Option<Option<String>>,
    /// The Sprint goal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// The target story-point total.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_points: Option<i64>,
}

/// What happens to unfinished Work Items when a Sprint is completed.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "mode", rename_all_fields = "camelCase")]
pub enum CompletionAction {
    /// Move unfinished Work Items back to the backlog.
    #[serde(rename = "backlog")]
    Backlog,
    /// Move unfinished Work Items to a future Sprint in the same Project.
    #[serde(rename = "sprint")]
    Sprint {
        /// The future Sprint that receives the unfinished Work Items.
        target_sprint_id: String,
    },
}

/// Body for `POST .../sprints/{id}/transitions`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransitionSprintRequest {
    /// The state to move the Sprint to (`active` or `done`).
    pub target_state: String,
    /// Required when completing a Sprint with unfinished Work Items.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_action: Option<CompletionAction>,
}

/// Body for `POST .../labels` (create a Project label).
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLabelRequest {
    /// The label's name (trimmed and stored lowercase by the server).
    pub name: String,
    /// Display color as a hex string; the server defaults to `#6366f1`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Body for attaching a Project label to a Work Item.
///
/// The live contract accepts either the label id (`labelId`) or its Project-
/// scoped name (`label`). Callers must set exactly one selector.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachLabelRequest {
    /// Project label id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_id: Option<String>,
    /// Project-scoped label name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Body for read-only Organization Work Item search with SqueakQL.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SqueakQlSearchRequest {
    /// SqueakQL expression (1–4,096 characters, containing non-whitespace).
    pub query: String,
    /// Maximum rows for this page (1–200; server default 50).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Opaque continuation cursor returned by an earlier SqueakQL search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Body for validating a SqueakQL expression without executing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SqueakQlValidateRequest {
    /// SqueakQL expression (1–4,096 characters, containing non-whitespace).
    pub query: String,
}

/// Stable diagnostic codes returned by the SqueakQL validator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SqueakQlDiagnosticCode {
    /// The expression is not syntactically valid.
    SqueakqlSyntaxError,
    /// The expression references an unknown field.
    SqueakqlUnknownField,
    /// The expression references an unknown function.
    SqueakqlUnknownFunction,
    /// An operand has an incompatible type.
    SqueakqlTypeMismatch,
    /// The selected operator is unsupported for its operands.
    SqueakqlUnsupportedOperator,
    /// A literal or other value is invalid.
    SqueakqlInvalidValue,
    /// The expression exceeds the server's complexity limit.
    SqueakqlQueryTooComplex,
}

impl SqueakQlDiagnosticCode {
    /// Returns the stable code exactly as it appears on the wire.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SqueakqlSyntaxError => "SQUEAKQL_SYNTAX_ERROR",
            Self::SqueakqlUnknownField => "SQUEAKQL_UNKNOWN_FIELD",
            Self::SqueakqlUnknownFunction => "SQUEAKQL_UNKNOWN_FUNCTION",
            Self::SqueakqlTypeMismatch => "SQUEAKQL_TYPE_MISMATCH",
            Self::SqueakqlUnsupportedOperator => "SQUEAKQL_UNSUPPORTED_OPERATOR",
            Self::SqueakqlInvalidValue => "SQUEAKQL_INVALID_VALUE",
            Self::SqueakqlQueryTooComplex => "SQUEAKQL_QUERY_TOO_COMPLEX",
        }
    }
}

/// One source-located SqueakQL validation diagnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqueakQlDiagnostic {
    /// Stable diagnostic code.
    pub code: SqueakQlDiagnosticCode,
    /// Human-readable explanation.
    pub message: String,
    /// One-based source line.
    pub line: u32,
    /// One-based source column.
    pub column: u32,
    /// Optional one-based inclusive ending line.
    #[serde(default)]
    pub end_line: Option<u32>,
    /// Optional one-based inclusive ending column.
    #[serde(default)]
    pub end_column: Option<u32>,
    /// The offending source token, when available.
    #[serde(default)]
    pub token: Option<String>,
    /// Values or constructs expected at the diagnostic location.
    #[serde(default)]
    pub expected: Option<Vec<String>>,
    /// A server-provided correction hint, when available.
    #[serde(default)]
    pub suggestion: Option<String>,
}

/// Result of validating a SqueakQL expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqueakQlValidationResponse {
    /// Whether the expression is valid.
    pub valid: bool,
    /// SqueakQL language version (currently `1`).
    pub language_version: u32,
    /// Source-located diagnostics; empty when valid.
    pub errors: Vec<SqueakQlDiagnostic>,
}

/// Body for `POST .../bulk-work-items`.
///
/// The envelope is modeled exactly so automation can use the public request
/// schema directly while the server remains authoritative for validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BulkCreateWorkItemOperation {
    /// Project key that receives the Work Item.
    #[serde(rename = "projectKey")]
    pub project_key: String,
    /// Work Item title.
    pub title: String,
}

/// One bulk Work Item update operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BulkUpdateWorkItemOperation {
    /// Project key containing the Work Item.
    pub project_key: String,
    /// Work Item key to update.
    pub work_item_key: String,
    /// Required in `require-revision` mode; omitted for last-write-wins.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<i64>,
    /// Mutable Work Item fields.
    pub changes: UpdateWorkItemRequest,
}

/// One bulk Work Item transition operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BulkTransitionWorkItemOperation {
    /// Project key containing the Work Item.
    pub project_key: String,
    /// Work Item key to transition.
    pub work_item_key: String,
    /// Required in `require-revision` mode; omitted for last-write-wins.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<i64>,
    /// Target Work Item status.
    pub target_status: String,
}

/// Body for `POST .../bulk-work-items`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BulkCreateEnvelope {
    /// 1–50 create operations.
    pub operations: Vec<BulkCreateWorkItemOperation>,
}

/// Body for `PATCH .../bulk-work-items`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BulkUpdateEnvelope {
    /// `require-revision` or `last-write-wins` (required by the contract).
    pub concurrency: String,
    /// 1–50 update operations.
    pub operations: Vec<BulkUpdateWorkItemOperation>,
}

/// Body for `POST .../bulk-work-item-transitions`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BulkTransitionEnvelope {
    /// `require-revision` (default) or `last-write-wins`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<String>,
    /// 1–50 transition operations.
    pub operations: Vec<BulkTransitionWorkItemOperation>,
}

/// One per-item bulk result, in input order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkResult {
    /// The input operation index this result corresponds to.
    pub index: i64,
    /// The per-item HTTP status (201/200 on success, 400/404/409 on failure).
    pub status: i64,
    /// The created/updated Work Item projection, when the item succeeded.
    #[serde(default)]
    pub work_item: Option<WorkItem>,
    /// The embedded error, when the item failed.
    #[serde(default)]
    pub error: Option<std::collections::BTreeMap<String, Value>>,
}

/// The bulk result list (`{results: [...]}`); there is no `page` envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkResultList {
    /// Per-item results in input order.
    pub results: Vec<BulkResult>,
}

/// Query options for `GET .../projects/{key}/reports/{type}`.
///
/// Every value is forwarded verbatim; the server decides which combinations a
/// given report type accepts and reports invalid ones. The CLI performs no
/// validation beyond rejecting empty strings and never computes report values
/// itself.
#[derive(Debug, Clone, Default)]
pub struct ProjectReportOptions {
    /// Maximum items per page (server-capped at 100).
    pub limit: Option<u32>,
    /// Opaque continuation cursor from a previous page's `nextCursor`.
    pub cursor: Option<String>,
    /// Recent-Sprint count or calendar-day window.
    pub range: Option<u32>,
    /// Inclusive ISO calendar date bounding the window.
    pub start: Option<String>,
    /// Inclusive ISO calendar date bounding the window.
    pub end: Option<String>,
    /// IANA time zone the calendar window is evaluated in.
    pub time_zone: Option<String>,
    /// Measurement unit (`count`, `points`, `items`).
    pub unit: Option<String>,
    /// Sampling interval (`day`, `week`, `month`).
    pub interval: Option<String>,
    /// Time measure (`cycle`, `lead`).
    pub measure: Option<String>,
    /// Canonical status whose first entry starts cycle time.
    pub cycle_start_status: Option<String>,
    /// Rolling window size for moving measures.
    pub window: Option<u32>,
    /// Grouping dimension (`status`, `type`, `priority`, `assignee`, `label`).
    pub group_by: Option<String>,
    /// Item scope (`open`, `all`).
    pub scope: Option<String>,
    /// Sprint id the report is restricted to.
    pub sprint: Option<String>,
    /// Result ordering key.
    pub sort: Option<String>,
    /// Free-text query filter.
    pub query: Option<String>,
    /// SqueakQL filter expression.
    pub squeakql: Option<String>,
    /// Status filter.
    pub status: Option<String>,
    /// Work Item type filter (contract parameter `type`).
    pub work_type: Option<String>,
    /// Priority filter.
    pub priority: Option<String>,
    /// Assignee filter.
    pub assignee: Option<String>,
    /// Label filter.
    pub label: Option<String>,
    /// Bucket count or explicit bucket boundaries for distribution reports.
    pub buckets: Option<String>,
}

/// A project reporting series or rollup (`GET .../projects/{key}/reports/{type}`).
///
/// `items` and `data` hold server-shaped rows whose columns differ per report
/// kind, so they stay as raw JSON objects: the server payload is authoritative
/// and the CLI neither narrows nor recomputes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectReport {
    /// The report kind the server produced.
    pub kind: String,
    /// Report rows; the paginated collection for this response.
    pub items: Vec<Value>,
    /// The report's series/rollup data for the requested window.
    pub data: Vec<Value>,
    /// Pagination metadata for `items`.
    pub page: Page,
    /// Server-reported caveats about the data (partial coverage, truncation).
    #[serde(default)]
    pub limitations: Vec<String>,
}

/// A Sprint delivery report (`GET .../sprints/{id}/report`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReport {
    /// The Sprint the report describes.
    pub sprint: SprintReportSprint,
    /// Sprint state at report time (`completed`, `active`, `future`).
    pub mode: String,
    /// Whether the reported numbers are considered stable by the server.
    pub stable: bool,
    /// Commitment snapshot.
    pub commitment: SprintReportCommitment,
    /// Scope planned at Sprint creation.
    pub planned_scope: SprintReportMetricCount,
    /// Completion measured against commitment and final scope.
    pub completion: SprintReportCompletion,
    /// Items added to and removed from the Sprint after commitment.
    pub scope_changes: SprintReportScopeChanges,
    /// Items carried over out of the Sprint.
    pub carryover: Vec<SprintReportCarryover>,
    /// Current per-status distribution.
    pub statuses: Vec<SprintReportStatus>,
    /// The burndown series.
    pub burndown: SprintReportBurndown,
    /// What is left in the Sprint.
    pub remaining: SprintReportMetricCount,
    /// Server-reported caveats about the data.
    #[serde(default)]
    pub limitations: Vec<String>,
    /// The paginated change feed for this Sprint.
    pub items: Vec<SprintReportFeedEntry>,
    /// Pagination metadata for `items`.
    pub page: Page,
}

/// The Sprint header of a Sprint report.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportSprint {
    /// Sprint id.
    pub id: String,
    /// Sprint name.
    pub name: String,
    /// Sprint state (`future`, `active`, `done`).
    pub state: String,
    /// Planned start, when set.
    pub start_date: Option<String>,
    /// Planned end, when set.
    pub end_date: Option<String>,
    /// Sprint goal, when set.
    pub goal: Option<String>,
    /// Target story-point total, when set.
    pub target_points: Option<i64>,
    /// Last modification timestamp of the Sprint.
    pub updated_at: String,
    /// End of the reported window, when the server fixes one.
    pub report_end_at: Option<String>,
}

/// Commitment availability and totals.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportCommitment {
    /// Whether the server has a commitment snapshot for this Sprint.
    pub available: bool,
    /// Committed item count (0 when unavailable).
    pub item_count: i64,
    /// Committed points (0 when unavailable).
    pub points: i64,
}

/// An `{itemCount, points}` metric pair.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportMetricCount {
    /// Number of Work Items.
    pub item_count: i64,
    /// Story points.
    pub points: i64,
}

/// Completion measured against original commitment and final scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportCompletion {
    /// Totals at commitment.
    pub original_commitment: SprintReportMetricCount,
    /// Totals in the final scope.
    pub final_scope: SprintReportMetricCount,
    /// Completed totals.
    pub completed: SprintReportMetricCount,
    /// How many items carry an estimate.
    pub estimated_item_count: i64,
    /// Item id sets behind the totals above.
    pub item_ids: SprintReportCompletionIds,
    /// Completion percentages for both denominators.
    pub percentages: SprintReportCompletionPercentages,
}

/// Item id sets behind the completion totals.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportCompletionIds {
    /// Item ids committed originally.
    #[serde(default)]
    pub original_commitment: Vec<String>,
    /// Item ids in the final scope.
    #[serde(default)]
    pub final_scope: Vec<String>,
    /// Item ids completed.
    #[serde(default)]
    pub completed: Vec<String>,
}

/// Completion percentages against both denominators.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportCompletionPercentages {
    /// Percentages measured against the final scope.
    pub final_scope: SprintReportPercentages,
    /// Percentages measured against the original commitment.
    pub original_commitment: SprintReportPercentages,
}

/// Item and point completion percentages, null when the denominator is zero.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportPercentages {
    /// Completion percentage counted in items.
    pub item_percent: Option<i64>,
    /// Completion percentage counted in points.
    pub point_percent: Option<i64>,
}

/// Added/removed scope after commitment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportScopeChanges {
    /// Items added after commitment.
    #[serde(default)]
    pub added: Vec<SprintReportWorkItem>,
    /// Items removed after commitment.
    #[serde(default)]
    pub removed: Vec<SprintReportWorkItem>,
}

/// One Work Item row of a Sprint report (scope change, carryover, or feed).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportWorkItem {
    /// Report-row id.
    pub id: String,
    /// Work Item id.
    pub work_item_id: String,
    /// Work Item key.
    pub key: String,
    /// Work Item title.
    pub title: String,
    /// Status at report time.
    pub status: String,
    /// Story points, when estimated.
    pub story_points: Option<i64>,
    /// When the reported change happened.
    pub occurred_at: String,
}

/// A carried-over item and where the server moved it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportCarryover {
    /// The carried Work Item.
    #[serde(flatten)]
    pub item: SprintReportWorkItem,
    /// Destination (`backlog`, or the receiving Sprint).
    pub destination: SprintReportDestination,
}

/// Where a carried item went.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportDestination {
    /// Destination kind (`backlog`, `sprint`).
    pub kind: String,
    /// Receiving Sprint id, when the kind is `sprint`.
    pub sprint_id: Option<String>,
    /// Human-readable destination name.
    pub name: String,
}

/// One status bucket of the Sprint's current distribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportStatus {
    /// Canonical status.
    pub status: String,
    /// Items in this status.
    pub item_count: i64,
    /// Points in this status.
    pub points: i64,
}

/// The Sprint burndown series.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportBurndown {
    /// Whether the server produced a burndown for this Sprint.
    pub available: bool,
    /// Remaining points per day.
    #[serde(default)]
    pub points: Vec<SprintReportBurntpoint>,
    /// Chart-ready points with labels and the ideal line.
    #[serde(default)]
    pub display: Vec<SprintReportBurntDisplay>,
}

/// One burndown sample (remaining points on a date).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportBurntpoint {
    /// Sample date (ISO calendar date).
    pub date: String,
    /// Points still remaining on that date.
    pub remaining_points: i64,
}

/// One burndown sample with the server-supplied label and ideal line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportBurntDisplay {
    /// Sample date (ISO calendar date).
    pub date: String,
    /// Short axis label for the sample.
    pub label: String,
    /// Points remaining.
    pub remaining: i64,
    /// Ideal remaining points for this date.
    pub ideal: i64,
}

/// One entry of the Sprint change feed.
///
/// The contract models the feed as a `oneOf` over scope changes and carryovers;
/// both variants share the Work Item row and differ only in which extra fields
/// are present, so they decode into one tolerant struct with optional
/// discriminators.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SprintReportFeedEntry {
    /// The Work Item the entry describes.
    #[serde(flatten)]
    pub item: SprintReportWorkItem,
    /// Entry kind (`scope_change`, `carryover`).
    pub kind: String,
    /// For a scope change, whether the item was added or removed.
    #[serde(default)]
    pub change_type: Option<String>,
    /// For a carryover, where the item went.
    #[serde(default)]
    pub destination: Option<SprintReportDestination>,
}

/// Complete input document for creating or replacing an Advanced Report.
///
/// The reporting dataset and visualization remain server-shaped JSON. Their
/// semantics evolve independently of the CLI and are validated authoritatively
/// by the Public API; the stable resource envelope stays typed here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvancedReportInput {
    /// Advanced Report definition version (currently `1`).
    pub version: u32,
    /// Report name.
    pub name: String,
    /// Report description.
    pub description: String,
    /// Organization UUID embedded in the definition.
    pub organization_id: String,
    /// Visibility (`personal` or `organization`); omitted means `personal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    /// Versioned dataset definition from the Public API contract.
    pub dataset: Value,
    /// Visualization definition from the Public API contract.
    pub visualization: Value,
}

/// Create request for an Advanced Report.
pub type CreateAdvancedReportRequest = AdvancedReportInput;
/// Full replacement request for an Advanced Report.
pub type UpdateAdvancedReportRequest = AdvancedReportInput;
/// Authorized Advanced Report definition returned by the server.
pub type AdvancedReportDefinition = AdvancedReportInput;

/// Owner projection on Advanced Reporting resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedOwner {
    /// Immutable public user identifier.
    pub public_id: String,
}

/// Server-calculated permissions on an Advanced Reporting resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedPermissions {
    /// Whether the caller may edit the resource.
    pub edit: bool,
    /// Whether the caller may delete the resource.
    pub delete: bool,
    /// Whether the caller may duplicate the resource.
    pub duplicate: bool,
}

/// Advanced Report summary/detail resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedReport {
    /// Report UUID.
    pub id: String,
    /// Report name.
    pub name: String,
    /// Report description.
    pub description: String,
    /// Visibility (`personal` or `organization`).
    pub visibility: String,
    /// Owning user.
    pub owner: AdvancedOwner,
    /// Optimistic-concurrency revision.
    pub revision: i64,
    /// Creation timestamp.
    pub created_at: String,
    /// Last-update timestamp.
    pub updated_at: String,
    /// Source visibility (`complete`, `partial`, or `unavailable`).
    pub source_availability: String,
    /// Caller permissions calculated by the server.
    pub permissions: AdvancedPermissions,
    /// Authorized definition, or null when the caller cannot receive it.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub definition: Option<AdvancedReportDefinition>,
}

/// Advanced Report detail resource.
pub type AdvancedReportDetail = AdvancedReport;
/// Advanced Report list row.
pub type AdvancedReportSummary = AdvancedReport;

/// One page of Advanced Reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedReportList {
    /// Authorized reports in this page.
    pub items: Vec<AdvancedReportSummary>,
    /// Pagination metadata.
    pub page: Page,
}

/// Request to evaluate a saved Advanced Report at its current revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvancedReportRunRequest {
    /// Revision the caller expects to evaluate.
    pub expected_revision: i64,
}

/// Result of evaluating one saved Advanced Report.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedReportRunResult {
    /// Authorized definition that was evaluated.
    pub definition: AdvancedReportDefinition,
    /// Server-produced dataset, or null when unavailable.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub dataset: Option<Value>,
    /// Server-produced presentation warning, or null.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub presentation_warning: Option<Value>,
    /// Source visibility for this evaluation.
    pub source_availability: String,
    /// Optional explanatory server message.
    #[serde(default)]
    pub message: Option<String>,
}

/// Dashboard filter document.
///
/// Each filter group is server-shaped JSON, while unknown top-level groups are
/// rejected to match the frozen Public API request contract.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvancedDashboardFilters {
    /// Project selection override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Value>,
    /// Date-range override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_range: Option<Value>,
    /// Work Item filter override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_items: Option<Value>,
}

/// Advanced Dashboard summary/detail resource.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedDashboard {
    /// Dashboard UUID.
    pub id: String,
    /// Dashboard name.
    pub name: String,
    /// Dashboard description.
    pub description: String,
    /// Visibility (`personal` or `organization`).
    pub visibility: String,
    /// Owning user.
    pub owner: AdvancedOwner,
    /// Optimistic-concurrency revision.
    pub revision: i64,
    /// Creation timestamp.
    pub created_at: String,
    /// Last-update timestamp.
    pub updated_at: String,
    /// Source visibility calculated by the server.
    pub source_availability: String,
    /// Caller permissions calculated by the server.
    pub permissions: AdvancedPermissions,
    /// Dashboard definition version (currently `1`).
    pub version: u32,
    /// Owning Organization UUID.
    pub organization_id: String,
    /// Default filters applied by the dashboard.
    pub default_filters: AdvancedDashboardFilters,
    /// Server-shaped dashboard widgets.
    pub widgets: Vec<Value>,
}

/// Advanced Dashboard detail resource.
pub type AdvancedDashboardDetail = AdvancedDashboard;
/// Advanced Dashboard list row.
pub type AdvancedDashboardSummary = AdvancedDashboard;

/// One page of Advanced Dashboards.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedDashboardList {
    /// Authorized dashboards in this page.
    pub items: Vec<AdvancedDashboardSummary>,
    /// Pagination metadata.
    pub page: Page,
}

/// Request to evaluate a saved Advanced Dashboard at its current revision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdvancedDashboardRunRequest {
    /// Revision the caller expects to evaluate.
    pub expected_revision: i64,
    /// Optional filter overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filters: Option<AdvancedDashboardFilters>,
}

/// Result of evaluating one saved Advanced Dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedDashboardRunResult {
    /// Dashboard result version (currently `1`).
    pub version: u32,
    /// Evaluated Dashboard UUID.
    pub dashboard_id: String,
    /// Evaluated revision.
    pub dashboard_revision: i64,
    /// Evaluation timestamp.
    pub evaluated_at: String,
    /// Source visibility calculated by the server.
    pub source_availability: String,
    /// Effective filters applied to the run.
    pub applied_filters: AdvancedDashboardFilters,
    /// Server-shaped widget results.
    pub widgets: Vec<Value>,
}

/// Project projection on an Advanced Report selection row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedSelectionProject {
    /// Project key.
    pub key: String,
    /// Project name.
    pub name: String,
}

/// One Work Item captured by an Advanced Report selection cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedSelectionItem {
    /// Work Item key.
    pub key: String,
    /// Project context.
    pub project: AdvancedSelectionProject,
    /// Work Item title.
    pub title: String,
    /// Captured status.
    pub status: String,
    /// Whether the item is archived.
    pub archived: bool,
    /// Whether the item is deleted.
    pub deleted: bool,
    /// Contribution captured for this result.
    pub captured_contribution: f64,
    /// Whether the source estimate was null.
    pub estimate_was_null: bool,
    /// Optional observation identifier.
    #[serde(default)]
    pub observation_key: Option<String>,
    /// Optional observation kind.
    #[serde(default)]
    pub observation_kind: Option<String>,
    /// Optional captured observation facts.
    #[serde(default)]
    pub captured_fact: Option<Value>,
}

/// One page of Work Items captured by an Advanced Report selection cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedSelectionPage {
    /// Evaluation timestamp.
    pub evaluated_at: String,
    /// Selection expiry timestamp.
    pub expires_at: String,
    /// Total items captured by the selection.
    pub total_items: u64,
    /// Captured aggregate value.
    pub captured_value: f64,
    /// Captured sample count.
    pub sample_count: u64,
    /// Sum of item contributions.
    pub contribution_sum: f64,
    /// Server aggregation identifier.
    pub aggregation: String,
    /// Server unit identifier.
    pub unit: String,
    /// Items in this page.
    pub items: Vec<AdvancedSelectionItem>,
    /// Pagination metadata.
    pub page: Page,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn work_item_roundtrips_full_resource() {
        let raw = r##"{
            "id":"1","key":"HAM-1","projectId":"2","title":"T","description":null,
            "type":"task","status":"todo","priority":"low","assignee":null,"reporter":{"id":"r","name":"R"},
            "sprint":null,"parent":null,"labels":[{"id":"l","name":"api","color":"#fff"}],
            "storyPoints":3,"dueDate":null,"archivedAt":null,
            "createdAt":"2026-01-01T00:00:00Z",
            "updatedAt":"2026-01-02T00:00:00Z","revision":4
        }"##;
        let wi: WorkItem = serde_json::from_str(raw).unwrap();
        assert_eq!(wi.item_type, "task");
        assert_eq!(wi.reporter.as_ref().unwrap().name(), "R");
        assert_eq!(wi.labels.len(), 1);
        assert_eq!(wi.revision, 4);
        assert_eq!(wi.archived_at, None);
    }

    #[test]
    fn work_item_accepts_public_user_summaries() {
        let raw = r##"{
            "id":"1","key":"HAM-1","projectId":"2","title":"T","description":null,
            "type":"task","status":"done","priority":"low",
            "assignee":{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"A"},
            "reporter":null,
            "sprint":null,"parent":null,"labels":[],
            "storyPoints":null,"dueDate":null,"archivedAt":"2026-02-01T00:00:00Z",
            "createdAt":"2026-01-01T00:00:00Z",
            "updatedAt":"2026-01-02T00:00:00Z","revision":5
        }"##;
        let wi: WorkItem = serde_json::from_str(raw).unwrap();
        assert_eq!(
            wi.assignee.as_ref().unwrap().public_id(),
            Some("usr_cPbfeqnghA-RLpDVOMQhHg")
        );
        assert_eq!(wi.archived_at.as_deref(), Some("2026-02-01T00:00:00Z"));
    }

    #[test]
    fn work_item_summary_is_sparse_tolerant() {
        let raw = r#"{"id":"1","key":"HAM-1","revision":2,"title":"Only title"}"#;
        let item: WorkItemSummary = serde_json::from_str(raw).unwrap();
        assert_eq!(item.title.as_deref(), Some("Only title"));
        assert_eq!(item.status, None);
        assert_eq!(item.updated_at, None);
    }

    #[test]
    fn organization_list_item_accepts_both_shapes() {
        let raw = r#"{"id":"1","slug":"acme","name":"Acme","suspended":false}"#;
        let item: OrganizationListItem = serde_json::from_str(raw).unwrap();
        assert_eq!(item.plan, None);
    }

    #[test]
    fn create_request_serializes_camel_case() {
        let req = CreateWorkItemRequest {
            title: "T".into(),
            story_points: Some(3),
            ..Default::default()
        };
        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["storyPoints"], 3);
        assert!(value.get("assigneeId").is_none());
        assert!(value.get("assigneePublicId").is_none());
    }

    #[test]
    fn update_request_serializes_tri_state_clear() {
        let req = UpdateWorkItemRequest {
            assignee_public_id: Some(None),
            ..Default::default()
        };
        let value = serde_json::to_value(&req).unwrap();
        assert!(value["assigneePublicId"].is_null());
        assert!(value.get("assigneeId").is_none());
    }

    #[test]
    fn me_parses_camel_case() {
        let raw = r##"{
            "id":"u1","publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"N","email":"n@x",
            "authentication":{"type":"pat","credentialId":"c","credentialName":"n","scopes":[],"expiresAt":"2027-01-01T00:00:00Z"},
            "defaultOrganization":null,
            "organizations":[{"id":"o1","slug":"acme","name":"Acme","username":"n"}]
        }"##;
        let me: Me = serde_json::from_str(raw).unwrap();
        assert_eq!(me.authentication.expires_at, "2027-01-01T00:00:00Z");
        assert_eq!(me.public_id, "usr_cPbfeqnghA-RLpDVOMQhHg");
        assert_eq!(me.organizations[0].username.as_deref(), Some("n"));
    }

    #[test]
    fn me_rejects_shape_missing_current_required_fields() {
        let raw = r##"{
            "id":"u1","name":"N","email":"n@x",
            "authentication":{"type":"pat","credentialId":"c","credentialName":"n","scopes":[],"expiresAt":"2027-01-01T00:00:00Z"},
            "defaultOrganization":null
        }"##;
        assert!(serde_json::from_str::<Me>(raw).is_err());
    }

    #[test]
    fn sprint_roundtrips_full_resource() {
        let raw = r##"{
            "id":"s1","name":"Sprint 1","state":"active","startDate":"2026-09-01T00:00:00Z",
            "endDate":null,"goal":"Ship","targetPoints":40,
            "createdAt":"2026-08-01T00:00:00Z","updatedAt":"2026-09-01T00:00:00Z","archivedAt":null,
            "revision":2
        }"##;
        let sprint: Sprint = serde_json::from_str(raw).unwrap();
        assert_eq!(sprint.state, "active");
        assert_eq!(sprint.target_points, Some(40));
        assert_eq!(sprint.revision, 2);
    }

    #[test]
    fn sprint_transition_list_parses() {
        let raw = r##"{
            "currentState":"active",
            "transitions":[{"targetState":"done","requiresCompletionAction":true}]
        }"##;
        let list: SprintTransitionList = serde_json::from_str(raw).unwrap();
        assert_eq!(list.current_state, "active");
        assert!(list.transitions[0].requires_completion_action);
    }

    #[test]
    fn attachment_parses_camel_case() {
        let raw = r##"{
            "id":"a1","workItemId":"w1","fileName":"design.png","contentType":"image/png",
            "size":1234,"createdBy":{"id":"u","name":"U"},"createdAt":"2026-01-01T00:00:00Z"
        }"##;
        let attachment: Attachment = serde_json::from_str(raw).unwrap();
        assert_eq!(attachment.file_name, "design.png");
        assert_eq!(attachment.size, 1234);
    }

    #[test]
    fn completion_action_serializes_tagged() {
        let backlog = serde_json::to_value(CompletionAction::Backlog).unwrap();
        assert_eq!(backlog["mode"], "backlog");
        let sprint = serde_json::to_value(CompletionAction::Sprint {
            target_sprint_id: "s2".into(),
        })
        .unwrap();
        assert_eq!(sprint["mode"], "sprint");
        assert_eq!(sprint["targetSprintId"], "s2");
    }

    #[test]
    fn create_sprint_request_omits_unset_nullables() {
        let req = CreateSprintRequest {
            name: "S".into(),
            ..Default::default()
        };
        let value = serde_json::to_value(&req).unwrap();
        assert!(value.get("startDate").is_none());
        assert!(value.get("targetPoints").is_none());
    }

    #[test]
    fn project_parses_revision_and_archived_at() {
        let raw = r##"{
            "id":"p1","organizationId":"o1","key":"HAM","name":"Ham","description":null,
            "color":"#3b82f6","revision":7,"archivedAt":"2026-03-01T00:00:00Z",
            "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-03-01T00:00:00Z"
        }"##;
        let project: Project = serde_json::from_str(raw).unwrap();
        assert_eq!(project.revision, 7);
        assert_eq!(project.archived_at.as_deref(), Some("2026-03-01T00:00:00Z"));
    }

    #[test]
    fn context_summary_flattens_and_carries_context() {
        let raw = r##"{
            "id":"1","key":"HAM-1","revision":1,"title":"T","status":"todo",
            "assignee":{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"A"},
            "project":{"id":"p","key":"HAM","name":"Ham","color":"#000000"},
            "organization":{"id":"o","slug":"acme","name":"Acme"}
        }"##;
        let row: WorkItemContextSummary = serde_json::from_str(raw).unwrap();
        assert_eq!(row.summary.key, "HAM-1");
        assert_eq!(row.project.key, "HAM");
        assert_eq!(row.organization.slug, "acme");
        assert_eq!(
            row.summary.assignee.as_ref().unwrap().public_id(),
            Some("usr_cPbfeqnghA-RLpDVOMQhHg")
        );
    }

    #[test]
    fn comment_parses_edited_at_and_public_author() {
        let raw = r##"{
            "id":"c1","workItemId":"w1","parentCommentId":null,
            "author":{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"A"},
            "body":"hi","deleted":false,
            "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-02T00:00:00Z",
            "editedAt":"2026-01-03T00:00:00Z"
        }"##;
        let comment: Comment = serde_json::from_str(raw).unwrap();
        assert_eq!(
            comment.author.public_id(),
            Some("usr_cPbfeqnghA-RLpDVOMQhHg")
        );
        assert_eq!(comment.edited_at.as_deref(), Some("2026-01-03T00:00:00Z"));
    }

    #[test]
    fn link_and_activity_parse() {
        let link_raw = r##"{
            "id":"l1","relation":"blocks",
            "otherWorkItem":{"id":"2","key":"HAM-2","project":{"id":"p","key":"HAM","name":"Ham"},"title":"Other","type":"task","status":"todo"},
            "createdBy":null,"createdAt":"2026-01-01T00:00:00Z"
        }"##;
        let link: WorkItemLink = serde_json::from_str(link_raw).unwrap();
        assert_eq!(link.relation, "blocks");
        assert_eq!(link.other_work_item.key, "HAM-2");

        let activity_raw = r##"{
            "id":"a1","action":"status_changed",
            "actor":{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"A"},
            "detail":{"from":"todo","to":"done"},
            "createdAt":"2026-01-01T00:00:00Z",
            "workItem":{"id":"1","key":"HAM-1","title":"T"},
            "organization":{"id":"o","slug":"acme","name":"Acme"},
            "project":{"id":"p","key":"HAM","name":"Ham"}
        }"##;
        let activity: ProfileActivity = serde_json::from_str(activity_raw).unwrap();
        assert_eq!(activity.activity.action, "status_changed");
        assert_eq!(activity.work_item.key, "HAM-1");
        assert_eq!(activity.organization.slug, "acme");
        assert_eq!(activity.project.key, "HAM");
    }

    #[test]
    fn bulk_results_parse() {
        let raw = r##"{
            "results":[
                {"index":0,"status":201,"workItem":{
                    "id":"1","key":"HAM-1","projectId":"p1","title":"T",
                    "description":null,"type":"task","status":"todo","priority":"medium",
                    "assignee":null,"reporter":null,"sprint":null,"parent":null,"labels":[],
                    "storyPoints":null,"dueDate":null,"archivedAt":null,
                    "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z",
                    "revision":1
                }},
                {"index":1,"status":400,"error":{"code":"VALIDATION_ERROR","message":"bad"}}
            ]
        }"##;
        let list: BulkResultList = serde_json::from_str(raw).unwrap();
        assert_eq!(list.results.len(), 2);
        assert_eq!(list.results[0].work_item.as_ref().unwrap().key, "HAM-1");
        assert_eq!(
            list.results[1].error.as_ref().unwrap()["code"],
            "VALIDATION_ERROR"
        );
    }

    #[test]
    fn user_summary_accepts_both_shapes() {
        let legacy: UserSummary = serde_json::from_str(r#"{"id":"u1","name":"A"}"#).unwrap();
        assert_eq!(legacy.name(), "A");
        assert_eq!(legacy.legacy_id(), Some("u1"));
        assert_eq!(legacy.public_id(), None);
        let public: UserSummary =
            serde_json::from_str(r#"{"publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"B"}"#)
                .unwrap();
        assert_eq!(public.public_id(), Some("usr_cPbfeqnghA-RLpDVOMQhHg"));
    }

    #[test]
    fn user_profile_parses() {
        let raw = r##"{
            "publicId":"usr_cPbfeqnghA-RLpDVOMQhHg","name":"Steven",
            "avatarUrl":null,"joinedAt":"2026-01-15T14:30:00.000Z",
            "isCurrentUser":false,
            "sharedOrganizations":[{"organization":{"id":"o","slug":"acme","name":"Acme"},"username":"steven"}],
            "stats":{"projects":6,"workItemsAssigned":7,"workItemsCreated":42,"workItemsCompleted":18,"comments":27}
        }"##;
        let profile: UserProfile = serde_json::from_str(raw).unwrap();
        assert_eq!(profile.stats.work_items_created, 42);
        assert_eq!(
            profile.shared_organizations[0].username.as_deref(),
            Some("steven")
        );
    }

    #[test]
    fn update_project_request_requires_a_field() {
        let req = UpdateProjectRequest {
            color: Some("#00ff00".into()),
            ..Default::default()
        };
        assert!(!req.is_empty());
        let value = serde_json::to_value(&req).unwrap();
        assert_eq!(value["color"], "#00ff00");
        assert!(value.get("name").is_none());
    }
}
