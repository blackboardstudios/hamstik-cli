// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Shared human rendering for the server report commands
//! (`hamstik project report`, `hamstik sprint report`).
//!
//! Every value printed here comes straight from the server payload: the CLI
//! never derives, completes, or re-aggregates a report metric (client-side
//! aggregation is a separate, later capability). `--json` bypasses this module
//! entirely and echoes the server body verbatim, so the human view below is a
//! presentation of the same document, never a second computation of it.

use hamstik_api_client::{Page, ProjectReport, SprintReport};
use serde_json::Value;

use crate::app::Session;
use crate::error::CliError;

use super::org::render_lines;

/// Width of the burndown bar chart.
const BURNDOWN_WIDTH: usize = 32;

/// Renders a project report: identity header, the report rows as a table, and
/// any server-reported limitations.
pub(crate) fn render_project_report(
    session: &mut Session<'_>,
    project: &str,
    report: &ProjectReport,
) -> Result<(), CliError> {
    render_lines(
        session,
        &[
            ("project", project.to_string()),
            ("report", report.kind.clone()),
            ("data rows", report.data.len().to_string()),
            ("item rows", report.items.len().to_string()),
        ],
    )?;

    // Report kinds differ: some put their series in `data`, others in the
    // paginated `items` collection. Render whichever carries rows, preferring
    // `data`, and say which one is on screen.
    let (label, rows) = if report.data.is_empty() {
        ("items", &report.items)
    } else {
        ("data", &report.data)
    };
    if rows.is_empty() {
        session
            .out
            .line("\n(no rows in this page)")
            .map_err(CliError::general)?;
    } else {
        session
            .out
            .line(&format!("\n{label}:"))
            .map_err(CliError::general)?;
        let (headers, cells) = value_table(rows);
        session
            .out
            .table(
                &headers.iter().map(String::as_str).collect::<Vec<_>>(),
                &cells,
            )
            .map_err(CliError::general)?;
    }

    render_page(session, &report.page)?;
    render_limitations(session, &report.limitations)
}

/// Renders a Sprint delivery report: header, completion, scope movement,
/// status distribution, a simple burndown, and the change feed page.
pub(crate) fn render_sprint_report(
    session: &mut Session<'_>,
    report: &SprintReport,
) -> Result<(), CliError> {
    let sprint = &report.sprint;
    let completion = &report.completion;
    let final_percent = &completion.percentages.final_scope;

    let mut lines: Vec<(&str, String)> = vec![
        ("sprint", format!("{} ({})", sprint.name, sprint.id)),
        ("state", format!("{} ({})", sprint.state, report.mode)),
        (
            "window",
            format!(
                "{} → {}",
                sprint.start_date.as_deref().unwrap_or("-"),
                sprint.end_date.as_deref().unwrap_or("-")
            ),
        ),
        (
            "goal",
            sprint.goal.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "target points",
            sprint
                .target_points
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("stable", yes_no(report.stable)),
        (
            "committed",
            if report.commitment.available {
                format!(
                    "{} items / {} points",
                    report.commitment.item_count, report.commitment.points
                )
            } else {
                "not available".to_string()
            },
        ),
        (
            "final scope",
            format!(
                "{} items / {} points",
                completion.final_scope.item_count, completion.final_scope.points
            ),
        ),
        (
            "completed",
            format!(
                "{} items / {} points ({} items, {} points of final scope)",
                completion.completed.item_count,
                completion.completed.points,
                percent(final_percent.item_percent),
                percent(final_percent.point_percent),
            ),
        ),
        (
            "remaining",
            format!(
                "{} items / {} points",
                report.remaining.item_count, report.remaining.points
            ),
        ),
        (
            "scope changes",
            format!(
                "{} added, {} removed",
                report.scope_changes.added.len(),
                report.scope_changes.removed.len()
            ),
        ),
        ("carryover", describe_carryover(report)),
    ];
    if let Some(through) = sprint.report_end_at.clone() {
        lines.push(("reported through", through));
    }
    render_lines(session, &lines)?;

    if !report.statuses.is_empty() {
        session.out.line("\nstatuses:").map_err(CliError::general)?;
        let rows: Vec<Vec<String>> = report
            .statuses
            .iter()
            .map(|status| {
                vec![
                    status.status.clone(),
                    status.item_count.to_string(),
                    status.points.to_string(),
                ]
            })
            .collect();
        session
            .out
            .table(&["STATUS", "ITEMS", "POINTS"], &rows)
            .map_err(CliError::general)?;
    }

    render_burndown(session, report)?;

    if report.items.is_empty() {
        session
            .out
            .line("\nchange feed: (no entries in this page)")
            .map_err(CliError::general)?;
    } else {
        session
            .out
            .line("\nchange feed:")
            .map_err(CliError::general)?;
        let rows: Vec<Vec<String>> = report
            .items
            .iter()
            .map(|entry| {
                vec![
                    entry.item.occurred_at.clone(),
                    entry.kind.clone(),
                    entry.item.key.clone(),
                    entry
                        .change_type
                        .clone()
                        .or_else(|| entry.destination.as_ref().map(|d| d.name.clone()))
                        .unwrap_or_else(|| "-".to_string()),
                    entry.item.status.clone(),
                    entry
                        .item
                        .story_points
                        .map(|p| p.to_string())
                        .unwrap_or_else(|| "-".to_string()),
                ]
            })
            .collect();
        session
            .out
            .table(
                &["WHEN", "KIND", "KEY", "CHANGE", "STATUS", "POINTS"],
                &rows,
            )
            .map_err(CliError::general)?;
    }

    render_page(session, &report.page)?;
    render_limitations(session, &report.limitations)
}

/// Renders the burndown as a scaled bar per sample, marking the ideal line.
///
/// The bar length is proportional to the remaining points the server reported
/// (scaled to the largest sample in the series); `|` marks the ideal value for
/// the same date. Nothing is interpolated: dates without a sample are absent.
fn render_burndown(session: &mut Session<'_>, report: &SprintReport) -> Result<(), CliError> {
    if !report.burndown.available {
        session
            .out
            .line("\nburndown: not available for this sprint")
            .map_err(CliError::general)?;
        return Ok(());
    }

    let samples: Vec<(String, i64, i64)> = if report.burndown.display.is_empty() {
        report
            .burndown
            .points
            .iter()
            .map(|p| (p.date.clone(), p.remaining_points, p.remaining_points))
            .collect()
    } else {
        report
            .burndown
            .display
            .iter()
            .map(|d| (d.label.clone(), d.remaining, d.ideal))
            .collect()
    };
    if samples.is_empty() {
        session
            .out
            .line("\nburndown: (no samples)")
            .map_err(CliError::general)?;
        return Ok(());
    }

    let scale_max = samples
        .iter()
        .flat_map(|(_, remaining, ideal)| [*remaining, *ideal])
        .max()
        .unwrap_or(0)
        .max(1);

    session
        .out
        .line("\nburndown (remaining points, | = ideal):")
        .map_err(CliError::general)?;
    for (label, remaining, ideal) in samples {
        let bar = burndown_bar(remaining, ideal, scale_max);
        session
            .out
            .line(&format!("{label:<12} {remaining:>5}  {bar}  ideal {ideal}"))
            .map_err(CliError::general)?;
    }
    Ok(())
}

/// Builds one burndown bar: `#` per remaining point, `|` at the ideal mark.
fn burndown_bar(remaining: i64, ideal: i64, scale_max: i64) -> String {
    let denominator = scale_max.max(1) as f64;
    let scaled = move |value: i64| -> usize {
        let value = value.clamp(0, scale_max);
        ((value as f64 / denominator) * BURNDOWN_WIDTH as f64).round() as usize
    };
    let mut bar = vec!['.'; BURNDOWN_WIDTH];
    for cell in bar.iter_mut().take(scaled(remaining).min(BURNDOWN_WIDTH)) {
        *cell = '#';
    }
    let ideal_at = scaled(ideal).min(BURNDOWN_WIDTH);
    if ideal_at > 0 && ideal_at <= BURNDOWN_WIDTH {
        bar[ideal_at - 1] = '|';
    }
    bar.into_iter().collect()
}

/// Renders server-reported limitations verbatim.
fn render_limitations(session: &mut Session<'_>, limitations: &[String]) -> Result<(), CliError> {
    if limitations.is_empty() {
        return Ok(());
    }
    session
        .out
        .line("\nlimitations:")
        .map_err(CliError::general)?;
    for limitation in limitations {
        session
            .out
            .line(&format!("  - {limitation}"))
            .map_err(CliError::general)?;
    }
    Ok(())
}

/// Renders the page trailer for a report collection.
///
/// The cursor itself is only shown through `--json`; a human continuation uses
/// the documented `--since-cursor` flag with the value from the previous page.
fn render_page(session: &mut Session<'_>, page: &Page) -> Result<(), CliError> {
    let more = if page.has_more {
        "yes (continue with --since-cursor <page.nextCursor from --json>)"
    } else {
        "no"
    };
    session
        .out
        .line(&format!(
            "\npage: limit {}, more pages: {}",
            page.limit, more
        ))
        .map_err(CliError::general)
}

/// Turns an array of server objects into a deterministic header/row table.
///
/// Columns are the keys of the rows in first-seen order; a value the row does
/// not carry prints as `-`, and nested values print as compact JSON. No value
/// is transformed beyond that.
fn value_table(rows: &[Value]) -> (Vec<String>, Vec<Vec<String>>) {
    let mut keys: Vec<String> = Vec::new();
    for row in rows {
        match row.as_object() {
            Some(object) => {
                for key in object.keys() {
                    if !keys.contains(key) {
                        keys.push(key.clone());
                    }
                }
            }
            None => {
                let fallback = "value".to_string();
                if !keys.contains(&fallback) {
                    keys.push(fallback);
                }
            }
        }
    }
    if keys.is_empty() {
        keys.push("value".to_string());
    }

    let headers = keys.iter().map(|key| key.to_uppercase()).collect();
    let cells = rows
        .iter()
        .map(|row| {
            keys.iter()
                .map(|key| match row.get(key) {
                    Some(value) => render_cell(value),
                    None => "-".to_string(),
                })
                .collect()
        })
        .collect();
    (headers, cells)
}

/// Renders one JSON value as a table cell.
fn render_cell(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => yes_no(*flag),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

fn yes_no(flag: bool) -> String {
    if flag { "yes" } else { "no" }.to_string()
}

/// Formats an optional percentage, keeping "not computable" explicit.
fn percent(value: Option<i64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |v| format!("{v}%"))
}

/// Summarizes where carried-over work went, grouped by destination name.
fn describe_carryover(report: &SprintReport) -> String {
    if report.carryover.is_empty() {
        return "none".to_string();
    }
    let mut destinations: Vec<(String, usize)> = Vec::new();
    for entry in &report.carryover {
        let name = entry.destination.name.clone();
        if let Some((_, count)) = destinations.iter_mut().find(|(known, _)| *known == name) {
            *count += 1;
        } else {
            destinations.push((name, 1));
        }
    }
    let parts: Vec<String> = destinations
        .iter()
        .map(|(name, count)| format!("{count} → {name}"))
        .collect();
    format!("{} ({})", report.carryover.len(), parts.join(", "))
}
