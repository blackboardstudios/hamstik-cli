// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik work` (list / view / create / edit / transitions / transition /
//! start / close / comment).
//!
//! Each subcommand family lives in its own submodule; this module only
//! dispatches and hosts the cross-cutting [`apply_filters`] helper.

mod activity;
mod archive;
mod attachment;
mod await_cmd;
mod bulk;
mod comment;
mod common;
mod label;
mod link;
mod list;
mod transitions;
mod view;
mod watcher;

use hamstik_api_client::ListWorkItemsQuery;

use crate::app::Session;
use crate::args::{WorkArgs, WorkCommand};
use crate::error::CliError;

// Re-exported for the submodules (`super::` from their point of view).
pub(crate) use super::bulk_preflight;
pub(crate) use super::dryrun;
pub(crate) use super::org::render_lines;
pub(crate) use super::{emit_json, emit_table, emit_view};
/// Runs the `work` subcommands.
pub async fn run(session: &mut Session<'_>, args: &WorkArgs) -> Result<(), CliError> {
    match &args.command {
        WorkCommand::List(list_args) => list::list(session, list_args).await,
        WorkCommand::Mine(mine_args) => list::mine(session, mine_args).await,
        WorkCommand::Search {
            query,
            file,
            saved,
            pagination,
        } => list::search(session, query, file, saved, pagination).await,
        WorkCommand::View { key } => view::view(session, key).await,
        WorkCommand::Context(args) => super::work_context::run(session, args).await,
        WorkCommand::Create(create_args) => view::create(session, create_args).await,
        WorkCommand::Edit(edit_args) => view::edit(session, edit_args).await,
        WorkCommand::Watcher(watcher_args) => watcher::watcher(session, watcher_args).await,
        WorkCommand::Transitions { key } => transitions::transitions(session, key).await,
        WorkCommand::Transition { key, target } => {
            transitions::transition_to(session, key, target.as_str()).await
        }
        WorkCommand::Start { key } => transitions::transition_to(session, key, "in_progress").await,
        WorkCommand::Close { key } => transitions::transition_to(session, key, "done").await,
        WorkCommand::Label(label_args) => label::label(session, label_args).await,
        WorkCommand::Attachment(attachment_args) => {
            attachment::attachment(session, attachment_args).await
        }
        WorkCommand::Comment(comment_args) => comment::comment(session, comment_args).await,
        WorkCommand::Link(link_args) => link::link(session, link_args).await,
        WorkCommand::Activity {
            key,
            since,
            pagination,
        } => activity::activity(session, key, since.as_deref(), pagination).await,
        WorkCommand::Archive {
            key,
            force,
            idempotency_key,
        } => archive::change_archive(session, key, true, *force, idempotency_key.as_deref()).await,
        WorkCommand::Unarchive {
            key,
            force,
            idempotency_key,
        } => archive::change_archive(session, key, false, *force, idempotency_key.as_deref()).await,
        WorkCommand::Delete {
            key,
            cascade,
            force,
            idempotency_key,
        } => archive::delete(session, key, *cascade, *force, idempotency_key.as_deref()).await,
        WorkCommand::Await(await_args) => await_cmd::await_item(session, await_args).await,
        WorkCommand::Bulk(bulk_args) => bulk::bulk(session, bulk_args).await,
    }
}
/// Applies the shared Work Item filters to a query.
pub(crate) fn apply_filters(query: &mut ListWorkItemsQuery, filters: &crate::args::WorkFilters) {
    query.q = filters.search.clone();
    query.status = filters
        .status
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();
    query.scope = filters.scope.map(|s| s.as_str().to_string());
    query.item_type = filters
        .item_type
        .iter()
        .map(|t| t.as_str().to_string())
        .collect();
    query.priority = filters
        .priority
        .iter()
        .map(|p| p.as_str().to_string())
        .collect();
    query.assignee = filters.assignee_query();
    query.sprint = filters.sprint.clone();
    query.label = filters.label.clone();
    query.label_name = filters.label_name.clone();
    query.parent = filters.parent.clone();
    query.top_level = filters.top_level;
    query.updated_after = filters.updated_after.clone();
    query.overdue = filters.overdue;
    query.due_before = filters.due_before.clone();
    query.due_after = filters.due_after.clone();
    query.sort = filters.sort.map(|s| s.as_str().to_string());
    query.archived = filters.archived;
    query.fields = filters.fields.clone();
}
