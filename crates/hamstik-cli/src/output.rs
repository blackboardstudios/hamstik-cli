// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Output rendering: human tables, machine JSON, and quiet/verbose handling.
//!
//! Contract (SPEC §39): success content goes to stdout, diagnostics/warnings go
//! to stderr, and `--json` output never contains ANSI codes. Failure rendering
//! switches between a human message and the stable JSON envelope.

use std::borrow::Cow;
use std::io::{self, Write};

use std::rc::Rc;

use bytes::Bytes;
use jaq_all::jaq_core::unwrap_valr;
use jaq_all::json::{self as json, Map, Val};
use serde_json::Number;
use serde_json::Value;

use crate::error::CliError;

/// The mutually-exclusive output modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Human-readable tables and detail views.
    Human,
    /// Stable JSON envelopes.
    Json,
    /// Only essential identifiers.
    Quiet,
    /// One JSON object per line (JSON Lines / NDJSON).
    JsonLines,
    /// Tab-separated values, one record per line.
    Tsv,
}

impl Mode {
    /// True when the mode emits machine-readable structured data.
    #[must_use]
    pub fn is_structured(self) -> bool {
        matches!(self, Self::Json | Self::JsonLines | Self::Tsv)
    }
}

/// Rendering options shared by list output modes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OutputOptions {
    /// Restrict output to these columns (header names), in order.
    /// `None` means all columns are emitted.
    pub columns: Option<Vec<String>>,
    /// Suppress the header row where applicable.
    pub no_header: bool,
}

/// Buffered output sink with mode awareness.
pub struct Output {
    mode: Mode,
    verbose: bool,
    out: Box<dyn Write>,
    err: Box<dyn Write>,
}

impl Output {
    /// Wraps the two streams with a mode and verbosity flag.
    #[must_use]
    pub fn new(mode: Mode, verbose: bool, out: Box<dyn Write>, err: Box<dyn Write>) -> Self {
        Self {
            mode,
            verbose,
            out,
            err,
        }
    }

    /// The active output mode.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// True when machine JSON envelopes are expected on stdout.
    #[must_use]
    pub fn is_json(&self) -> bool {
        self.mode == Mode::Json
    }

    /// True when only essential identifiers should be printed.
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.mode == Mode::Quiet
    }

    /// True when JSON Lines (one JSON object per line) should be emitted.
    #[must_use]
    pub fn is_jsonl(&self) -> bool {
        self.mode == Mode::JsonLines
    }

    /// True when TSV (tab-separated values) should be emitted.
    #[must_use]
    pub fn is_tsv(&self) -> bool {
        self.mode == Mode::Tsv
    }

    /// Writes a pretty-printed JSON value to stdout.
    pub fn json(&mut self, value: &Value) -> io::Result<()> {
        let rendered = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
        writeln!(self.out, "{rendered}")
    }

    /// Writes a human content line to stdout.
    ///
    /// Server-influenced text is filtered by [`sanitize_for_terminal`] first so
    /// resource content cannot smuggle terminal control sequences into the
    /// user's session.
    pub fn line(&mut self, text: &str) -> io::Result<()> {
        writeln!(self.out, "{}", sanitize_for_terminal(text))
    }

    /// Writes raw text directly to stdout (used for completion scripts).
    pub fn raw(&mut self, text: &str) -> io::Result<()> {
        write!(self.out, "{text}")
    }

    /// Prints nothing when quiet, otherwise a human line to stdout.
    pub fn human(&mut self, text: &str) -> io::Result<()> {
        if self.mode == Mode::Quiet {
            Ok(())
        } else {
            self.line(text)
        }
    }

    /// Writes a table to stdout (human mode).
    pub fn table(&mut self, headers: &[&str], rows: &[Vec<String>]) -> io::Result<()> {
        let rendered = render_table(headers, rows);
        if rendered.is_empty() {
            Ok(())
        } else {
            write!(self.out, "{rendered}")
        }
    }

    /// Writes table rows without a header or separator (human mode).
    fn table_body(&mut self, rows: &[Vec<String>]) -> io::Result<()> {
        let rendered = render_table_body(rows);
        if rendered.is_empty() {
            Ok(())
        } else {
            write!(self.out, "{rendered}")
        }
    }

    /// Streams the command's collection as one JSON Lines record per line.
    ///
    /// Each line is one complete raw API resource (field names and values are
    /// exactly what the server sent). The `page` envelope, banners, separators,
    /// and diagnostics never reach stdout, so the stream stays valid when read
    /// line by line; pagination metadata is available in `--json` output.
    pub fn emit_jsonl(&mut self, json_value: &Value) -> io::Result<()> {
        match collection_items(json_value) {
            Some(items) => {
                for item in items {
                    self.jsonl_record(item)?;
                }
            }
            None => self.jsonl_record(json_value)?,
        }
        Ok(())
    }

    /// Emits pre-computed records — the `--jq` outputs — one complete JSON
    /// value per line, which is the standard jq streaming semantics.
    pub fn jsonl_records(&mut self, records: &[Value]) -> io::Result<()> {
        for record in records {
            self.jsonl_record(record)?;
        }
        Ok(())
    }

    /// Writes one compact JSON value plus a newline (never a pretty envelope).
    fn jsonl_record(&mut self, value: &Value) -> io::Result<()> {
        let rendered = serde_json::to_string(value).map_err(io::Error::other)?;
        writeln!(self.out, "{rendered}")
    }

    /// Emits TSV: an optional header row, then one line per record.
    ///
    /// Column order is the command's documented table order, or the order given
    /// by `--columns`. Every cell is escaped so a record is always exactly one
    /// line: `\`, tab, LF, and CR are backslash-escaped, any other control
    /// character becomes U+FFFD, and terminal color swatches are stripped
    /// because machine output must never contain escape sequences. An empty
    /// field means the value was null or absent.
    pub fn emit_tsv(
        &mut self,
        rows: &[Vec<String>],
        headers: &[&str],
        options: &OutputOptions,
    ) -> io::Result<()> {
        let indices = resolve_columns(&options.columns, headers)?;
        if !options.no_header {
            let cells: Vec<String> = indices
                .iter()
                .filter_map(|&i| headers.get(i))
                .map(|header| tsv_cell(header).into_owned())
                .collect();
            if !cells.is_empty() {
                writeln!(self.out, "{}", cells.join("\t"))?;
            }
        }
        for row in rows {
            let cells: Vec<String> = indices
                .iter()
                .filter_map(|&i| row.get(i))
                .map(|cell| tsv_cell(cell).into_owned())
                .collect();
            writeln!(self.out, "{}", cells.join("\t"))?;
        }
        Ok(())
    }

    /// Emits jq results as TSV, one filter result per row.
    ///
    /// Array results become multiple cells; every other JSON value becomes a
    /// single cell. Objects and nested arrays use compact JSON in that cell.
    /// jq defines the result shape, so this form has no generated header.
    fn emit_jq_tsv(&mut self, records: &[Value]) -> io::Result<()> {
        for record in records {
            let values = match record {
                Value::Array(values) => values.as_slice(),
                value => std::slice::from_ref(value),
            };
            let cells = values
                .iter()
                .map(json_tsv_cell)
                .collect::<io::Result<Vec<_>>>()?;
            writeln!(self.out, "{}", cells.join("\t"))?;
        }
        Ok(())
    }

    /// Render a list collection according to the active mode and `options`.
    ///
    /// `jq` holds the outputs of an applied `--jq` filter (see
    /// [`crate::output::jq_outputs`]); it is `None` when no filter was given and
    /// is only reachable in the machine modes that accept `--jq`.
    ///
    /// * Human     — padded table, `--columns` projection, no header with
    ///   `--no-header`.
    /// * Json      — the full envelope (or the filtered value) verbatim.
    /// * Quiet     — first column identifier per row, sanitized like every other
    ///   human line.
    /// * JsonLines — raw resources (or jq outputs), one per line.
    /// * Tsv       — tab-separated table projection.
    pub fn render_list(
        &mut self,
        json_value: &Value,
        jq: Option<&[Value]>,
        headers: &[&str],
        rows: &[Vec<String>],
        options: &OutputOptions,
    ) -> io::Result<()> {
        match self.mode {
            Mode::Human => {
                let (filtered_headers, filtered_rows) =
                    apply_column_options(headers, rows, options)?;
                if options.no_header {
                    self.table_body(&filtered_rows)
                } else {
                    self.table(&filtered_headers, &filtered_rows)
                }
            }
            Mode::Json => match jq {
                Some(outputs) => self.json(&jq_single_value(outputs)),
                None => self.json(json_value),
            },
            Mode::Quiet => {
                for row in rows {
                    if let Some(cell) = row.first() {
                        self.line(cell)?;
                    }
                }
                Ok(())
            }
            Mode::JsonLines => match jq {
                Some(outputs) => self.jsonl_records(outputs),
                None => self.emit_jsonl(json_value),
            },
            Mode::Tsv => match jq {
                Some(outputs) => self.emit_jq_tsv(outputs),
                None => self.emit_tsv(rows, headers, options),
            },
        }
    }
}

/// The collection array inside a list envelope, if the value holds one.
///
/// `items` wins because it is the documented collection key; otherwise a single
/// array-valued member is used, so `--jsonl` streams records rather than one
/// opaque object. Anything else is emitted as a single record.
fn collection_items(value: &Value) -> Option<&Vec<Value>> {
    if let Some(items) = value.as_array() {
        return Some(items);
    }
    let object = value.as_object()?;
    if let Some(items) = object.get("items").and_then(Value::as_array) {
        return Some(items);
    }
    let mut arrays = object.iter().filter_map(|(_k, v)| v.as_array());
    let first = arrays.next()?;
    if arrays.next().is_some() {
        // Ambiguous: two or more collections, so emit the object itself.
        return None;
    }
    Some(first)
}

/// Escapes one TSV cell (see [`Output::emit_tsv`]).
fn tsv_cell(text: &str) -> Cow<'_, str> {
    let stripped = strip_ansi(text);
    if !stripped.contains(['\\', '\t', '\n', '\r']) && !stripped.chars().any(|c| c.is_control()) {
        return stripped;
    }
    let mut out = String::with_capacity(stripped.len() + 8);
    for c in stripped.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Converts a JSON value into one escaped TSV cell.
fn json_tsv_cell(value: &Value) -> io::Result<String> {
    let text = match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        value => serde_json::to_string(value).map_err(io::Error::other)?,
    };
    Ok(tsv_cell(&text).into_owned())
}

/// Removes terminal escape sequences so machine output is never colorized.
fn strip_ansi(text: &str) -> Cow<'_, str> {
    if !text.contains('\u{1b}') {
        return Cow::Borrowed(text);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c != '\u{1b}' {
            out.push(c);
            index += 1;
            continue;
        }

        match chars.get(index + 1) {
            // CSI: skip through its final byte (0x40..=0x7e).
            Some('[') => {
                let mut end = index + 2;
                while end < chars.len() && !matches!(chars[end] as u32, 0x40..=0x7e) {
                    end += 1;
                }
                index = (end + 1).min(chars.len());
            }
            // OSC and related string sequences: skip through BEL or ST.
            Some(']' | 'P' | 'X' | '^' | '_') => {
                let mut end = index + 2;
                while end < chars.len() {
                    if chars[end] == '\u{7}' {
                        end += 1;
                        break;
                    }
                    if chars[end] == '\u{1b}' && end + 1 < chars.len() && chars[end + 1] == '\\' {
                        end += 2;
                        break;
                    }
                    end += 1;
                }
                index = end;
            }
            // Drop any other two-character escape, or a lone ESC.
            Some(_) => index += 2,
            None => index += 1,
        }
    }
    Cow::Owned(out)
}

/// Resolves requested column names against available headers.
///
/// Returns indices into the full `headers` slice in the order requested.
/// Errors when any requested name is absent so callers can surface a precise
/// message before any output is written.
pub fn validate_columns(
    requested: &Option<Vec<String>>,
    available: &[&str],
) -> Result<Vec<usize>, String> {
    let Some(names) = requested else {
        return Ok(all_columns(available));
    };
    let mut indices = Vec::with_capacity(names.len());
    for name in names {
        let idx = available
            .iter()
            .position(|h| h.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                let available = available.join(", ");
                format!("unknown column {name:?}; available: {available}")
            })?;
        indices.push(idx);
    }
    Ok(indices)
}

/// Every column index, in the command's documented order.
fn all_columns(headers: &[&str]) -> Vec<usize> {
    (0..headers.len()).collect()
}

fn resolve_columns(requested: &Option<Vec<String>>, headers: &[&str]) -> io::Result<Vec<usize>> {
    validate_columns(requested, headers).map_err(io::Error::other)
}

fn apply_column_options<'a>(
    headers: &'a [&str],
    rows: &[Vec<String>],
    options: &OutputOptions,
) -> io::Result<(Vec<&'a str>, Vec<Vec<String>>)> {
    let indices = resolve_columns(&options.columns, headers)?;
    let filtered_headers: Vec<&str> = indices
        .iter()
        .filter_map(|&i| headers.get(i))
        .copied()
        .collect();
    let filtered_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            indices
                .iter()
                .filter_map(|&i| row.get(i).cloned())
                .collect()
        })
        .collect();
    Ok((filtered_headers, filtered_rows))
}

/// Applies a jq filter expression and returns each value it emits.
///
/// The filter is compiled once per invocation; parse errors are reported with
/// source location so the caller can surface an actionable message. Runtime
/// errors are returned as well; a failed filter must never silently produce a
/// partial result.
pub fn jq_outputs(value: &Value, filter_expr: &str) -> Result<Vec<Value>, String> {
    use jaq_all::{data, load};

    let filter = jaq_all::compile_with(filter_expr, jaq_all::defs(), data::base_funs(), &[])
        .map_err(|reports| {
            reports
                .iter()
                .map(|r| format!("{}", load::FileReportsDisp::new(r)))
                .collect::<Vec<_>>()
                .join("; ")
        })?;

    // Convert serde_json::Value -> jaq_json::Val for filter execution.
    let val_input = value_to_jaq(value);
    let mut results = Vec::new();
    let inputs: std::iter::Once<Result<_, String>> = std::iter::once(Ok(val_input));
    let runner = data::Runner::default();
    let vars = Default::default();
    data::run(
        &runner,
        &filter,
        vars,
        inputs,
        |error| error,
        |v| {
            let val = unwrap_valr(v).map_err(|error| error.to_string())?;
            let json = jaq_val_to_json(&val);
            results.push(json);
            Ok(())
        },
    )?;
    Ok(results)
}

/// Coalesces jq's output stream into one valid JSON document.
fn jq_single_value(results: &[Value]) -> Value {
    match results.len() {
        0 => Value::Null,
        1 => results[0].clone(),
        _ => Value::Array(results.to_vec()),
    }
}

fn value_to_jaq(value: &Value) -> Val {
    match value {
        Value::Null => Val::Null,
        Value::Bool(b) => Val::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i128() {
                // jaq's inline integer is pointer-sized. Preserve larger
                // serde integers as decimal numbers, including on 32-bit
                // platforms, instead of truncating through an `as` cast.
                if let Ok(int) = isize::try_from(i) {
                    Val::Num(json::Num::Int(int))
                } else {
                    Val::Num(
                        json::Num::from_str_radix(&n.to_string(), 10)
                            .unwrap_or_else(|| json::Num::Dec(Rc::new(n.to_string()))),
                    )
                }
            } else if let Some(f) = n.as_f64() {
                Val::Num(json::Num::Float(f))
            } else {
                Val::Num(json::Num::Dec(Rc::new(n.to_string())))
            }
        }
        Value::String(s) => Val::TStr(Box::new(Bytes::from(s.clone()))),
        Value::Array(arr) => Val::Arr(Rc::new(arr.iter().map(value_to_jaq).collect())),
        Value::Object(obj) => {
            let map: Map<Val, Val> = obj
                .iter()
                .map(|(k, v)| (Val::TStr(Box::new(Bytes::from(k.clone()))), value_to_jaq(v)))
                .collect();
            Val::Obj(Rc::new(map))
        }
    }
}

#[allow(clippy::needless_borrow)]
fn jaq_val_to_json(val: &Val) -> Value {
    match val {
        Val::Null => Value::Null,
        Val::Bool(b) => Value::Bool(*b),
        Val::Num(n) => {
            // Prefer Number::from_i128 for integer values to preserve exactness.
            if let Some(i) = n.as_isize() {
                Value::Number(match Number::from_i128(i as i128) {
                    Some(n) => n,
                    None => unreachable!("isize always fits in i128"),
                })
            } else {
                // Use Display string and parse; fall back to a string representation
                // for very large numbers or exotic formats.
                let s = n.to_string();
                match serde_json::from_str::<Number>(&s) {
                    Ok(num) => Value::Number(num),
                    Err(_) => Value::String(s),
                }
            }
        }
        Val::BStr(b) => {
            // Represent byte strings as UTF-8 strings with lossy replacement.
            Value::String(String::from_utf8_lossy(&b).into_owned())
        }
        Val::TStr(b) => {
            // Represent text strings as UTF-8, preserving replacement characters.
            Value::String(String::from_utf8_lossy(&b).into_owned())
        }
        Val::Arr(arr) => Value::Array(
            rc_unwrap_or_clone(arr)
                .iter()
                .map(jaq_val_to_json)
                .collect(),
        ),
        Val::Obj(obj) => Value::Object(
            rc_unwrap_or_clone(obj)
                .into_iter()
                .map(|(k, v)| {
                    let key = match k {
                        Val::TStr(b) => String::from_utf8_lossy(&b).into_owned(),
                        Val::BStr(b) => String::from_utf8_lossy(&b).into_owned(),
                        _ => k.to_string(),
                    };
                    (key, jaq_val_to_json(&v))
                })
                .collect(),
        ),
    }
}

fn rc_unwrap_or_clone<T: Clone>(rc: &Rc<T>) -> T {
    match Rc::try_unwrap(rc.clone()) {
        Ok(v) => v,
        Err(r) => (*r).clone(),
    }
}

impl Output {
    /// Diagnostics to stderr, only when verbose.
    pub fn verbose(&mut self, text: &str) {
        if self.verbose {
            let _ = writeln!(self.err, "[verbose] {text}");
        }
    }

    /// Warnings/progress to stderr.
    ///
    /// Filtered like [`Output::line`] because diagnostics can quote server
    /// text through callers.
    pub fn warn(&mut self, text: &str) {
        let _ = writeln!(self.err, "{}", sanitize_for_terminal(text));
    }

    /// Renders a failure: JSON envelope to stderr in `--json`, human otherwise.
    pub fn error(&mut self, error: &CliError) -> io::Result<()> {
        if self.is_json() {
            let rendered =
                serde_json::to_string_pretty(&error.to_json()).map_err(io::Error::other)?;
            writeln!(self.err, "{rendered}")
        } else {
            writeln!(self.err, "error [{}]: {}", error.code, error.message)?;
            for (field, messages) in error.field_errors.iter() {
                for message in messages {
                    writeln!(self.err, "  {field}: {message}")?;
                }
            }
            // Correlation ids are always useful and must not require a second
            // request with --verbose merely to make the failure traceable.
            if let Some(request_id) = &error.request_id {
                writeln!(self.err, "request id: {request_id}")?;
            }
            if self.verbose
                && let Some(status) = error.status
            {
                writeln!(self.err, "http status: {status}")?;
            }
            Ok(())
        }
    }
}

/// Strips terminal control sequences from server-influenced text while
/// preserving the SGR sequences the CLI itself emits (color swatches, doctor
/// status markers).
///
/// Human output embeds server-controlled strings (titles, descriptions,
/// comment bodies, names). Unfiltered, a hostile or merely mischievous author
/// could smuggle OSC/CSI sequences into another user's terminal (clipboard
/// writes, window-title changes, cursor movement). Only well-formed SGR
/// sequences (`ESC [ digits/semicolons m`) survive; every other escape
/// sequence and control character is removed (tab and newline are kept, since
/// detail views render multi-line descriptions). JSON output is untouched:
/// `serde_json` escapes control characters itself.
#[must_use]
pub fn sanitize_for_terminal(input: &str) -> Cow<'_, str> {
    let needs_filter = input
        .chars()
        .any(|c| c == '\u{1b}' || (c.is_control() && c != '\t' && c != '\n'));
    if !needs_filter {
        return Cow::Borrowed(input);
    }

    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if c != '\u{1b}' {
            if c == '\t' || c == '\n' || !c.is_control() {
                out.push(c);
            }
            index += 1;
            continue;
        }

        match chars.get(index + 1) {
            // CSI: preserve only SGR (`...m`); drop every other final byte.
            Some('[') => {
                let mut end = index + 2;
                while end < chars.len() && matches!(chars[end] as u32, 0x20..=0x3F) {
                    end += 1;
                }
                if end < chars.len() && chars[end] == 'm' {
                    out.extend(chars[index..=end].iter());
                    index = end + 1;
                } else if end < chars.len() && matches!(chars[end] as u32, 0x40..=0x7E) {
                    index = end + 1;
                } else {
                    index += 2;
                }
            }
            // OSC and other string-introduced sequences: skip to BEL or ST.
            Some(']' | 'P' | 'X' | '^' | '_') => {
                let mut end = index + 2;
                while end < chars.len() {
                    if chars[end] == '\u{7}' {
                        end += 1;
                        break;
                    }
                    if chars[end] == '\u{1b}' && end + 1 < chars.len() && chars[end + 1] == '\\' {
                        end += 2;
                        break;
                    }
                    end += 1;
                }
                index = end;
            }
            // A lone ESC (or ESC followed by an unknown byte) is dropped.
            _ => index += 1,
        }
    }
    Cow::Owned(out)
}

/// Renders a left-aligned, width-padded text table.
///
/// Cells are passed through [`sanitize_for_terminal`] before measuring, so
/// server content cannot inject terminal controls and our own SGR swatches
/// survive unchanged. Column widths are derived from the widest cell by
/// *visible* width, so cells may contain ANSI SGR sequences without breaking
/// alignment. Empty input produces an empty string so callers can suppress the
/// whole table.
#[must_use]
pub fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    if headers.is_empty() {
        return String::new();
    }
    let headers: Vec<String> = headers
        .iter()
        .map(|header| sanitize_for_terminal(header).into_owned())
        .collect();
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| sanitize_for_terminal(cell).into_owned())
                .collect()
        })
        .collect();

    let columns = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| visible_width(h)).collect();
    for row in &rows {
        for (index, cell) in row.iter().enumerate().take(columns) {
            widths[index] = widths[index].max(visible_width(cell));
        }
    }

    let mut out = String::new();
    let header_cells: Vec<&str> = headers.iter().map(String::as_str).collect();
    write_row(&mut out, &header_cells, &widths);
    let separator: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    let separator: Vec<&str> = separator.iter().map(String::as_str).collect();
    write_row(&mut out, &separator, &widths);
    for row in &rows {
        let cells: Vec<&str> = (0..columns)
            .map(|index| row.get(index).map(String::as_str).unwrap_or(""))
            .collect();
        write_row(&mut out, &cells, &widths);
    }
    out
}

/// Renders only table data rows, omitting the header and separator.
#[must_use]
pub fn render_table_body(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return String::new();
    }

    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| sanitize_for_terminal(cell).into_owned())
                .collect()
        })
        .collect();
    let mut widths = vec![0; columns];
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(visible_width(cell));
        }
    }

    let mut out = String::new();
    for row in &rows {
        let cells: Vec<&str> = (0..columns)
            .map(|index| row.get(index).map(String::as_str).unwrap_or(""))
            .collect();
        write_row(&mut out, &cells, &widths);
    }
    out
}

/// The number of display columns a cell occupies, skipping ANSI SGR escapes.
///
/// Block-element swatch fills (U+2588) render as one terminal column each in
/// every terminal the CLI targets, so a plain `char` count outside escape
/// sequences is the width.
#[must_use]
pub fn visible_width(text: &str) -> usize {
    let mut count = 0;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip an entire CSI sequence (ESC [ ... final byte).
            if chars.next() == Some('[') {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            count += 1;
        }
    }
    count
}

fn write_row(out: &mut String, cells: &[&str], widths: &[usize]) {
    let mut line = String::new();
    let last = cells.len().saturating_sub(1);
    for (index, cell) in cells.iter().enumerate() {
        line.push_str(cell);
        if index != last {
            let pad = widths[index].saturating_sub(visible_width(cell)) + 2;
            line.push_str(&" ".repeat(pad));
        }
    }
    out.push_str(line.trim_end());
    out.push('\n');
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::exit;

    // A cheap shared in-memory writer for capturing output in tests.
    #[derive(Clone, Default)]
    struct Sink(Rc<RefCell<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn text(sink: &Sink) -> String {
        String::from_utf8(sink.0.borrow().clone()).unwrap()
    }

    #[test]
    fn sanitize_for_terminal_keeps_sgr_and_strips_escapes() {
        assert_eq!(sanitize_for_terminal("plain text"), "plain text");
        assert_eq!(
            sanitize_for_terminal("a\x1b[31mb\x1b[0m"),
            "a\x1b[31mb\x1b[0m"
        );
        assert_eq!(sanitize_for_terminal("a\x1b[2Jb"), "ab");
        assert_eq!(sanitize_for_terminal("a\x1b]0;pwned\x07b"), "ab");
        assert_eq!(
            sanitize_for_terminal("a\x1b]52;c;cGF5bG9hZA==\x1b\\b"),
            "ab"
        );
        assert_eq!(sanitize_for_terminal("a\rb\u{7}c\u{0}d"), "abcd");
        assert_eq!(
            sanitize_for_terminal("keep\ttabs\nand newlines"),
            "keep\ttabs\nand newlines"
        );
        assert_eq!(sanitize_for_terminal("\x1b"), "");
    }

    #[test]
    fn line_filters_terminal_escape_sequences() {
        let sink = Sink::default();
        let mut out = Output::new(
            Mode::Human,
            false,
            Box::new(sink.clone()),
            Box::new(Sink::default()),
        );
        out.line("ok \x1b[2Jdone").unwrap();
        assert_eq!(text(&sink), "ok done\n");
    }

    #[test]
    fn table_filters_terminal_escape_sequences_in_cells() {
        let rendered = render_table(&["KEY"], &[vec!["\x1b]0;title\x07HAM-1".to_string()]]);
        assert!(!rendered.contains('\u{7}'));
        assert!(!rendered.contains("title"));
        assert!(rendered.contains("HAM-1"));
    }

    #[test]
    fn renders_padded_table() {
        let out = render_table(
            &["KEY", "TITLE"],
            &[
                vec!["HAM-1".to_string(), "Short".to_string()],
                vec!["HAM-10".into(), "A longer title".into()],
            ],
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "KEY     TITLE");
        assert_eq!(lines[1], "------  --------------");
        assert_eq!(lines[2], "HAM-1   Short");
        assert_eq!(lines[3], "HAM-10  A longer title");
    }

    #[test]
    fn empty_headers_render_empty() {
        assert_eq!(render_table(&[], &[vec!["x".to_string()]]), "");
    }

    #[test]
    fn json_output_captured() {
        let sink = Sink::default();
        let mut out = Output::new(
            Mode::Json,
            false,
            Box::new(sink.clone()),
            Box::new(Sink::default()),
        );
        out.json(&serde_json::json!({"a": 1})).unwrap();
        assert_eq!(text(&sink).trim(), "{\n  \"a\": 1\n}");
    }

    #[test]
    fn quiet_suppresses_human_lines() {
        let sink = Sink::default();
        let mut out = Output::new(
            Mode::Quiet,
            false,
            Box::new(sink.clone()),
            Box::new(Sink::default()),
        );
        out.human("hidden").unwrap();
        assert!(text(&sink).is_empty());
    }

    #[test]
    fn error_json_envelope_written_to_stderr() {
        let err_sink = Sink::default();
        let mut out = Output::new(
            Mode::Json,
            false,
            Box::new(Sink::default()),
            Box::new(err_sink.clone()),
        );
        let err = CliError::from_api(hamstik_api_client::error::ApiError {
            status: 412,
            code: "REVISION_CONFLICT".into(),
            message: "changed".into(),
            request_id: Some("r1".into()),
            field_errors: Default::default(),
            details: None,
            retry_after: None,
            rate_limit: None,
        });
        out.error(&err).unwrap();
        let rendered: Value = serde_json::from_str(&text(&err_sink)).unwrap();
        assert_eq!(rendered["error"]["code"], "REVISION_CONFLICT");
        assert_eq!(err.exit_code(), exit::CONFLICT);
    }

    #[test]
    fn verbose_writes_only_when_enabled() {
        let err_sink = Sink::default();
        let mut out = Output::new(
            Mode::Human,
            true,
            Box::new(Sink::default()),
            Box::new(err_sink.clone()),
        );
        out.verbose("note");
        assert!(text(&err_sink).contains("note"));
    }

    #[test]
    fn structured_modes_are_identified() {
        assert!(!Mode::Human.is_structured());
        assert!(!Mode::Quiet.is_structured());
        assert!(Mode::Json.is_structured());
        assert!(Mode::JsonLines.is_structured());
        assert!(Mode::Tsv.is_structured());
    }

    #[test]
    fn jsonl_streams_raw_collection_items() {
        let sink = Sink::default();
        let mut out = Output::new(
            Mode::JsonLines,
            false,
            Box::new(sink.clone()),
            Box::new(Sink::default()),
        );
        out.emit_jsonl(&serde_json::json!({
            "items": [{"key": "HAM-1", "points": 3}, {"key": "HAM-2"}],
            "page": {"hasMore": false}
        }))
        .unwrap();
        assert_eq!(
            text(&sink),
            "{\"key\":\"HAM-1\",\"points\":3}\n{\"key\":\"HAM-2\"}\n"
        );
    }

    #[test]
    fn jq_preserves_multiple_outputs_and_reports_runtime_errors() {
        let value = serde_json::json!({"items": [{"key": "HAM-1"}, {"key": "HAM-2"}]});
        assert_eq!(
            jq_outputs(&value, ".items[].key").unwrap(),
            vec![Value::String("HAM-1".into()), Value::String("HAM-2".into())]
        );
        assert!(jq_outputs(&value, "error(\"boom\")").is_err());
    }

    #[test]
    fn tsv_projects_and_escapes_cells() {
        let sink = Sink::default();
        let mut out = Output::new(
            Mode::Tsv,
            false,
            Box::new(sink.clone()),
            Box::new(Sink::default()),
        );
        let options = OutputOptions {
            columns: Some(vec!["title".into(), "key".into()]),
            no_header: true,
        };
        out.emit_tsv(
            &[vec!["HAM-1".into(), "line one\nline two\tend".into()]],
            &["KEY", "TITLE"],
            &options,
        )
        .unwrap();
        assert_eq!(text(&sink), "line one\\nline two\\tend\tHAM-1\n");
    }

    #[test]
    fn tsv_strips_complete_terminal_escape_sequences() {
        assert_eq!(
            tsv_cell("before\x1b[31mred\x1b[0m\x1b[2~after"),
            "beforeredafter"
        );
        assert_eq!(tsv_cell("a\x1b]0;title\x1b\\b"), "ab");
    }

    #[test]
    fn jq_arrays_render_as_tsv_rows() {
        let sink = Sink::default();
        let mut out = Output::new(
            Mode::Tsv,
            false,
            Box::new(sink.clone()),
            Box::new(Sink::default()),
        );
        out.emit_jq_tsv(&[
            serde_json::json!(["HAM-1", 3]),
            serde_json::json!(["HAM-2", null]),
        ])
        .unwrap();
        assert_eq!(text(&sink), "HAM-1\t3\nHAM-2\t\n");
    }

    #[test]
    fn headerless_human_table_keeps_aligned_rows_only() {
        assert_eq!(
            render_table_body(&[
                vec!["HAM-1".into(), "Short".into()],
                vec!["HAM-20".into(), "Longer".into()],
            ]),
            "HAM-1   Short\nHAM-20  Longer\n"
        );
    }
}
