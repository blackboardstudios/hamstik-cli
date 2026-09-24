// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work bulk from-csv`: convert a local CSV file into the exact `work bulk`
//! operations JSON array.
//!
//! The converter is purely local: it never opens a connection. It emits the
//! same operations array `work bulk create|update --operations-file` accepts,
//! and it runs that array through the existing [`bulk_preflight`] so the CSV
//! path can never accept an envelope the JSON path would reject. Diagnostics
//! name the CSV row number and column header, mirroring the JSON preflight's
//! operation-index and field-path reporting.
//!
//! The dialect is RFC 4180: comma-separated fields, `"` quoting, `""` for an
//! embedded quote, and CRLF or LF record separators. The first record is a
//! required header. A quoted field may span records; embedded CRLF is
//! normalized to LF. Free-text values (title, description, work item key) are
//! not trimmed. Identifier cells (type, priority, assignee, sprint, parent,
//! revision, story points, due date) are trimmed while being interpreted, and
//! the trimmed identifier is what lands in the operation. An empty cell omits
//! that field.
//!
//! Only columns the frozen Public API v1 bulk envelopes actually carry are
//! accepted: `title` for `--op create`; `workItemKey`, `revision`, `title`,
//! `description`, `type`, `priority`, `assignee`, `sprint`, `parent`,
//! `storyPoints`, and `dueDate` for `--op update`. Status and labels are not
//! part of either bulk envelope (status is a transition, labels use the label
//! endpoints), so they are rejected as unknown columns rather than silently
//! ignored.

use std::path::Path;
use std::str::FromStr;

use hamstik_api_client::{
    BulkCreateWorkItemOperation, BulkUpdateWorkItemOperation, UpdateWorkItemRequest,
};
use serde_json::{Value, json};

use crate::app::Session;
use crate::args::{BulkOpArg, PriorityArg, TypeArg};
use crate::error::CliError;
use crate::fsutil;
use crate::input::{MAX_TEXT_BYTES, read_capped};
use crate::time_arg::TimeArg;

use super::bulk_preflight::{self, PreflightKind};

/// Converts the CSV at `file` and writes the operations array to `output`
/// (or stdout when `output` is `None` or `-`).
///
/// `project` is the per-command override; when absent, the global `--project`
/// and then normal local project selection (`HAMSTIK_PROJECT`, the context
/// file, the profile default) supply the key. `selection()` performs no
/// network or credential access, so the conversion stays purely local.
pub(super) fn from_csv(
    session: &mut Session<'_>,
    file: &str,
    op: BulkOpArg,
    project: Option<&str>,
    output: Option<&str>,
) -> Result<(), CliError> {
    let project = match non_empty(project).or_else(|| non_empty(session.global.project.as_deref()))
    {
        Some(project) => project,
        None => session
            .selection()?
            .project
            .value
            .ok_or_else(|| {
                CliError::usage(
                    "no project selected; pass --project, set HAMSTIK_PROJECT, or run `hamstik project use <key>`",
                )
            })?,
    };
    let project = project.as_str();
    let text = read_source(file)?;
    let source = if file == "-" { "stdin" } else { file };
    let kind = kind_for(op);

    // Build the typed operations, then re-run the exact same preflight the
    // JSON `--operations-file` path uses. The preflight result is what we emit,
    // so the two paths are byte-equivalent in structure and shared rules
    // (including the 1–50 operation bound).
    let operations = convert(&text, source, op, project)?;
    let compact = serde_json::to_string(&operations).map_err(CliError::general)?;
    let validated = bulk_preflight::preflight(&compact, source, kind)?;
    verify_typed(op, &validated, source)?;

    let count = validated.len();
    let mut rendered =
        serde_json::to_string_pretty(&Value::Array(validated)).map_err(CliError::general)?;
    rendered.push('\n');

    match output.filter(|path| *path != "-") {
        Some(path) => {
            fsutil::write_atomic(Path::new(path), rendered.as_bytes())
                .map_err(|err| CliError::general(format!("cannot write {path}: {err}")))?;
            if session.json() {
                return super::emit_json(session, &json!({ "path": path, "operations": count }));
            }
            if !session.out.is_quiet() {
                session
                    .out
                    .human(&format!("wrote {count} operations to {path}"))
                    .map_err(CliError::general)?;
            }
            Ok(())
        }
        None => session
            .out
            .raw(&rendered)
            .map_err(|err| CliError::general(format!("cannot write stdout: {err}"))),
    }
}

fn kind_for(op: BulkOpArg) -> PreflightKind {
    match op {
        BulkOpArg::Create => PreflightKind::Create,
        BulkOpArg::Update => PreflightKind::Update,
    }
}

/// Normalizes an optional project key, treating blank as absent.
fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Reads the CSV from a path or stdin, capped like every other text input.
fn read_source(file: &str) -> Result<String, CliError> {
    if file == "-" {
        let stdin = std::io::stdin();
        read_capped(stdin.lock(), MAX_TEXT_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read stdin: {err}")))
    } else {
        let handle = std::fs::File::open(file)
            .map_err(|err| CliError::usage(format!("cannot read {file}: {err}")))?;
        read_capped(handle, MAX_TEXT_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read {file}: {err}")))
    }
}

/// Converts the CSV text into a typed operations JSON array.
fn convert(text: &str, source: &str, op: BulkOpArg, project: &str) -> Result<Vec<Value>, CliError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let records = parse_csv(text).map_err(|err| parse_error(source, err))?;
    let Some((header, rows)) = records.split_first() else {
        return Err(CliError::usage(format!(
            "{source} is empty; the first row must be a CSV header"
        )));
    };
    let columns = resolve_columns(&header.fields, op, source)?;

    let mut findings: Vec<String> = Vec::new();
    let mut operations: Vec<Value> = Vec::new();
    for record in rows {
        if record.fields.len() != columns.len() {
            findings.push(finding(
                record.line,
                None,
                format!(
                    "expected {} columns to match the header, found {}",
                    columns.len(),
                    record.fields.len()
                ),
            ));
            continue;
        }
        let cells: Vec<&str> = record.fields.iter().map(String::as_str).collect();
        match op {
            BulkOpArg::Create => {
                convert_create(
                    record.line,
                    &columns,
                    &cells,
                    project,
                    &mut operations,
                    &mut findings,
                );
            }
            BulkOpArg::Update => {
                convert_update(
                    record.line,
                    &columns,
                    &cells,
                    project,
                    &mut operations,
                    &mut findings,
                );
            }
        }
    }

    if !findings.is_empty() {
        return Err(aggregate(source, &findings));
    }
    if operations.is_empty() {
        return Err(CliError::usage(format!(
            "{source} contains no data rows; add at least one operation below the header"
        )));
    }
    if operations.len() > bulk_preflight::MAX_BULK_OPERATIONS {
        return Err(CliError::usage(format!(
            "{source} contains {} operations; bulk requests accept at most {}",
            operations.len(),
            bulk_preflight::MAX_BULK_OPERATIONS
        )));
    }
    Ok(operations)
}

/// Confirms the emitted array still deserializes into the typed public client
/// envelopes. This is a belt-and-braces guard: if the converter ever drifts
/// from the schema, the failure is local and explicit rather than a server 400.
fn verify_typed(op: BulkOpArg, operations: &[Value], source: &str) -> Result<(), CliError> {
    let value = Value::Array(operations.to_vec());
    let result: Result<(), serde_json::Error> = match op {
        BulkOpArg::Create => {
            serde_json::from_value::<Vec<BulkCreateWorkItemOperation>>(value).map(|_| ())
        }
        BulkOpArg::Update => {
            serde_json::from_value::<Vec<BulkUpdateWorkItemOperation>>(value).map(|_| ())
        }
    };
    result.map_err(|err| {
        CliError::usage(format!(
            "{source} produced operations that do not match the Public API bulk schema: {err}"
        ))
    })
}

/// The CSV columns the two bulk envelopes can express.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Title,
    Description,
    ItemType,
    Priority,
    Assignee,
    Sprint,
    Parent,
    Revision,
    WorkItemKey,
    StoryPoints,
    DueDate,
}

impl Column {
    /// The canonical header/message spelling.
    fn name(self) -> &'static str {
        match self {
            Column::Title => "title",
            Column::Description => "description",
            Column::ItemType => "type",
            Column::Priority => "priority",
            Column::Assignee => "assignee",
            Column::Sprint => "sprint",
            Column::Parent => "parent",
            Column::Revision => "revision",
            Column::WorkItemKey => "workItemKey",
            Column::StoryPoints => "storyPoints",
            Column::DueDate => "dueDate",
        }
    }
}

/// The header names accepted for `--op update`, in documented order.
const UPDATE_COLUMNS: &[Column] = &[
    Column::WorkItemKey,
    Column::Revision,
    Column::Title,
    Column::Description,
    Column::ItemType,
    Column::Priority,
    Column::Assignee,
    Column::Sprint,
    Column::Parent,
    Column::StoryPoints,
    Column::DueDate,
];

/// The header names accepted for `--op create`.
const CREATE_COLUMNS: &[Column] = &[Column::Title];

/// Maps a header cell to a column, accepting camelCase and snake_case spellings
/// case-insensitively. Unknown spellings are rejected.
fn parse_column(raw: &str) -> Option<Column> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "title" => Some(Column::Title),
        "description" => Some(Column::Description),
        "type" => Some(Column::ItemType),
        "priority" => Some(Column::Priority),
        "assignee" => Some(Column::Assignee),
        "sprint" | "sprintid" | "sprint_id" => Some(Column::Sprint),
        "parent" | "parentid" | "parent_id" => Some(Column::Parent),
        "revision" => Some(Column::Revision),
        "workitemkey" | "work_item_key" => Some(Column::WorkItemKey),
        "storypoints" | "story_points" => Some(Column::StoryPoints),
        "duedate" | "due_date" => Some(Column::DueDate),
        _ => None,
    }
}

/// Resolves and validates the header row against the selected operation.
fn resolve_columns(
    header: &[String],
    op: BulkOpArg,
    source: &str,
) -> Result<Vec<Column>, CliError> {
    let allowed: &[Column] = match op {
        BulkOpArg::Create => CREATE_COLUMNS,
        BulkOpArg::Update => UPDATE_COLUMNS,
    };
    let op_name = match op {
        BulkOpArg::Create => "create",
        BulkOpArg::Update => "update",
    };
    let mut columns: Vec<Column> = Vec::with_capacity(header.len());
    let mut findings: Vec<String> = Vec::new();
    for raw in header {
        match parse_column(raw) {
            Some(column) if allowed.contains(&column) => {
                if columns.contains(&column) {
                    findings.push(format!(
                        "header column {:?}: duplicate column; each column may appear only once",
                        raw.trim()
                    ));
                } else {
                    columns.push(column);
                }
            }
            Some(column) => findings.push(format!(
                "header column {:?}: column {} is not supported by --op {op_name}; supported columns are: {}",
                raw.trim(),
                column.name(),
                allowed
                    .iter()
                    .map(|c| c.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            None => findings.push(format!(
                "header column {:?}: unknown column for --op {op_name}; supported columns are: {}",
                raw.trim(),
                allowed
                    .iter()
                    .map(|c| c.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
    if findings.is_empty() {
        Ok(columns)
    } else {
        Err(aggregate(source, &findings))
    }
}

/// Converts one create row; appends the operation or a row finding.
fn convert_create(
    line: usize,
    columns: &[Column],
    cells: &[&str],
    project: &str,
    operations: &mut Vec<Value>,
    findings: &mut Vec<String>,
) {
    let mut title: Option<String> = None;
    for (column, raw) in columns.iter().zip(cells) {
        match column {
            Column::Title => {
                if raw.trim().is_empty() {
                    findings.push(finding(line, Some(column.name()), "must not be empty"));
                    return;
                }
                if raw.chars().count() > 500 {
                    findings.push(finding(
                        line,
                        Some(column.name()),
                        "must be at most 500 characters",
                    ));
                    return;
                }
                title = Some((*raw).to_string());
            }
            _ => unreachable!("resolve_columns only allows title for create"),
        }
    }
    let Some(title) = title else {
        findings.push(finding(line, Some("title"), "is required"));
        return;
    };
    match serde_json::to_value(BulkCreateWorkItemOperation {
        project_key: project.to_string(),
        title,
        attributes: None,
    }) {
        Ok(value) => operations.push(value),
        Err(err) => findings.push(finding(
            line,
            None,
            format!("internal serialization error: {err}"),
        )),
    }
}

/// Converts one update row; appends the operation or row findings.
fn convert_update(
    line: usize,
    columns: &[Column],
    cells: &[&str],
    project: &str,
    operations: &mut Vec<Value>,
    findings: &mut Vec<String>,
) {
    let mut work_item_key: Option<String> = None;
    let mut revision: Option<i64> = None;
    let mut changes = UpdateWorkItemRequest::default();
    let mut row_ok = true;

    for (column, raw) in columns.iter().zip(cells) {
        match column {
            Column::WorkItemKey => {
                if raw.trim().is_empty() {
                    findings.push(finding(line, Some(column.name()), "must not be empty"));
                    row_ok = false;
                } else {
                    work_item_key = Some((*raw).to_string());
                }
            }
            Column::Revision => match parse_positive_integer(raw) {
                Some(value) => revision = Some(value),
                None => {
                    findings.push(finding(
                        line,
                        Some(column.name()),
                        format!("{:?} is not a positive integer (>= 1)", raw.trim()),
                    ));
                    row_ok = false;
                }
            },
            Column::Title => {
                if raw.is_empty() {
                    // An empty cell omits the field.
                } else if raw.trim().is_empty() {
                    findings.push(finding(line, Some(column.name()), "must not be blank"));
                    row_ok = false;
                } else if raw.chars().count() > 500 {
                    findings.push(finding(
                        line,
                        Some(column.name()),
                        "must be at most 500 characters",
                    ));
                    row_ok = false;
                } else {
                    changes.title = Some((*raw).to_string());
                }
            }
            Column::Description => {
                if !raw.is_empty() {
                    if raw.chars().count() > 64_000 {
                        findings.push(finding(
                            line,
                            Some(column.name()),
                            "must be at most 64000 characters",
                        ));
                        row_ok = false;
                    } else {
                        changes.description = Some(Some((*raw).to_string()));
                    }
                }
            }
            Column::ItemType => {
                if !raw.trim().is_empty() {
                    match normalize_type(raw) {
                        Some(value) => changes.item_type = Some(value.to_string()),
                        None => {
                            findings.push(finding(
                                line,
                                Some(column.name()),
                                format!(
                                    "{:?} is not a Work Item type; expected one of: {}",
                                    raw.trim(),
                                    TypeArg::ALL
                                        .iter()
                                        .map(|kind| kind.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                            ));
                            row_ok = false;
                        }
                    }
                }
            }
            Column::Priority => {
                if !raw.trim().is_empty() {
                    match normalize_priority(raw) {
                        Some(value) => changes.priority = Some(value.to_string()),
                        None => {
                            findings.push(finding(
                                line,
                                Some(column.name()),
                                format!(
                                    "{:?} is not a priority; expected one of: {}",
                                    raw.trim(),
                                    PriorityArg::ALL
                                        .iter()
                                        .map(|priority| priority.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ),
                            ));
                            row_ok = false;
                        }
                    }
                }
            }
            Column::Assignee => {
                if !raw.trim().is_empty() {
                    let value = raw.trim();
                    if value.starts_with("usr_") {
                        changes.assignee_public_id = Some(Some(value.to_string()));
                    } else if uuid::Uuid::parse_str(value).is_ok() {
                        changes.assignee_id = Some(Some(value.to_string()));
                    } else {
                        findings.push(finding(
                            line,
                            Some(column.name()),
                            format!(
                                "{:?} is neither a usr_ public ID nor a UUID; resolve `me` with the API before converting",
                                raw.trim()
                            ),
                        ));
                        row_ok = false;
                    }
                }
            }
            Column::Sprint => {
                if !raw.trim().is_empty() {
                    let value = raw.trim();
                    if uuid::Uuid::parse_str(value).is_ok() {
                        changes.sprint_id = Some(Some(value.to_string()));
                    } else {
                        findings.push(finding(
                            line,
                            Some(column.name()),
                            format!("{:?} is not a Sprint UUID", raw.trim()),
                        ));
                        row_ok = false;
                    }
                }
            }
            Column::Parent => {
                if !raw.trim().is_empty() {
                    let value = raw.trim();
                    if value.chars().count() > 50 {
                        findings.push(finding(
                            line,
                            Some(column.name()),
                            "must be at most 50 characters (Work Item key or parent UUID)",
                        ));
                        row_ok = false;
                    } else {
                        changes.parent_id = Some(Some(value.to_string()));
                    }
                }
            }
            Column::StoryPoints => {
                if !raw.trim().is_empty() {
                    match parse_story_points(raw) {
                        Some(value) => changes.story_points = Some(Some(value)),
                        None => {
                            findings.push(finding(
                                line,
                                Some(column.name()),
                                format!("{:?} is not an integer between 0 and 100000", raw.trim()),
                            ));
                            row_ok = false;
                        }
                    }
                }
            }
            Column::DueDate => {
                if !raw.trim().is_empty() {
                    match TimeArg::from_str(raw.trim()) {
                        Ok(value) => changes.due_date = Some(Some(value.to_string())),
                        Err(err) => {
                            findings.push(finding(
                                line,
                                Some(column.name()),
                                format!("{:?} is not a valid date/time: {err}", raw.trim()),
                            ));
                            row_ok = false;
                        }
                    }
                }
            }
        }
    }

    if !row_ok {
        return;
    }
    let Some(work_item_key) = work_item_key else {
        findings.push(finding(line, Some("workItemKey"), "is required"));
        return;
    };
    if changes.is_empty() {
        findings.push(finding(
            line,
            None,
            "has no change columns; specify at least one of title, description, type, priority, \
             assignee, sprint, parent, storyPoints, dueDate",
        ));
        return;
    }
    match serde_json::to_value(BulkUpdateWorkItemOperation {
        project_key: project.to_string(),
        work_item_key,
        revision,
        changes,
    }) {
        Ok(value) => operations.push(value),
        Err(err) => findings.push(finding(
            line,
            None,
            format!("internal serialization error: {err}"),
        )),
    }
}

/// Normalizes a `type` cell to its documented wire spelling.
fn normalize_type(raw: &str) -> Option<&'static str> {
    let value = raw.trim().to_ascii_lowercase();
    TypeArg::ALL
        .iter()
        .copied()
        .find(|kind| kind.as_str() == value)
        .map(TypeArg::as_str)
}

/// Normalizes a `priority` cell to its documented wire spelling.
fn normalize_priority(raw: &str) -> Option<&'static str> {
    let value = raw.trim().to_ascii_lowercase();
    PriorityArg::ALL
        .iter()
        .copied()
        .find(|priority| priority.as_str() == value)
        .map(PriorityArg::as_str)
}

fn parse_positive_integer(raw: &str) -> Option<i64> {
    raw.trim().parse::<i64>().ok().filter(|value| *value >= 1)
}

fn parse_story_points(raw: &str) -> Option<i64> {
    raw.trim()
        .parse::<i64>()
        .ok()
        .filter(|value| (0..=100_000).contains(value))
}

/// One CSV record with the physical line it started on.
#[derive(Debug)]
struct Record {
    line: usize,
    fields: Vec<String>,
}

/// A fatal structural CSV error (parsing cannot meaningfully continue).
#[derive(Debug)]
struct CsvParseError {
    line: usize,
    message: String,
}

/// RFC 4180 parser. Returns every record including the header. Embedded CRLF
/// inside quoted fields is normalized to LF so values are platform-stable.
fn parse_csv(text: &str) -> Result<Vec<Record>, CsvParseError> {
    let bytes = text.as_bytes();
    let mut pos = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    };
    let mut line = 1usize;
    let mut records: Vec<Record> = Vec::new();

    while pos < bytes.len() {
        let record_line = line;
        let mut fields: Vec<String> = Vec::new();
        loop {
            let mut field = String::new();
            if pos < bytes.len() && bytes[pos] == b'"' {
                pos += 1;
                loop {
                    if pos >= bytes.len() {
                        return Err(CsvParseError {
                            line,
                            message: "unterminated quoted field".to_string(),
                        });
                    }
                    match bytes[pos] {
                        b'"' => {
                            if pos + 1 < bytes.len() && bytes[pos + 1] == b'"' {
                                field.push('"');
                                pos += 2;
                            } else {
                                pos += 1;
                                break;
                            }
                        }
                        b'\r' => {
                            if pos + 1 < bytes.len() && bytes[pos + 1] == b'\n' {
                                pos += 1;
                            }
                            field.push('\n');
                            line += 1;
                            pos += 1;
                        }
                        b'\n' => {
                            field.push('\n');
                            line += 1;
                            pos += 1;
                        }
                        _ => {
                            let Some(ch) = text[pos..].chars().next() else {
                                break;
                            };
                            field.push(ch);
                            pos += ch.len_utf8();
                        }
                    }
                }
                if pos < bytes.len() && !matches!(bytes[pos], b',' | b'\r' | b'\n') {
                    return Err(CsvParseError {
                        line,
                        message: "unexpected character after a closing quote; double `\"\"` \
                                  to embed a quote"
                            .to_string(),
                    });
                }
            } else {
                loop {
                    if pos >= bytes.len() {
                        break;
                    }
                    match bytes[pos] {
                        b',' | b'\r' | b'\n' => break,
                        b'"' => {
                            return Err(CsvParseError {
                                line,
                                message: "unexpected quote in an unquoted field; wrap the field \
                                          in double quotes"
                                    .to_string(),
                            });
                        }
                        _ => {
                            let Some(ch) = text[pos..].chars().next() else {
                                break;
                            };
                            field.push(ch);
                            pos += ch.len_utf8();
                        }
                    }
                }
            }
            fields.push(field);
            if pos < bytes.len() && bytes[pos] == b',' {
                pos += 1;
                continue;
            }
            break;
        }
        records.push(Record {
            line: record_line,
            fields,
        });
        if pos < bytes.len() {
            if bytes[pos] == b'\r' {
                pos += 1;
                if pos < bytes.len() && bytes[pos] == b'\n' {
                    pos += 1;
                }
                line += 1;
            } else if bytes[pos] == b'\n' {
                pos += 1;
                line += 1;
            }
        }
    }
    Ok(records)
}

/// Formats one row/column finding.
fn finding(line: usize, column: Option<&str>, message: impl Into<String>) -> String {
    match column {
        Some(column) => format!("row {line}, column {column:?}: {}", message.into()),
        None => format!("row {line}: {}", message.into()),
    }
}

/// Formats a fatal parser error.
fn parse_error(source: &str, error: CsvParseError) -> CliError {
    CliError::usage(format!(
        "{source} is not valid RFC 4180 CSV at row {}: {}",
        error.line, error.message
    ))
}

/// Aggregates findings into a usage error, one line each.
fn aggregate(source: &str, findings: &[String]) -> CliError {
    let mut message = format!(
        "{source} failed CSV conversion with {} problem{}:",
        findings.len(),
        if findings.len() == 1 { "" } else { "s" }
    );
    for finding in findings {
        message.push_str(&format!("\n  - {finding}"));
    }
    CliError::usage(message)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn titles(records: &[Record]) -> Vec<Vec<&str>> {
        records
            .iter()
            .map(|record| record.fields.iter().map(String::as_str).collect())
            .collect()
    }

    #[test]
    fn parses_plain_records_and_line_numbers() {
        let records = parse_csv("title,priority\nOne,high\nTwo,low\n").unwrap();
        assert_eq!(
            titles(&records),
            vec![
                vec!["title", "priority"],
                vec!["One", "high"],
                vec!["Two", "low"],
            ]
        );
        assert_eq!(records[0].line, 1);
        assert_eq!(records[1].line, 2);
        assert_eq!(records[2].line, 3);
    }

    #[test]
    fn parses_quoted_fields_with_commas_quotes_and_newlines() {
        let records = parse_csv("title,description\n\"A, B\",\"say \"\"hi\"\"\nnext\"\n").unwrap();
        assert_eq!(records[1].fields[0], "A, B");
        assert_eq!(records[1].fields[1], "say \"hi\"\nnext");
        assert_eq!(records[1].line, 2);
    }

    #[test]
    fn crlf_is_accepted() {
        let records = parse_csv("title\r\nOne\r\n").unwrap();
        assert_eq!(titles(&records), vec![vec!["title"], vec!["One"]]);
    }

    #[test]
    fn rejects_unterminated_quote() {
        let err = parse_csv("title\n\"One\n").unwrap_err();
        assert!(err.message.contains("unterminated"), "{err:?}");
    }

    #[test]
    fn rejects_quote_in_unquoted_field() {
        let err = parse_csv("title\nOn\"e\n").unwrap_err();
        assert!(err.message.contains("unquoted"), "{err:?}");
    }

    #[test]
    fn create_csv_emits_typed_envelope() {
        let operations = convert(
            "title\n\"First item\"\nSecond item\n",
            "ops.csv",
            BulkOpArg::Create,
            "HAM",
        )
        .unwrap();
        assert_eq!(operations.len(), 2);
        assert_eq!(
            operations[0],
            json!({ "projectKey": "HAM", "title": "First item" })
        );
    }

    #[test]
    fn update_csv_emits_camel_case_changes() {
        let operations = convert(
            "workItemKey,revision,title,type,priority,storyPoints\n\
             HAM-1,3,New title,BUG,HIGH,5\n",
            "ops.csv",
            BulkOpArg::Update,
            "HAM",
        )
        .unwrap();
        assert_eq!(
            operations[0],
            json!({
                "projectKey": "HAM",
                "workItemKey": "HAM-1",
                "revision": 3,
                "changes": {
                    "title": "New title",
                    "type": "bug",
                    "priority": "high",
                    "storyPoints": 5,
                },
            })
        );
    }

    #[test]
    fn unknown_column_names_the_row_and_column() {
        let err = convert(
            "title,status\nOne,done\n",
            "ops.csv",
            BulkOpArg::Create,
            "HAM",
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("unknown column"), "{message}");
        assert!(message.contains("status"), "{message}");
    }

    #[test]
    fn bad_enum_reports_row_and_column() {
        let err = convert(
            "workItemKey,priority\nHAM-1,urgnet\n",
            "ops.csv",
            BulkOpArg::Update,
            "HAM",
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("row 2"), "{message}");
        assert!(message.contains("\"priority\""), "{message}");
        assert!(message.contains("urgnet"), "{message}");
    }

    #[test]
    fn missing_work_item_key_is_reported_per_row() {
        let err = convert(
            "workItemKey,title\n,One\n",
            "ops.csv",
            BulkOpArg::Update,
            "HAM",
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("row 2"), "{message}");
        assert!(message.contains("workItemKey"), "{message}");
    }

    #[test]
    fn empty_changes_row_is_rejected() {
        let err = convert(
            "workItemKey,revision\nHAM-1,2\n",
            "ops.csv",
            BulkOpArg::Update,
            "HAM",
        )
        .unwrap_err();
        assert!(err.to_string().contains("no change columns"), "{err}");
    }

    #[test]
    fn wrong_column_count_is_reported_per_row() {
        let err = convert(
            "workItemKey,revision\nHAM-1\n",
            "ops.csv",
            BulkOpArg::Update,
            "HAM",
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("row 2"), "{message}");
        assert!(message.contains("expected 2 columns"), "{message}");
    }

    #[test]
    fn empty_cells_omit_optional_fields() {
        // An all-empty row has no change columns.
        let err = convert(
            "workItemKey,title,description\nHAM-1,,\n",
            "ops.csv",
            BulkOpArg::Update,
            "HAM",
        )
        .unwrap_err();
        assert!(err.to_string().contains("no change columns"), "{err}");

        let operations = convert(
            "workItemKey,title,description\nHAM-1,Keep,  \n",
            "ops.csv",
            BulkOpArg::Update,
            "HAM",
        )
        .unwrap();
        assert_eq!(operations[0]["changes"]["title"], "Keep");
        assert_eq!(operations[0]["changes"]["description"], "  ");
    }

    #[test]
    fn duplicate_columns_are_rejected() {
        let err = convert(
            "title,title\nOne,Two\n",
            "ops.csv",
            BulkOpArg::Create,
            "HAM",
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicate column"), "{err}");
    }

    #[test]
    fn assignee_cells_are_trimmed_before_classification() {
        let operations = convert(
            "workItemKey,assignee,sprint\nHAM-1, usr_cPbfeqnghA-RLpDVOMQhHg , 00000000-0000-0000-0000-000000000000 \n",
            "ops.csv",
            BulkOpArg::Update,
            "HAM",
        )
        .unwrap();
        assert_eq!(
            operations[0]["changes"]["assigneePublicId"],
            "usr_cPbfeqnghA-RLpDVOMQhHg"
        );
        assert_eq!(
            operations[0]["changes"]["sprintId"],
            "00000000-0000-0000-0000-000000000000"
        );
    }
}
