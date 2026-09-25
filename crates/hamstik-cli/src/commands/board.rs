// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `board view` — a read-only, kanban-style board composed from existing reads.
//!
//! The Public API v1 exposes no board resource, so the CLI composes one from
//! the already-shipped Work Item list read: `GET
//! .../projects/{key}/work-items` (optionally filtered by `sprint`). The
//! command is strictly read-only — workflow changes continue through
//! `work transition`, and there is no `board move`.
//!
//! Columns are derived from the statuses the server reports on the returned
//! Work Items; the CLI keeps no local status catalog, so a new server status
//! becomes a column automatically. Items whose sparse projection omits
//! `status` (or whose status is empty) render in an explicit "no status"
//! bucket instead of being dropped. Column order uses the documented canonical
//! status order as presentation only; status values themselves are the
//! server's verbatim strings.
//!
//! The human layout is width-aware: it honours `$COLUMNS`, otherwise uses a
//! terminal-width default, sizes columns to their content when it fits, and
//! wraps card text when the board would otherwise overflow.

use serde_json::{Value, json};

use hamstik_api_client::{ListWorkItemsQuery, PageItems, follow_with};

use crate::app::Session;
use crate::args::{BoardArgs, BoardCommand, BoardViewArgs};
use crate::error::CliError;
use crate::output::{sanitize_for_terminal, visible_width};
use crate::terminal::{Glyphs, TerminalProfile};

use super::{emit_json, follow_policy, validate_work_item_fields};

/// Envelope schema version for the stable `--json` board document.
const BOARD_VERSION: u64 = 1;

/// Presentation order for the documented canonical statuses. Unknown
/// server-reported statuses follow, in first-appearance order; the
/// missing-status bucket is always last.
const CANONICAL_STATUS_ORDER: &[&str] = &["backlog", "todo", "in_progress", "in_review", "done"];

/// Assumed terminal width when `$COLUMNS` is unset and stdout is a terminal.
const DEFAULT_WIDTH: usize = 100;
/// Width used when stdout is not a terminal: natural column widths, so piped
/// output is not wrapped by an invented terminal size.
const PIPED_WIDTH: usize = 400;
/// Bounds for an explicit `$COLUMNS` value.
const MIN_WIDTH: usize = 32;
const MAX_WIDTH: usize = 400;
/// Narrowest a column may be squeezed to before horizontal overflow wins.
const MIN_COLUMN_WIDTH: usize = 12;
/// Spaces between side-by-side columns.
const COLUMN_GAP: usize = 2;

/// Runs the `board` subcommands.
pub(crate) async fn run(session: &mut Session<'_>, args: &BoardArgs) -> Result<(), CliError> {
    match &args.command {
        BoardCommand::View(view) => view_board(session, view).await,
    }
}

/// One status column: the server-reported status (`None` for the explicit
/// missing-status bucket) and the raw Work Item summaries it holds.
struct Column {
    status: Option<String>,
    items: Vec<Value>,
}

/// Runs `board view`.
async fn view_board(session: &mut Session<'_>, args: &BoardViewArgs) -> Result<(), CliError> {
    let selection = session.selection()?;
    let org = session.require_org(&selection)?;
    let project = super::sprint::require_project(session, args.project.as_deref())?;
    // `--fields` is the server-side sparse fieldset on Work Item list reads,
    // so unknown names fail locally as a usage error, exactly like `work list`.
    validate_work_item_fields(&session.global.fields)?;
    let api = session.api(&selection)?;

    // The only network call is the existing Work Item list read. `--fields`
    // is forwarded as the server's sparse fieldset, exactly like `work list`.
    let query = ListWorkItemsQuery {
        limit: args.pagination.page_size(),
        cursor: args.pagination.cursor.clone(),
        sprint: args.sprint.clone(),
        fields: session.global.fields.clone(),
        ..Default::default()
    };

    let (items, truncated) = if args.pagination.all {
        let fetch_api = api.clone();
        let org = org.clone();
        let project = project.clone();
        let base = query.clone();
        let page = follow_with(follow_policy(&args.pagination), move |cursor| {
            let fetch_api = fetch_api.clone();
            let org = org.clone();
            let project = project.clone();
            let mut query = base.clone();
            query.cursor = cursor;
            async move {
                let response = fetch_api.list_work_items(&org, &project, query).await?;
                Ok(PageItems::new(
                    response.value.items,
                    &response.raw,
                    response.value.page,
                ))
            }
        })
        .await
        .map_err(CliError::from_client)?;
        (page.raw_items, page.page.has_more)
    } else {
        let response = api
            .list_work_items(&org, &project, query)
            .await
            .map_err(CliError::from_client)?;
        let items = response
            .raw
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        (items, response.value.page.has_more)
    };

    let columns = columns_from_items(&items);
    let items_fetched = items.len();

    if session.json() {
        let document = board_json(
            &project,
            args.sprint.as_deref(),
            &columns,
            items_fetched,
            truncated,
        );
        emit_json(session, &document)
    } else {
        let width = board_width(session);
        render_board(session, &project, args.sprint.as_deref(), &columns, width)?;
        if truncated {
            session.out.warn(
                "note: results truncated: the server reported more pages; rerun with --all (optionally --limit) to fill every column",
            );
        }
        Ok(())
    }
}

/// The stable `--json` board document.
fn board_json(
    project: &str,
    sprint: Option<&str>,
    columns: &[Column],
    items_fetched: usize,
    truncated: bool,
) -> Value {
    let columns: Vec<Value> = columns
        .iter()
        .map(|column| {
            json!({
                "status": column.status,
                "count": column.items.len(),
                "storyPoints": story_points(&column.items),
                "items": column.items,
            })
        })
        .collect();
    json!({
        "boardVersion": BOARD_VERSION,
        "scope": {
            "kind": if sprint.is_some() { "sprint" } else { "project" },
            "project": project,
            "sprint": sprint,
        },
        "columns": columns,
        "itemsFetched": items_fetched,
        "truncated": truncated,
    })
}

/// Buckets Work Item summaries into columns by their server-reported status.
fn columns_from_items(items: &[Value]) -> Vec<Column> {
    let mut known: Vec<(String, Vec<Value>)> = Vec::new();
    let mut missing: Vec<Value> = Vec::new();
    for item in items {
        match status_of(item) {
            Some(status) => {
                if let Some(entry) = known.iter_mut().find(|(candidate, _)| candidate == &status) {
                    entry.1.push(item.clone());
                } else {
                    known.push((status, vec![item.clone()]));
                }
            }
            None => missing.push(item.clone()),
        }
    }
    // Stable sort: canonical statuses first in their documented order, then
    // any unrecognized server statuses in first-appearance order.
    known.sort_by_key(|(status, _)| canonical_rank(status));

    let mut columns: Vec<Column> = known
        .into_iter()
        .map(|(status, items)| Column {
            status: Some(status),
            items,
        })
        .collect();
    if !missing.is_empty() {
        columns.push(Column {
            status: None,
            items: missing,
        });
    }
    columns
}

/// The status to bucket an item under, treating absent/blank as missing.
fn status_of(item: &Value) -> Option<String> {
    item.get("status")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|status| !status.is_empty())
        .map(str::to_string)
}

/// Rank a status by its documented presentation order.
fn canonical_rank(status: &str) -> usize {
    CANONICAL_STATUS_ORDER
        .iter()
        .position(|candidate| *candidate == status)
        .unwrap_or(usize::MAX)
}

/// Sum of numeric story points across a column's items.
fn story_points(items: &[Value]) -> i64 {
    items
        .iter()
        .filter_map(|item| item.get("storyPoints").and_then(Value::as_i64))
        .sum()
}

/// The board's available display width.
///
/// `HAMSTIK_TERM=ascii` pins the natural (piped) width so captured logs do not
/// depend on `$COLUMNS` or on whether stdout is a terminal.
fn board_width(session: &Session<'_>) -> usize {
    if session.terminal_profile() == TerminalProfile::Ascii {
        return PIPED_WIDTH;
    }
    if let Some(columns) = session
        .env
        .var("COLUMNS")
        .and_then(|value| value.trim().parse::<usize>().ok())
    {
        return columns.clamp(MIN_WIDTH, MAX_WIDTH);
    }
    if session.env.stdout_is_terminal() {
        DEFAULT_WIDTH
    } else {
        PIPED_WIDTH
    }
}

/// Renders the human (or quiet) board.
fn render_board(
    session: &mut Session<'_>,
    project: &str,
    sprint: Option<&str>,
    columns: &[Column],
    width: usize,
) -> Result<(), CliError> {
    if session.out.is_quiet() {
        for column in columns {
            for item in &column.items {
                session.out.line(&key_of(item)).map_err(CliError::general)?;
            }
        }
        return Ok(());
    }

    let glyphs = session.glyphs();
    let scope = match sprint {
        Some(sprint) => format!(
            "Board {} Sprint {sprint} {} Project {project}",
            glyphs.em_dash(),
            glyphs.middle_dot()
        ),
        None => format!("Board {} Project {project}", glyphs.em_dash()),
    };
    session.out.line(&scope).map_err(CliError::general)?;

    if columns.is_empty() {
        session
            .out
            .line("(no Work Items in scope)")
            .map_err(CliError::general)?;
        return Ok(());
    }

    // Size columns to their content when the board fits, otherwise share the
    // available width so the layout stays within the terminal.
    let widths: Vec<usize> = column_widths(columns, width, glyphs);
    let rendered: Vec<Vec<String>> = columns
        .iter()
        .zip(&widths)
        .map(|(column, column_width)| column_lines(column, *column_width, glyphs))
        .collect();
    let height = rendered.iter().map(Vec::len).max().unwrap_or(0);
    for row in 0..height {
        let mut line = String::new();
        for (index, lines) in rendered.iter().enumerate() {
            if index > 0 {
                line.push_str(&" ".repeat(COLUMN_GAP));
            }
            let cell = lines.get(row).map_or("", String::as_str);
            line.push_str(cell);
            if index + 1 < rendered.len() {
                let pad = widths[index].saturating_sub(visible_width(cell));
                line.push_str(&" ".repeat(pad));
            }
        }
        session
            .out
            .line(line.trim_end())
            .map_err(CliError::general)?;
    }
    Ok(())
}

/// Content-derived or width-clamped column widths.
fn column_widths(columns: &[Column], width: usize, glyphs: Glyphs) -> Vec<usize> {
    let natural: Vec<usize> = columns
        .iter()
        .map(|column| natural_width(column, glyphs))
        .collect();
    let gaps = COLUMN_GAP * columns.len().saturating_sub(1);
    let budget = width.saturating_sub(gaps);
    if natural.iter().sum::<usize>() <= budget {
        return natural;
    }
    let per_column = (budget / columns.len()).max(MIN_COLUMN_WIDTH);
    vec![per_column; columns.len()]
}

/// The width a column would need to render unwrapped.
fn natural_width(column: &Column, glyphs: Glyphs) -> usize {
    let mut width =
        visible_width(&sanitize_for_terminal(&column_header(column, glyphs))).max(MIN_COLUMN_WIDTH);
    for item in &column.items {
        width = width.max(visible_width(&key_of(item)));
        width = width.max(visible_width(&sanitize_for_terminal(&title_of(item))));
        width = width.max(visible_width(&sanitize_for_terminal(&card_meta(
            item, glyphs,
        ))));
    }
    width
}

/// The header label and item/point totals for a column.
fn column_header(column: &Column, glyphs: Glyphs) -> String {
    let label = status_label(column.status.as_deref());
    let points = story_points(&column.items);
    if points > 0 {
        format!(
            "{label} ({}) {} {points}pt",
            column.items.len(),
            glyphs.middle_dot()
        )
    } else {
        format!("{label} ({})", column.items.len())
    }
}

/// Human label for a status, or the explicit missing-status bucket.
fn status_label(status: Option<&str>) -> String {
    status.map_or_else(|| "(no status)".to_string(), ToString::to_string)
}

/// The rendered lines of one column: header, rule, then cards separated by
/// blank lines, each wrapped to the column width.
fn column_lines(column: &Column, width: usize, glyphs: Glyphs) -> Vec<String> {
    let mut lines = wrap_text(&column_header(column, glyphs), width);
    lines.push("-".repeat(width));
    for item in &column.items {
        lines.push(String::new());
        lines.extend(wrap_text(&key_of(item), width));
        lines.extend(wrap_text(&title_of(item), width));
        let meta = card_meta(item, glyphs);
        if !meta.is_empty() {
            lines.extend(wrap_text(&meta, width));
        }
    }
    lines
}

/// A Work Item key, or `-` when the sparse projection omitted it.
fn key_of(item: &Value) -> String {
    item.get("key")
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_string()
}

/// A Work Item title, or an empty string when omitted.
fn title_of(item: &Value) -> String {
    item.get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A compact card meta line (`priority · @assignee · Npt`), omitting absent
/// fields.
fn card_meta(item: &Value, glyphs: Glyphs) -> String {
    let mut parts = Vec::new();
    if let Some(priority) = item
        .get("priority")
        .and_then(Value::as_str)
        .filter(|priority| !priority.is_empty())
    {
        parts.push(priority.to_string());
    }
    if let Some(name) = item
        .get("assignee")
        .and_then(|assignee| assignee.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
    {
        parts.push(format!("@{name}"));
    }
    if let Some(points) = item.get("storyPoints").and_then(Value::as_i64) {
        parts.push(format!("{points}pt"));
    }
    parts.join(&format!(" {} ", glyphs.middle_dot()))
}

/// Word-wraps sanitized text to `width` columns, hard-breaking over-long
/// words and never returning an empty line list.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let sanitized = sanitize_for_terminal(text);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;

    for word in sanitized.split_whitespace() {
        let mut word = word.to_string();
        while word.chars().count() > width {
            let head: String = word.chars().take(width).collect();
            word = word.chars().skip(width).collect();
            if current_len > 0 {
                lines.push(std::mem::take(&mut current));
                current_len = 0;
            }
            lines.push(head);
        }
        let word_len = word.chars().count();
        if current_len == 0 {
            current = word;
            current_len = word_len;
        } else if current_len + 1 + word_len <= width {
            current.push(' ');
            current.push_str(&word);
            current_len += 1 + word_len;
        } else {
            lines.push(std::mem::take(&mut current));
            current = word;
            current_len = word_len;
        }
    }
    if current_len > 0 {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn item(key: &str, status: Option<&str>) -> Value {
        json!({
            "key": key,
            "title": format!("Item {key}"),
            "status": status,
            "priority": "medium",
            "assignee": {"id": "u1", "name": "Alice"},
            "storyPoints": 3,
        })
    }

    #[test]
    fn columns_come_from_reported_statuses_and_bucket_missing_ones() {
        let items = vec![
            item("HAM-1", Some("done")),
            item("HAM-2", Some("todo")),
            item("HAM-3", None),
            item("HAM-4", Some("")),
            item("HAM-5", Some("todo")),
        ];
        let columns = columns_from_items(&items);

        let statuses: Vec<Option<&str>> = columns
            .iter()
            .map(|column| column.status.as_deref())
            .collect();
        assert_eq!(
            statuses,
            vec![Some("todo"), Some("done"), None],
            "canonical order, then the explicit missing bucket"
        );
        assert_eq!(columns[0].items.len(), 2);
        assert_eq!(columns[2].items.len(), 2, "null and blank are both missing");
    }

    #[test]
    fn unknown_statuses_are_kept_in_first_appearance_order() {
        let items = vec![
            item("HAM-1", Some("blocked")),
            item("HAM-2", Some("todo")),
            item("HAM-3", Some("triage")),
        ];
        let columns = columns_from_items(&items);
        let statuses: Vec<Option<&str>> = columns
            .iter()
            .map(|column| column.status.as_deref())
            .collect();
        assert_eq!(
            statuses,
            vec![Some("todo"), Some("blocked"), Some("triage")]
        );
    }

    #[test]
    fn wrapping_respects_width_and_hard_breaks_long_words() {
        assert_eq!(wrap_text("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap_text("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(wrap_text("", 10), vec![""]);
    }

    #[test]
    fn json_document_has_the_stable_shape() {
        let columns = columns_from_items(&[item("HAM-1", Some("todo")), item("HAM-2", None)]);
        let document = board_json("HAM", Some("sprint-1"), &columns, 2, false);
        assert_eq!(document["boardVersion"], 1);
        assert_eq!(document["scope"]["kind"], "sprint");
        assert_eq!(document["scope"]["project"], "HAM");
        assert_eq!(document["scope"]["sprint"], "sprint-1");
        assert_eq!(document["columns"][0]["status"], "todo");
        assert_eq!(document["columns"][0]["count"], 1);
        assert_eq!(document["columns"][0]["storyPoints"], 3);
        assert_eq!(document["columns"][1]["status"], Value::Null);
        assert_eq!(document["itemsFetched"], 2);
        assert_eq!(document["truncated"], false);
    }
}
