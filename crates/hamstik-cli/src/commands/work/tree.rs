// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work tree` — a bounded parent/child hierarchy with optional server links.
//!
//! Composed entirely from existing Public API v1 reads: the root's full
//! Work Item (`GET .../work-items/{key}`) and each displayed node's children
//! (`GET .../work-items?parent={key}`). With `--links`, each displayed node's
//! `GET .../work-items/{key}/links` page is annotated verbatim.
//!
//! The CLI computes no workflow meaning. Status, priority, relationship
//! spelling (`blocks` / `blocked_by` / `relates`), and every other field come
//! from the server. Readiness, blocking, and transition validity are never
//! derived client-side.
//!
//! Traversal is bounded twice: `--depth` caps descendant levels and
//! `--max-nodes` caps the total rendered nodes. Both cuts are explicit
//! ([`Marker`]); a parent/child cycle is detected and marked rather than
//! recursed forever.

use std::collections::{HashSet, VecDeque};
use std::fmt::Write as _;
use std::sync::Arc;

use serde_json::{Value, json};

use hamstik_api_client::{
    FollowPolicy, HamstikApi, ListOptions, ListWorkItemsQuery, PageItems, WorkItem, WorkItemLink,
    WorkItemParent, WorkItemSummary, follow_with,
};

use crate::app::Session;
use crate::args::{PaginationArgs, WorkTreeArgs};
use crate::error::CliError;

use super::emit_json;

/// Envelope schema version for the JSON tree document.
pub(crate) const TREE_VERSION: u64 = 1;

/// Why a node's subtree stopped short of the complete structure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Marker {
    /// The node is at the `--depth` boundary and has children.
    Depth,
    /// The `--max-nodes` budget was exhausted while expanding this node.
    Nodes,
    /// A child repeats a key already present in the tree.
    Cycle,
}

impl Marker {
    /// Stable machine reason.
    fn reason(self) -> &'static str {
        match self {
            Self::Depth => "depth",
            Self::Nodes => "nodes",
            Self::Cycle => "cycle",
        }
    }

    /// Human-readable explanation.
    fn message(self) -> &'static str {
        match self {
            Self::Depth => "depth limit reached; children not expanded",
            Self::Nodes => "node limit reached; some descendants not expanded",
            Self::Cycle => "cycle detected; already shown elsewhere in the tree",
        }
    }

    fn to_json(self) -> Value {
        json!({ "reason": self.reason(), "message": self.message() })
    }
}

/// One rendered node plus its position in the flat arena.
struct NodeData {
    key: String,
    title: String,
    item_type: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    /// The root's server-reported parent, for orientation only. Children
    /// carry `None`: their parent is the node that listed them.
    parent: Option<WorkItemParent>,
    /// Server link objects, present only when `--links`.
    links: Option<Vec<WorkItemLink>>,
    /// Child arena indices, in the (key-sorted) order they were listed.
    children: Vec<usize>,
    /// Explicit truncation markers for this node's subtree.
    markers: Vec<Marker>,
}

impl NodeData {
    fn from_item(item: &WorkItem) -> Self {
        Self {
            key: item.key.clone(),
            title: item.title.clone(),
            item_type: Some(item.item_type.clone()),
            status: Some(item.status.clone()),
            priority: Some(item.priority.clone()),
            parent: item.parent.clone(),
            links: None,
            children: Vec::new(),
            markers: Vec::new(),
        }
    }

    fn from_summary(summary: &WorkItemSummary) -> Self {
        Self {
            key: summary.key.clone(),
            title: summary.title.clone().unwrap_or_default(),
            item_type: summary.item_type.clone(),
            status: summary.status.clone(),
            priority: summary.priority.clone(),
            parent: None,
            links: None,
            children: Vec::new(),
            markers: Vec::new(),
        }
    }

    /// `title [type, status, priority]` with `-` for absent summary fields.
    fn summary_line(&self) -> String {
        format!(
            "{} [{}, {}, {}]",
            self.title,
            self.item_type.as_deref().unwrap_or("-"),
            self.status.as_deref().unwrap_or("-"),
            self.priority.as_deref().unwrap_or("-"),
        )
    }
}

/// Fetches the root item and assembles the bounded tree.
async fn build(
    session: &Session<'_>,
    args: &WorkTreeArgs,
) -> Result<(String, String, Vec<NodeData>), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = session.require_project(&selection)?;
    let api = session.api(&selection)?;

    let root_response = api
        .get_work_item(&org, &project, &args.key)
        .await
        .map_err(CliError::from_client)?;
    let root = NodeData::from_item(&root_response.value);

    let mut nodes: Vec<NodeData> = vec![root];
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(nodes[0].key.clone());
    let mut queue: VecDeque<(usize, u32)> = VecDeque::new();
    queue.push_back((0, 0));

    let max_nodes = args.max_nodes as usize;
    // Set once a node has reported the `--max-nodes` cut, so later frontier
    // nodes are not each probed for a marker that is already announced.
    let mut node_cap_announced = false;

    while let Some((index, depth)) = queue.pop_front() {
        // Links never add nodes, so every node that made it into the arena
        // gets its annotation even after the node budget is exhausted.
        if args.links && nodes[index].links.is_none() {
            let links = fetch_links(api.clone(), &org, &project, &nodes[index].key).await?;
            nodes[index].links = Some(links);
        }

        if depth >= args.depth {
            // At the boundary, probe for children with a cheap one-item read
            // so the marker reflects a real structure rather than a guess.
            if has_children(api.clone(), &org, &project, &nodes[index].key).await? {
                nodes[index].markers.push(Marker::Depth);
            }
            continue;
        }

        if nodes.len() >= max_nodes {
            // The `--max-nodes` cap is full; this node's children cannot be
            // added. Probe once so the cut is reported only when descendants
            // actually exist, and only at the first frontier node.
            if !node_cap_announced
                && has_children(api.clone(), &org, &project, &nodes[index].key).await?
            {
                nodes[index].markers.push(Marker::Nodes);
            }
            node_cap_announced = true;
            continue;
        }

        let remaining = max_nodes.saturating_sub(nodes.len());
        let (children, more) =
            fetch_children(api.clone(), &org, &project, &nodes[index].key, remaining).await?;
        if more {
            nodes[index].markers.push(Marker::Nodes);
            node_cap_announced = true;
        }

        for child in children {
            if visited.contains(&child.key) {
                let child_index = nodes.len();
                let mut cycle = NodeData::from_summary(&child);
                cycle.markers.push(Marker::Cycle);
                nodes.push(cycle);
                nodes[index].children.push(child_index);
                continue;
            }
            visited.insert(child.key.clone());
            let child_index = nodes.len();
            nodes.push(NodeData::from_summary(&child));
            nodes[index].children.push(child_index);
            queue.push_back((child_index, depth + 1));
        }
    }

    Ok((org, project, nodes))
}

/// Lists one node's children, following pages up to `max_items`.
///
/// Fetches one item beyond the cap so "there were more children than the
/// budget allowed" is distinguishable from "the collection ended exactly at
/// the cap" (the server's `hasMore` tracks pages, not the client-side cap).
/// Returns the children (key-sorted for deterministic output) and whether any
/// were dropped.
async fn fetch_children(
    api: Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
    max_items: usize,
) -> Result<(Vec<WorkItemSummary>, bool), CliError> {
    if max_items == 0 {
        return Ok((Vec::new(), true));
    }
    let probe = max_items.saturating_add(1);
    let page = follow_with(
        FollowPolicy {
            follow: true,
            max_items: Some(probe),
            start_cursor: None,
        },
        move |cursor| {
            let api = api.clone();
            let org = org.to_string();
            let project = project.to_string();
            let key = key.to_string();
            async move {
                let response = api
                    .list_work_items(
                        &org,
                        &project,
                        ListWorkItemsQuery {
                            limit: Some(PaginationArgs::MAX_PAGE_SIZE),
                            cursor,
                            parent: Some(key),
                            ..Default::default()
                        },
                    )
                    .await?;
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        },
    )
    .await
    .map_err(CliError::from_client)?;

    let mut items = page.items;
    let more = items.len() > max_items;
    items.truncate(max_items);
    items.sort_by(|a, b| a.key.cmp(&b.key));
    Ok((items, more))
}

/// A cheap existence probe for a boundary node's children.
async fn has_children(
    api: Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
) -> Result<bool, CliError> {
    let response = api
        .list_work_items(
            org,
            project,
            ListWorkItemsQuery {
                limit: Some(1),
                parent: Some(key.to_string()),
                ..Default::default()
            },
        )
        .await
        .map_err(CliError::from_client)?;
    Ok(!response.value.items.is_empty() || response.value.page.has_more)
}

/// Fetches all of a node's server-reported links, following cursors so no
/// link is silently dropped at a page boundary.
async fn fetch_links(
    api: Arc<dyn HamstikApi>,
    org: &str,
    project: &str,
    key: &str,
) -> Result<Vec<WorkItemLink>, CliError> {
    let page = follow_with(FollowPolicy::all(), {
        let api = api.clone();
        let org = org.to_string();
        let project = project.to_string();
        let key = key.to_string();
        move |cursor| {
            let api = api.clone();
            let org = org.clone();
            let project = project.clone();
            let key = key.clone();
            async move {
                let response = api
                    .list_work_item_links(
                        &org,
                        &project,
                        &key,
                        ListOptions {
                            limit: Some(PaginationArgs::DIRECTORY_PAGE_SIZE),
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
        }
    })
    .await
    .map_err(CliError::from_client)?;
    Ok(page.items)
}

/// Runs `work tree`.
pub(crate) async fn tree(session: &mut Session<'_>, args: &WorkTreeArgs) -> Result<(), CliError> {
    let (org, project, nodes) = build(session, args).await?;
    if session.json() {
        let envelope = envelope(&org, &project, args, &nodes);
        return emit_json(session, &envelope);
    }
    let doc = human(&nodes);
    session.out.line(&doc).map_err(CliError::general)
}

/// Builds the stable JSON document from the arena.
fn envelope(org: &str, project: &str, args: &WorkTreeArgs, nodes: &[NodeData]) -> Value {
    let truncated = nodes.iter().any(|node| !node.markers.is_empty());
    json!({
        "treeVersion": TREE_VERSION,
        "organization": org,
        "project": project,
        "depth": args.depth,
        "maxNodes": args.max_nodes,
        "truncated": truncated,
        "root": node_json(nodes, 0, args.links),
    })
}

/// Serializes one node and its children.
fn node_json(nodes: &[NodeData], index: usize, include_links: bool) -> Value {
    let node = &nodes[index];
    let mut value = json!({
        "key": node.key,
        "title": node.title,
        "type": node.item_type,
        "status": node.status,
        "priority": node.priority,
        "parent": node.parent,
        "children": node
            .children
            .iter()
            .map(|&child| node_json(nodes, child, include_links))
            .collect::<Vec<_>>(),
        "truncated": node.markers.iter().map(|m| m.to_json()).collect::<Vec<_>>(),
    });
    if include_links {
        // A cycle reference is a pointer to a node shown elsewhere and has no
        // links of its own; keep the array shape stable for every node.
        value["links"] = json!(node.links.clone().unwrap_or_default());
    }
    value
}

/// Renders the human tree with ASCII connectors (no platform-specific glyphs).
fn human(nodes: &[NodeData]) -> String {
    let mut out = String::new();
    let root = &nodes[0];
    if let Some(parent) = &root.parent {
        let _ = writeln!(out, "parent: {} {}", parent.key, parent.title);
    }
    let _ = writeln!(out, "{} {}", root.key, root.summary_line());
    render_annotations(&mut out, "", root);
    let last = root.children.len().saturating_sub(1);
    for (position, &child) in root.children.iter().enumerate() {
        render_child(&mut out, nodes, child, "", position == last);
    }
    out
}

/// Renders a non-root node and descendants with box-drawing connectors.
fn render_child(out: &mut String, nodes: &[NodeData], index: usize, prefix: &str, is_last: bool) {
    let node = &nodes[index];
    let connector = if is_last { "`- " } else { "|- " };
    let _ = writeln!(
        out,
        "{prefix}{connector}{} {}",
        node.key,
        node.summary_line()
    );
    let child_prefix = format!("{prefix}{}", if is_last { "   " } else { "|  " });
    render_annotations(out, &child_prefix, node);
    let last = node.children.len().saturating_sub(1);
    for (position, &child) in node.children.iter().enumerate() {
        render_child(out, nodes, child, &child_prefix, position == last);
    }
}

/// Writes a node's link and truncation annotations.
fn render_annotations(out: &mut String, prefix: &str, node: &NodeData) {
    if let Some(links) = &node.links {
        for link in links {
            let other = &link.other_work_item;
            let _ = writeln!(
                out,
                "{prefix}   links: {} -> {} {} [{}, {}]",
                link.relation, other.key, other.title, other.item_type, other.status
            );
        }
    }
    for marker in &node.markers {
        let _ = writeln!(out, "{prefix}   [{}]", marker.message());
    }
}
