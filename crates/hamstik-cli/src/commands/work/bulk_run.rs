// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work bulk run`: resumable, batched execution of a large operation set.
//!
//! The runner reads operations from a JSON array or JSON-lines source (stdin
//! included), preflights every operation against the same schema-derived rules
//! the single-request bulk commands use, and plans them into batches of at most
//! [`MAX_BULK_OPERATIONS`]. Each batch's exact request body and idempotency key
//! are written to a local, append-only journal *before* execution, so an
//! interrupted run resumes from the journal without redoing completed batches or
//! duplicating creates. Separate batches are separate API requests and are not
//! one atomic transaction.
//!
//! The journal is a JSON-lines file with four record kinds (see the README
//! "Bulk operations and preflight" section for the documented schema):
//!
//! ```text
//! {"type":"header",...}   exactly once, identifies the run
//! {"type":"batch",...}    one plan per batch (key + exact operations)
//! {"type":"complete",...} written once planning is finished
//! {"type":"result",...}   one per batch execution attempt
//! ```
//!
//! The journal carries the organization, operation kind, concurrency mode,
//! operations, idempotency keys, and server results. It never carries
//! credentials, tokens, or request headers.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use hamstik_api_client::{
    BulkCreateEnvelope, BulkCreateWorkItemOperation, BulkTransitionEnvelope,
    BulkTransitionWorkItemOperation, BulkUpdateEnvelope, BulkUpdateWorkItemOperation, ClientError,
    HamstikApi, generate_key,
};

use crate::app::Session;
use crate::args::{BulkRunOpArg, ConcurrencyArg, WorkBulkRunArgs};
use crate::error::CliError;
use crate::exit;

use super::bulk_preflight::{self, MAX_BULK_OPERATIONS, PreflightKind};
use super::emit_json;

/// The journal schema version this build writes and accepts.
const JOURNAL_VERSION: u32 = 1;

/// The largest JSON-array operations document accepted from a file or stdin.
const MAX_ARRAY_BYTES: usize = 1024 * 1024;

/// The largest single JSON-lines operation accepted.
const MAX_OPERATION_BYTES: usize = 1024 * 1024;

/// Runs `work bulk run`.
pub(super) async fn run(session: &mut Session<'_>, args: &WorkBulkRunArgs) -> Result<(), CliError> {
    let kind = preflight_kind(args.op);
    let selection = session.selection()?;
    let path = PathBuf::from(&args.journal);

    if args.restart {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(CliError::general(format!(
                    "cannot replace journal {}: {err}",
                    path.display()
                )));
            }
        }
    }

    let existing = load_journal(&path)?;
    let resumed = existing.is_some();
    let org = match selection.organization.value.clone() {
        Some(org) => org,
        None => existing
            .as_ref()
            .map(|journal| journal.header.organization.clone())
            .ok_or_else(|| {
                CliError::usage(
                    "no organization selected; pass --org, set HAMSTIK_ORG, or run `hamstik org use <slug>`",
                )
            })?,
    };
    let api = session.api(&selection)?;

    let mut journal = match (existing, args.operations_file.as_deref()) {
        (Some(journal), Some(source)) => {
            validate_header(&journal, &org, kind, args.concurrency)?;
            plan_operations(&path, source, kind, &journal.header, &journal.batches)?;
            load_journal(&path)?
                .ok_or_else(|| CliError::general("journal disappeared after planning"))?
        }
        (Some(journal), None) => {
            validate_header(&journal, &org, kind, args.concurrency)?;
            if !journal.complete {
                return Err(CliError::usage(
                    "the journal was interrupted before all operations were planned; re-run with --operations-file to finish planning, or --restart to start over",
                ));
            }
            journal
        }
        (None, Some(source)) => {
            let concurrency = effective_concurrency(kind, args.concurrency)?;
            let header = JournalHeader {
                journal_version: JOURNAL_VERSION,
                operation: kind_name(kind).to_string(),
                organization: org.clone(),
                concurrency,
                source: source.to_string(),
                created_at: now(),
            };
            ensure_header(&path, &header)?;
            plan_operations(&path, source, kind, &header, &[])?;
            load_journal(&path)?
                .ok_or_else(|| CliError::general("journal disappeared after planning"))?
        }
        (None, None) => {
            return Err(CliError::usage(
                "--operations-file is required when starting a new run; pass a path or `-` for stdin, or point --journal at a completed journal to resume",
            ));
        }
    };

    let concurrency = journal.header.concurrency.clone();
    let sender = BatchSender {
        api: api.as_ref(),
        org: &org,
        kind,
        concurrency: concurrency.as_deref(),
    };
    execute(
        session,
        &sender,
        &path,
        &mut journal.batches,
        args.retry_failed,
    )
    .await?;
    // A skipped failed/uncertain/pending batch still means the run is not
    // clean: surface a non-zero exit code even though no new request was made.
    if session.exit_code == exit::SUCCESS {
        let summary = summarize(&journal.batches);
        if summary.uncertain > 0 {
            session.exit_code = exit::NETWORK;
        } else if summary.failed > 0 {
            session.exit_code = exit::GENERAL;
        }
    }
    render(session, &journal, kind, &org, &path, resumed)
}

/// Maps the CLI operation selector to the shared preflight kind.
fn preflight_kind(op: BulkRunOpArg) -> PreflightKind {
    match op {
        BulkRunOpArg::Create => PreflightKind::Create,
        BulkRunOpArg::Update => PreflightKind::Update,
        BulkRunOpArg::Transition => PreflightKind::Transition,
    }
}

/// The stable journal name for a bulk kind.
fn kind_name(kind: PreflightKind) -> &'static str {
    match kind {
        PreflightKind::Create => "create",
        PreflightKind::Update => "update",
        PreflightKind::Transition => "transition",
    }
}

/// The concurrency mode recorded for a fresh run.
fn effective_concurrency(
    kind: PreflightKind,
    arg: Option<ConcurrencyArg>,
) -> Result<Option<String>, CliError> {
    match kind {
        PreflightKind::Create => {
            if arg.is_some() {
                return Err(CliError::usage(
                    "--concurrency applies to update and transition batches, not create",
                ));
            }
            Ok(None)
        }
        PreflightKind::Update => Ok(Some(
            arg.unwrap_or(ConcurrencyArg::RequireRevision)
                .as_str()
                .to_string(),
        )),
        PreflightKind::Transition => Ok(arg.map(|mode| mode.as_str().to_string())),
    }
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ---------------------------------------------------------------------------
// Journal schema
// ---------------------------------------------------------------------------

/// One JSON-lines journal record.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum JournalRecord {
    /// The run identity, written once before any plan.
    #[serde(rename = "header")]
    Header(JournalHeader),
    /// One planned batch: its idempotency key and exact operations.
    #[serde(rename = "batch")]
    Plan(JournalBatchPlan),
    /// Marks planning complete, so a source-less resume is safe.
    #[serde(rename = "complete")]
    Complete(JournalComplete),
    /// The outcome of one batch execution attempt.
    #[serde(rename = "result")]
    Result(JournalBatchResult),
}

/// Journal header: the immutable identity of a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalHeader {
    /// Schema version.
    journal_version: u32,
    /// `create`, `update`, or `transition`.
    operation: String,
    /// Organization slug the batches address.
    organization: String,
    /// Concurrency mode for update/transition batches.
    #[serde(skip_serializing_if = "Option::is_none")]
    concurrency: Option<String>,
    /// Human description of the operations source (`-` for stdin).
    source: String,
    /// Creation timestamp (RFC 3339).
    created_at: String,
}

/// A planned batch, written before any request is sent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalBatchPlan {
    /// Zero-based batch position.
    sequence: u64,
    /// The idempotency key reused for every attempt of this batch.
    idempotency_key: String,
    /// The exact operations in this batch (at most 50).
    operations: Vec<Value>,
}

/// The planning-complete marker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalComplete {
    /// Completion timestamp (RFC 3339).
    at: String,
}

/// The outcome of one batch execution attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalBatchResult {
    /// The batch this result belongs to.
    sequence: u64,
    /// `completed`, `failed`, or `uncertain`.
    state: BatchState,
    /// When the request was attempted (RFC 3339).
    #[serde(skip_serializing_if = "Option::is_none")]
    attempted_at: Option<String>,
    /// When the outcome was recorded (RFC 3339).
    recorded_at: String,
    /// Raw server `results` array for a completed batch.
    #[serde(skip_serializing_if = "Option::is_none")]
    results: Option<Vec<Value>>,
    /// The request-level failure for a failed/uncertain batch.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JournalError>,
}

/// A request-level failure recorded in the journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalError {
    /// Stable error code.
    code: String,
    /// Human message.
    message: String,
    /// HTTP status, when the server answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    /// Server correlation id, when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
}

impl JournalError {
    fn from_cli(err: &CliError) -> Self {
        Self {
            code: err.code.clone(),
            message: err.message.clone(),
            status: err.status,
            request_id: err.request_id.clone(),
        }
    }
}

/// The terminal state of a batch execution attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BatchState {
    /// The request succeeded; per-operation results are recorded.
    Completed,
    /// The server answered with a request-level error; never auto-retried.
    Failed,
    /// A network failure left the outcome unknown; safe to replay.
    Uncertain,
}

/// In-memory view of a journal.
struct Journal {
    /// The run identity.
    header: JournalHeader,
    /// Planned batches in sequence order, with any recorded outcomes.
    batches: Vec<Batch>,
    /// True once the planning-complete marker was read.
    complete: bool,
}

/// One planned batch plus its latest recorded outcome.
struct Batch {
    /// Zero-based batch position.
    sequence: u64,
    /// Stable idempotency key.
    idempotency_key: String,
    /// The exact operations sent for this batch.
    operations: Vec<Value>,
    /// The latest recorded state (`None` while unexecuted).
    state: Option<BatchState>,
    /// Raw server results for a completed attempt.
    results: Option<Vec<Value>>,
    /// Request-level failure for a failed/uncertain attempt.
    error: Option<JournalError>,
    /// When the latest attempt was sent.
    attempted_at: Option<String>,
}

/// Writes the header if the journal file does not exist yet.
fn ensure_header(path: &Path, header: &JournalHeader) -> Result<(), CliError> {
    if path.exists() {
        return Ok(());
    }
    append_record(path, &JournalRecord::Header(header.clone()))?;
    let _ = crate::fsutil::restrict_permissions(path);
    Ok(())
}

/// Streams `source`, verifies any already-planned batches, appends new plans,
/// and writes the planning-complete marker.
fn plan_operations(
    path: &Path,
    source: &str,
    kind: PreflightKind,
    header: &JournalHeader,
    existing: &[Batch],
) -> Result<(), CliError> {
    ensure_header(path, header)?;
    let mut cursor = PlanCursor::new(existing);
    let mut pending: Vec<Value> = Vec::with_capacity(MAX_BULK_OPERATIONS);
    let mut sequence = existing.len() as u64;
    for_each_operation(source, kind, &mut |index, operation| {
        if let Some(expected) = cursor.expected() {
            if expected != &operation {
                return Err(CliError::usage(format!(
                    "operations source operation {index} differs from the journal plan; use --restart to start a new run"
                )));
            }
            cursor.advance();
            return Ok(());
        }
        pending.push(operation);
        if pending.len() == MAX_BULK_OPERATIONS {
            append_plan(path, sequence, &mut pending)?;
            sequence += 1;
        }
        Ok(())
    })?;
    if !cursor.finished() {
        return Err(CliError::usage(
            "operations source contains fewer operations than the journal plan; use --restart to start a new run",
        ));
    }
    if !pending.is_empty() {
        append_plan(path, sequence, &mut pending)?;
    }
    append_record(
        path,
        &JournalRecord::Complete(JournalComplete { at: now() }),
    )?;
    Ok(())
}

/// Appends one planned batch, assigning a fresh idempotency key.
fn append_plan(path: &Path, sequence: u64, pending: &mut Vec<Value>) -> Result<(), CliError> {
    let operations = std::mem::take(pending);
    append_record(
        path,
        &JournalRecord::Plan(JournalBatchPlan {
            sequence,
            idempotency_key: generate_key(),
            operations,
        }),
    )
}

/// Flattened cursor over the already-planned batches.
struct PlanCursor<'a> {
    batches: &'a [Batch],
    batch: usize,
    offset: usize,
}

impl<'a> PlanCursor<'a> {
    fn new(batches: &'a [Batch]) -> Self {
        Self {
            batches,
            batch: 0,
            offset: 0,
        }
    }

    /// The next expected operation, or `None` once every plan is consumed.
    fn expected(&mut self) -> Option<&'a Value> {
        while self.batch < self.batches.len() {
            let operations = &self.batches[self.batch].operations;
            if self.offset < operations.len() {
                return Some(&operations[self.offset]);
            }
            self.batch += 1;
            self.offset = 0;
        }
        None
    }

    /// Advances past the operation returned by [`PlanCursor::expected`].
    fn advance(&mut self) {
        self.offset += 1;
    }

    /// True once every planned operation has been matched.
    fn finished(&mut self) -> bool {
        self.expected().is_none()
    }
}

/// Appends one journal record and durably flushes it.
fn append_record(path: &Path, record: &JournalRecord) -> Result<(), CliError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|err| {
            CliError::general(format!(
                "cannot create journal directory {}: {err}",
                parent.display()
            ))
        })?;
    }
    let line = serde_json::to_string(record).map_err(CliError::general)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|err| {
            CliError::general(format!("cannot write journal {}: {err}", path.display()))
        })?;
    writeln!(file, "{line}").map_err(|err| {
        CliError::general(format!("cannot write journal {}: {err}", path.display()))
    })?;
    file.flush().map_err(|err| {
        CliError::general(format!("cannot flush journal {}: {err}", path.display()))
    })?;
    file.sync_all().map_err(|err| {
        CliError::general(format!("cannot sync journal {}: {err}", path.display()))
    })?;
    Ok(())
}

/// Loads and replays a journal, truncating a torn trailing record if present.
fn load_journal(path: &Path) -> Result<Option<Journal>, CliError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(CliError::general(format!(
                "cannot read journal {}: {err}",
                path.display()
            )));
        }
    };
    // Every complete line ends with a newline; bytes after the last newline are
    // a torn append from an interrupted write and are discarded.
    let valid_len = if text.ends_with('\n') {
        text.len()
    } else {
        text.rfind('\n').map_or(0, |index| index + 1)
    };
    if valid_len < text.len() {
        let file = OpenOptions::new().write(true).open(path).map_err(|err| {
            CliError::general(format!("cannot repair journal {}: {err}", path.display()))
        })?;
        file.set_len(valid_len as u64).map_err(|err| {
            CliError::general(format!("cannot repair journal {}: {err}", path.display()))
        })?;
    }

    let mut header: Option<JournalHeader> = None;
    let mut complete = false;
    let mut batches: Vec<Batch> = Vec::new();
    let mut by_sequence: BTreeMap<u64, usize> = BTreeMap::new();
    for (line_no, line) in text[..valid_len].lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: JournalRecord = serde_json::from_str(line).map_err(|err| {
            CliError::usage(format!(
                "journal {} line {} is not a valid record: {err}",
                path.display(),
                line_no + 1
            ))
        })?;
        match record {
            JournalRecord::Header(found) => {
                if header.is_some() {
                    return Err(corrupt(path, "has more than one header"));
                }
                header = Some(found);
            }
            JournalRecord::Plan(plan) => {
                if header.is_none() {
                    return Err(corrupt(path, "plans a batch before its header"));
                }
                if by_sequence.contains_key(&plan.sequence) {
                    return Err(corrupt(
                        path,
                        &format!("plans batch {} more than once", plan.sequence),
                    ));
                }
                by_sequence.insert(plan.sequence, batches.len());
                batches.push(Batch {
                    sequence: plan.sequence,
                    idempotency_key: plan.idempotency_key,
                    operations: plan.operations,
                    state: None,
                    results: None,
                    error: None,
                    attempted_at: None,
                });
            }
            JournalRecord::Complete(_) => complete = true,
            JournalRecord::Result(result) => {
                let Some(&index) = by_sequence.get(&result.sequence) else {
                    return Err(corrupt(
                        path,
                        &format!("records a result for unplanned batch {}", result.sequence),
                    ));
                };
                let batch = &mut batches[index];
                batch.state = Some(result.state);
                batch.results = result.results;
                batch.error = result.error;
                batch.attempted_at = result.attempted_at;
            }
        }
    }
    // Batches are planned sequentially; a gap means the journal is corrupt.
    for (expected, batch) in batches.iter().enumerate() {
        if batch.sequence != expected as u64 {
            return Err(corrupt(
                path,
                &format!(
                    "batch sequence {} is out of order (expected {expected})",
                    batch.sequence
                ),
            ));
        }
    }

    let Some(header) = header else {
        return Ok(None);
    };
    Ok(Some(Journal {
        header,
        batches,
        complete,
    }))
}

fn corrupt(path: &Path, detail: &str) -> CliError {
    CliError::usage(format!("journal {} {detail}", path.display()))
}

/// Validates that a loaded journal matches the invocation.
fn validate_header(
    journal: &Journal,
    org: &str,
    kind: PreflightKind,
    explicit_concurrency: Option<ConcurrencyArg>,
) -> Result<(), CliError> {
    if journal.header.journal_version != JOURNAL_VERSION {
        return Err(CliError::usage(format!(
            "journal schema version {} is not supported; expected {JOURNAL_VERSION}",
            journal.header.journal_version
        )));
    }
    if journal.header.operation != kind_name(kind) {
        return Err(CliError::usage(format!(
            "journal records `{}` operations but --op is `{}`; use --restart to start a new run",
            journal.header.operation,
            kind_name(kind)
        )));
    }
    if journal.header.organization != org {
        return Err(CliError::usage(format!(
            "journal belongs to organization `{}` but `{org}` was selected; use --restart to start a new run",
            journal.header.organization
        )));
    }
    if let Some(explicit) = explicit_concurrency {
        let stored = journal.header.concurrency.as_deref().unwrap_or("none");
        if stored != explicit.as_str() {
            return Err(CliError::usage(format!(
                "journal was created with concurrency `{stored}` but --concurrency {} was passed; omit the flag to use the journal's mode or use --restart",
                explicit.as_str()
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Streaming operations input
// ---------------------------------------------------------------------------

/// Streams operations from `source` (`-` means stdin), validates each against
/// the shared preflight rules, and visits it with its global index.
///
/// A source whose first non-whitespace byte is `[` is a JSON array (read as one
/// capped document); anything else is JSON-lines, read one operation at a time
/// so a pipeline never materializes the whole stream.
fn for_each_operation(
    source: &str,
    kind: PreflightKind,
    visit: &mut dyn FnMut(usize, Value) -> Result<(), CliError>,
) -> Result<(), CliError> {
    let label = if source == "-" { "stdin" } else { source };
    if source == "-" {
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        for_each_operation_reader(&mut reader, label, kind, visit)
    } else {
        let file = File::open(source)
            .map_err(|err| CliError::usage(format!("cannot read {label}: {err}")))?;
        let mut reader = BufReader::new(file);
        for_each_operation_reader(&mut reader, label, kind, visit)
    }
}

fn for_each_operation_reader<R: BufRead>(
    reader: &mut R,
    source: &str,
    kind: PreflightKind,
    visit: &mut dyn FnMut(usize, Value) -> Result<(), CliError>,
) -> Result<(), CliError> {
    let is_array = {
        let buffered = reader
            .fill_buf()
            .map_err(|err| CliError::usage(format!("cannot read {source}: {err}")))?;
        buffered
            .iter()
            .find(|byte| !byte.is_ascii_whitespace())
            .is_some_and(|byte| *byte == b'[')
    };
    if is_array {
        let text = crate::input::read_capped(reader, MAX_ARRAY_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read {source}: {err}")))?;
        let parsed: Value = serde_json::from_str(&text)
            .map_err(|err| CliError::usage(format!("{source} is not valid JSON: {err}")))?;
        let Value::Array(items) = parsed else {
            return Err(CliError::usage(format!(
                "{source} must contain a JSON array of operations, not {}",
                type_name(&parsed)
            )));
        };
        if items.is_empty() {
            return Err(CliError::usage(format!("{source} contains no operations")));
        }
        for (index, item) in items.into_iter().enumerate() {
            bulk_preflight::preflight_operation(source, index, &item, kind)?;
            visit(index, item)?;
        }
        return Ok(());
    }

    let mut index = 0usize;
    let mut line_no = 0usize;
    loop {
        line_no += 1;
        let Some(line) = read_line_capped(reader, MAX_OPERATION_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read {source}: {err}")))?
        else {
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        let item: Value = serde_json::from_str(&line).map_err(|err| {
            CliError::usage(format!("{source} line {line_no} is not valid JSON: {err}"))
        })?;
        bulk_preflight::preflight_operation(source, index, &item, kind)?;
        visit(index, item)?;
        index += 1;
    }
    if index == 0 {
        return Err(CliError::usage(format!("{source} contains no operations")));
    }
    Ok(())
}

/// Reads one newline-terminated line with a byte cap, returning `None` at EOF.
fn read_line_capped<R: BufRead>(reader: &mut R, max: usize) -> std::io::Result<Option<String>> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let mut chunk: Vec<u8> = Vec::new();
        let mut newline_at: Option<usize> = None;
        let mut eof = false;
        {
            let buffered = reader.fill_buf()?;
            if buffered.is_empty() {
                eof = true;
            } else if let Some(position) = buffered.iter().position(|byte| *byte == b'\n') {
                chunk.extend_from_slice(&buffered[..position]);
                newline_at = Some(position);
            } else {
                chunk.extend_from_slice(buffered);
            }
        }
        if eof {
            if line.is_empty() && chunk.is_empty() {
                return Ok(None);
            }
            if line.len() + chunk.len() > max {
                return Err(too_large(max));
            }
            line.extend_from_slice(&chunk);
            break;
        }
        if line.len() + chunk.len() > max {
            return Err(too_large(max));
        }
        line.extend_from_slice(&chunk);
        if let Some(position) = newline_at {
            reader.consume(position + 1);
            break;
        }
        reader.consume(chunk.len());
    }
    String::from_utf8(line)
        .map(Some)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.utf8_error()))
}

fn too_large(max: usize) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("operation exceeds the {max} byte limit"),
    )
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// The outcome of one batch request.
enum BatchOutcome {
    /// The server returned a `results` array.
    Completed { results: Vec<Value>, replayed: bool },
    /// The server answered with a request-level error; never auto-retried.
    Failed {
        error: JournalError,
        exit_code: i32,
        terminal: bool,
    },
    /// A transport failure left the outcome unknown; safe to replay.
    Uncertain { error: JournalError, exit_code: i32 },
}

/// The invariant context shared by every batch request.
struct BatchSender<'a> {
    api: &'a dyn HamstikApi,
    org: &'a str,
    kind: PreflightKind,
    concurrency: Option<&'a str>,
}

/// Sends every executable batch in order, appending each outcome to the journal.
async fn execute(
    session: &mut Session<'_>,
    sender: &BatchSender<'_>,
    journal_path: &Path,
    batches: &mut [Batch],
    retry_failed: bool,
) -> Result<(), CliError> {
    let total = batches.len();
    for batch in batches.iter_mut() {
        let should_send = match batch.state {
            None | Some(BatchState::Uncertain) => true,
            Some(BatchState::Completed) => false,
            Some(BatchState::Failed) => retry_failed,
        };
        if !should_send {
            session.out.verbose(&format!(
                "batch {}/{}: {} (skipped)",
                batch.sequence + 1,
                total,
                state_name(batch.state)
            ));
            continue;
        }

        // Mark the attempt before the request, so an interruption between send
        // and response leaves an explicit `uncertain` record rather than a bare
        // pending plan. The idempotency key makes the replay safe.
        let attempted_at = now();
        append_record(
            journal_path,
            &JournalRecord::Result(JournalBatchResult {
                sequence: batch.sequence,
                state: BatchState::Uncertain,
                attempted_at: Some(attempted_at.clone()),
                recorded_at: now(),
                results: None,
                error: None,
            }),
        )?;
        let outcome = sender
            .send(&batch.operations, &batch.idempotency_key)
            .await?;
        let (state, results, error, exit_code, terminal) = match outcome {
            BatchOutcome::Completed { results, replayed } => {
                session.out.verbose(&format!(
                    "batch {}/{}: completed{}",
                    batch.sequence + 1,
                    total,
                    if replayed { " (idempotent replay)" } else { "" }
                ));
                (
                    BatchState::Completed,
                    Some(results),
                    None,
                    exit::SUCCESS,
                    false,
                )
            }
            BatchOutcome::Failed {
                error,
                exit_code,
                terminal,
            } => {
                session.out.warn(&format!(
                    "batch {}/{} failed: {}",
                    batch.sequence + 1,
                    total,
                    error.message
                ));
                (BatchState::Failed, None, Some(error), exit_code, terminal)
            }
            BatchOutcome::Uncertain { error, exit_code } => {
                session.out.warn(&format!(
                    "batch {}/{} is uncertain (network failure): {}",
                    batch.sequence + 1,
                    total,
                    error.message
                ));
                (BatchState::Uncertain, None, Some(error), exit_code, true)
            }
        };

        append_record(
            journal_path,
            &JournalRecord::Result(JournalBatchResult {
                sequence: batch.sequence,
                state,
                attempted_at: Some(attempted_at.clone()),
                recorded_at: now(),
                results: results.clone(),
                error: error.clone(),
            }),
        )?;
        batch.state = Some(state);
        batch.results = results;
        batch.error = error;
        batch.attempted_at = Some(attempted_at);

        if exit_code != exit::SUCCESS && session.exit_code == exit::SUCCESS {
            session.exit_code = exit_code;
        }
        if terminal {
            break;
        }
    }
    Ok(())
}

impl BatchSender<'_> {
    /// Sends one batch, feeding the existing typed bulk envelope and
    /// idempotency key so internal retries and resume replays reuse the exact
    /// same request.
    async fn send(
        &self,
        operations: &[Value],
        idempotency_key: &str,
    ) -> Result<BatchOutcome, CliError> {
        let result = match decode_batch(self.kind, self.concurrency, operations)? {
            BatchBody::Create(body) => {
                self.api
                    .bulk_create_work_items(self.org, &body, idempotency_key)
                    .await
            }
            BatchBody::Update(body) => {
                self.api
                    .bulk_update_work_items(self.org, &body, idempotency_key)
                    .await
            }
            BatchBody::Transition(body) => {
                self.api
                    .bulk_transition_work_items(self.org, &body, idempotency_key)
                    .await
            }
        };
        self.classify(result)
    }

    /// Converts a client result into a journaled batch outcome.
    fn classify(
        &self,
        result: Result<
            hamstik_api_client::ApiResponse<hamstik_api_client::BulkResultList>,
            ClientError,
        >,
    ) -> Result<BatchOutcome, CliError> {
        match result {
            Ok(response) => {
                let results = response
                    .raw
                    .get("results")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                Ok(BatchOutcome::Completed {
                    results,
                    replayed: response.idempotency_replayed,
                })
            }
            Err(ClientError::Host(host)) => Err(CliError::config(format!("invalid host: {host}"))),
            Err(ClientError::Network { message, .. }) => {
                let error = JournalError {
                    code: "NETWORK_ERROR".to_string(),
                    message,
                    status: None,
                    request_id: None,
                };
                Ok(BatchOutcome::Uncertain {
                    error,
                    exit_code: exit::NETWORK,
                })
            }
            Err(other) => {
                let cli = CliError::from_client(other);
                let exit_code = cli.exit_code();
                let terminal = is_terminal(&cli);
                Ok(BatchOutcome::Failed {
                    error: JournalError::from_cli(&cli),
                    exit_code,
                    terminal,
                })
            }
        }
    }
}

/// True for failures that make continuing pointless (auth and server faults).
fn is_terminal(err: &CliError) -> bool {
    matches!(
        err.code.as_str(),
        "AUTH_REQUIRED"
            | "INVALID_TOKEN"
            | "INSUFFICIENT_SCOPE"
            | "FORBIDDEN"
            | "ORGANIZATION_SUSPENDED"
    ) || err.status.is_some_and(|status| status >= 500)
}

/// The typed request body for one batch.
enum BatchBody {
    Create(BulkCreateEnvelope),
    Update(BulkUpdateEnvelope),
    Transition(BulkTransitionEnvelope),
}

/// Rebuilds the existing typed bulk envelope from the journal's raw operations.
fn decode_batch(
    kind: PreflightKind,
    concurrency: Option<&str>,
    operations: &[Value],
) -> Result<BatchBody, CliError> {
    let value = Value::Array(operations.to_vec());
    match kind {
        PreflightKind::Create => {
            let operations: Vec<BulkCreateWorkItemOperation> = serde_json::from_value(value)
                .map_err(|err| CliError::protocol(format!("invalid create batch: {err}")))?;
            Ok(BatchBody::Create(BulkCreateEnvelope { operations }))
        }
        PreflightKind::Update => {
            let operations: Vec<BulkUpdateWorkItemOperation> = serde_json::from_value(value)
                .map_err(|err| CliError::protocol(format!("invalid update batch: {err}")))?;
            Ok(BatchBody::Update(BulkUpdateEnvelope {
                concurrency: concurrency.unwrap_or("require-revision").to_string(),
                operations,
            }))
        }
        PreflightKind::Transition => {
            let operations: Vec<BulkTransitionWorkItemOperation> = serde_json::from_value(value)
                .map_err(|err| CliError::protocol(format!("invalid transition batch: {err}")))?;
            Ok(BatchBody::Transition(BulkTransitionEnvelope {
                concurrency: concurrency.map(str::to_string),
                operations,
            }))
        }
    }
}

fn state_name(state: Option<BatchState>) -> &'static str {
    match state {
        Some(BatchState::Completed) => "completed",
        Some(BatchState::Failed) => "failed",
        Some(BatchState::Uncertain) => "uncertain",
        None => "pending",
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// Per-state batch counts.
struct Summary {
    completed: usize,
    failed: usize,
    uncertain: usize,
    pending: usize,
    operations: usize,
    failed_operations: usize,
}

fn summarize(batches: &[Batch]) -> Summary {
    let mut summary = Summary {
        completed: 0,
        failed: 0,
        uncertain: 0,
        pending: 0,
        operations: 0,
        failed_operations: 0,
    };
    for batch in batches {
        summary.operations += batch.operations.len();
        match batch.state {
            Some(BatchState::Completed) => {
                summary.completed += 1;
                if let Some(results) = &batch.results {
                    summary.failed_operations += results
                        .iter()
                        .filter(|result| result.get("error").is_some_and(|err| !err.is_null()))
                        .count();
                }
            }
            Some(BatchState::Failed) => summary.failed += 1,
            Some(BatchState::Uncertain) => summary.uncertain += 1,
            None => summary.pending += 1,
        }
    }
    summary
}

/// Renders the run: the full batch journal in structured modes, an actionable
/// per-batch summary in human mode, and always the non-atomicity note.
fn render(
    session: &mut Session<'_>,
    journal: &Journal,
    kind: PreflightKind,
    org: &str,
    path: &Path,
    resumed: bool,
) -> Result<(), CliError> {
    let summary = summarize(&journal.batches);
    if session.json() {
        let batches: Vec<Value> = journal
            .batches
            .iter()
            .map(|batch| {
                json!({
                    "sequence": batch.sequence,
                    "state": state_name(batch.state),
                    "idempotencyKey": batch.idempotency_key,
                    "operationCount": batch.operations.len(),
                    "attemptedAt": batch.attempted_at,
                    "results": batch.results,
                    "error": batch.error,
                })
            })
            .collect();
        return emit_json(
            session,
            &json!({
                "operation": kind_name(kind),
                "organization": org,
                "journal": path.display().to_string(),
                "source": journal.header.source,
                "createdAt": journal.header.created_at,
                "resumed": resumed,
                "batches": batches,
                "summary": {
                    "batches": journal.batches.len(),
                    "completed": summary.completed,
                    "failed": summary.failed,
                    "uncertain": summary.uncertain,
                    "pending": summary.pending,
                    "operations": summary.operations,
                    "failedOperations": summary.failed_operations,
                },
                "atomic": false,
                "note": "each batch is a separate API request; batches are not one atomic transaction",
            }),
        );
    }

    if session.out.is_quiet() {
        for batch in &journal.batches {
            session
                .out
                .line(&format!(
                    "{}\t{}\t{}",
                    batch.sequence,
                    state_name(batch.state),
                    batch.operations.len()
                ))
                .map_err(CliError::general)?;
        }
        session.out.warn(
            "note: each batch is a separate API request; batches are not one atomic transaction",
        );
        return Ok(());
    }

    session
        .out
        .line(&format!("journal: {}", path.display()))
        .map_err(CliError::general)?;
    session
        .out
        .line(&format!(
            "batches: {} ({} completed, {} failed, {} uncertain, {} pending)",
            journal.batches.len(),
            summary.completed,
            summary.failed,
            summary.uncertain,
            summary.pending
        ))
        .map_err(CliError::general)?;

    for batch in &journal.batches {
        match batch.state {
            Some(BatchState::Completed) => {
                let total = batch.operations.len();
                let failed = batch
                    .results
                    .as_ref()
                    .map(|results| {
                        results
                            .iter()
                            .filter(|result| result.get("error").is_some_and(|err| !err.is_null()))
                            .count()
                    })
                    .unwrap_or(0);
                session
                    .out
                    .line(&format!(
                        "batch {}: completed, {}/{} operations succeeded",
                        batch.sequence,
                        total - failed,
                        total
                    ))
                    .map_err(CliError::general)?;
                if let Some(results) = &batch.results {
                    for result in results {
                        if result.get("error").is_some_and(|err| !err.is_null()) {
                            let index = result.get("index").and_then(Value::as_i64).unwrap_or(0);
                            let status = result.get("status").and_then(Value::as_i64).unwrap_or(0);
                            let error = result.get("error").unwrap_or(&Value::Null);
                            let code = error
                                .get("code")
                                .and_then(Value::as_str)
                                .unwrap_or("UNKNOWN");
                            let message = error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("the server did not provide a message");
                            session
                                .out
                                .line(&format!(
                                    "  - operations[{index}] (HTTP {status}) {code}: {message}"
                                ))
                                .map_err(CliError::general)?;
                        }
                    }
                }
            }
            Some(BatchState::Failed) | Some(BatchState::Uncertain) => {
                let error = batch.error.as_ref();
                let code = error.map(|err| err.code.as_str()).unwrap_or("UNKNOWN");
                let message = error.map(|err| err.message.as_str()).unwrap_or_default();
                session
                    .out
                    .line(&format!(
                        "batch {}: {} ({code}): {message}",
                        batch.sequence,
                        state_name(batch.state)
                    ))
                    .map_err(CliError::general)?;
            }
            None => {
                session
                    .out
                    .line(&format!(
                        "batch {}: pending ({} operations, not sent)",
                        batch.sequence,
                        batch.operations.len()
                    ))
                    .map_err(CliError::general)?;
            }
        }
    }

    session
        .out
        .line("note: each batch is a separate API request; batches are not one atomic transaction")
        .map_err(CliError::general)?;
    if summary.failed > 0 {
        session
            .out
            .warn("failed batches are never retried automatically; review the journal and re-run with --retry-failed to retry them");
    }
    if summary.pending > 0 || summary.uncertain > 0 {
        session
            .out
            .warn("the journal was preserved; re-run the same command to resume");
    }
    Ok(())
}
