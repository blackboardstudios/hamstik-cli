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
mod bulk_csv;
mod bulk_run;
mod comment;
pub(crate) mod common;
mod create;
mod edit;
mod handoff;
mod label;
mod link;
mod list;
pub(crate) mod sort;
mod template;
mod transitions;
mod tree;
mod triage;
mod view;
mod watch;
mod watcher;

use hamstik_api_client::ListWorkItemsQuery;

use crate::app::Session;
use crate::args::{WorkArgs, WorkCommand};
use crate::error::CliError;
use crate::output::Output;
use crate::time_arg;

// Re-exported for the submodules (`super::` from their point of view).
pub(crate) use super::bulk_preflight;
pub(crate) use super::dryrun;
pub(crate) use super::org::render_lines;
pub(crate) use super::{emit_json, emit_table, emit_view, follow_policy};
/// Runs the `work` subcommands.
pub async fn run(session: &mut Session<'_>, args: &WorkArgs) -> Result<(), CliError> {
    match &args.command {
        WorkCommand::List(list_args) => list::list(session, list_args).await,
        WorkCommand::Mine(mine_args) => list::mine(session, mine_args).await,
        WorkCommand::Triage(triage_args) => triage::triage(session, triage_args).await,
        WorkCommand::Search {
            query,
            file,
            saved,
            pagination,
        } => list::search(session, query, file, saved, pagination).await,
        WorkCommand::View(args) => view::view(session, args).await,
        WorkCommand::Context(args) => super::work_context::run(session, args).await,
        WorkCommand::Tree(args) => tree::tree(session, args).await,
        WorkCommand::Create(create_args) => create::create(session, create_args).await,
        WorkCommand::Edit(edit_args) => edit::edit(session, edit_args).await,
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
        } => activity::activity(session, key, since.as_ref(), pagination).await,
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
        WorkCommand::Watch(watch_args) => watch::watch(session, watch_args).await,
        WorkCommand::Bulk(bulk_args) => bulk::bulk(session, bulk_args).await,
        WorkCommand::Export(export_args) => handoff::export(session, export_args).await,
        WorkCommand::Import(import_args) => handoff::import(session, import_args).await,
    }
}
/// Reports the resolved instant of the shared date filters on `--verbose`.
///
/// Shared by the list commands that flatten [`crate::args::WorkFilters`] so the
/// diagnostic never drifts between them.
pub(crate) fn report_filter_dates(out: &mut Output, filters: &crate::args::WorkFilters) {
    time_arg::report_resolved(
        out,
        &[
            ("--updated-after", filters.updated_after.as_ref()),
            ("--due-before", filters.due_before.as_ref()),
            ("--due-after", filters.due_after.as_ref()),
        ],
    );
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
    query.updated_after = filters.updated_after.map(|d| d.to_string());
    query.overdue = filters.overdue;
    query.due_before = filters.due_before.map(|d| d.to_string());
    query.due_after = filters.due_after.map(|d| d.to_string());
    query.sort = filters.sort.map(|s| s.as_str().to_string());
    query.archived = filters.archived;
    query.fields = filters.fields.clone();
}
