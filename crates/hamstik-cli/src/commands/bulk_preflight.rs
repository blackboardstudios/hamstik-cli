// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Bulk operation preflight.
//!
//! Validates a bulk operations file against the checked-in Public API
//! contract — JSON syntax, envelope shape, per-operation required fields,
//! enum spellings, revision constraints, and operation-count limits — before
//! any HTTP request is attempted. Diagnostics identify the failing operation
//! index and field path. Preflight never reproduces server business rules:
//! passing preflight says nothing about authorization, transition legality,
//! or Organization isolation.

use serde_json::{Value, json};

use crate::error::CliError;

/// The documented bulk operations limit (`minItems: 1, maxItems: 50`).
pub(crate) const MAX_BULK_OPERATIONS: usize = 50;

/// One preflight finding: which operation (array index) and which field
/// failed, with an actionable message.
pub(crate) struct Finding {
    /// Zero-based operation index within the operations array.
    pub index: Option<usize>,
    /// JSON pointer-style field path relative to the operation object.
    pub field: Option<String>,
    /// The actionable description.
    pub message: String,
}

impl Finding {
    fn operation(index: usize, field: Option<&str>, message: impl Into<String>) -> Self {
        Self {
            index: Some(index),
            field: field.map(str::to_string),
            message: message.into(),
        }
    }

    /// The human/JSON location label (empty for envelope-level findings).
    fn location(&self) -> String {
        match (self.index, &self.field) {
            (Some(index), Some(field)) => format!("operations[{index}].{field}"),
            (Some(index), None) => format!("operations[{index}]"),
            (None, Some(field)) => field.clone(),
            (None, None) => String::new(),
        }
    }
}

/// What must be true of every operation in the file.
struct OperationRules {
    /// Required per-operation fields.
    required: &'static [&'static str],
    /// Optional per-operation fields that must be positive integers when present.
    positive_integer_optional: &'static [&'static str],
}

/// Per-kind bulk operation rules derived from the OpenAPI schemas
/// (`BulkCreateWorkItemOperation`, `BulkUpdateWorkItemOperation`,
/// `BulkTransitionWorkItemOperation`: `additionalProperties: false`).
const CREATE_RULES: OperationRules = OperationRules {
    required: &["projectKey", "title"],
    positive_integer_optional: &[],
};
const UPDATE_RULES: OperationRules = OperationRules {
    required: &["projectKey", "workItemKey", "changes"],
    positive_integer_optional: &["revision"],
};
const TRANSITION_RULES: OperationRules = OperationRules {
    required: &["projectKey", "workItemKey", "targetStatus"],
    positive_integer_optional: &["revision"],
};

/// Work Item statuses accepted by the Public API contract.
const WORK_ITEM_STATUSES: [&str; 5] = ["backlog", "todo", "in_progress", "in_review", "done"];

/// Parses and validates a bulk operations file without any HTTP request.
///
/// `source` describes the input for diagnostics (a path or `stdin`); `kind`
/// selects the per-operation schema rules.
pub(crate) fn preflight(
    text: &str,
    source: &str,
    kind: PreflightKind,
) -> Result<Vec<Value>, CliError> {
    let parsed: Value = serde_json::from_str(text).map_err(|err| {
        CliError::usage(format!(
            "{source} is not valid JSON: {err} (the operations file must be a JSON array)"
        ))
    })?;
    let Value::Array(items) = &parsed else {
        return Err(CliError::usage(format!(
            "{source} must contain a JSON array of operations, not {}",
            type_name(&parsed)
        )));
    };
    if items.is_empty() {
        return Err(CliError::usage(format!(
            "{source} contains no operations; bulk requests accept between 1 and {MAX_BULK_OPERATIONS}"
        )));
    }
    if items.len() > MAX_BULK_OPERATIONS {
        return Err(CliError::usage(format!(
            "{source} contains {} operations; bulk requests accept at most {MAX_BULK_OPERATIONS}",
            items.len()
        )));
    }

    let rules = match kind {
        PreflightKind::Create => &CREATE_RULES,
        PreflightKind::Update => &UPDATE_RULES,
        PreflightKind::Transition => &TRANSITION_RULES,
    };
    let mut findings: Vec<Finding> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        validate_operation(index, item, rules, &mut findings);
    }
    if !findings.is_empty() {
        return Err(preflight_error(source, &findings));
    }
    Ok(items.clone())
}

/// Which bulk operation kind is being preflighted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreflightKind {
    /// `work bulk create`.
    Create,
    /// `work bulk update`.
    Update,
    /// `work bulk transition`.
    Transition,
}

/// Validates one operation object against the schema-derived rules.
fn validate_operation(
    index: usize,
    item: &Value,
    rules: &OperationRules,
    findings: &mut Vec<Finding>,
) {
    let Value::Object(object) = item else {
        findings.push(Finding::operation(
            index,
            None,
            format!("must be a JSON object, not {}", type_name(item)),
        ));
        return;
    };

    for required in rules.required {
        match object.get(*required) {
            None => findings.push(Finding::operation(
                index,
                Some(required),
                "is required by the Public API bulk schema",
            )),
            Some(Value::Null) => findings.push(Finding::operation(
                index,
                Some(required),
                "must not be null",
            )),
            Some(Value::String(value)) if value.trim().is_empty() => {
                findings.push(Finding::operation(
                    index,
                    Some(required),
                    "must not be empty",
                ));
            }
            _ => {}
        }
    }

    for field in rules.positive_integer_optional {
        match object.get(*field) {
            Some(Value::Null) | None => {}
            Some(value) if value.as_i64().is_some_and(|v| v >= 1) => {}
            Some(_) => findings.push(Finding::operation(
                index,
                Some(field),
                "must be a positive integer (>= 1) when present",
            )),
        }
    }

    // Unknown fields are rejected: the Public API schemas set
    // `additionalProperties: false` and the typed client uses
    // `deny_unknown_fields`, so an unknown field could only fail server-side.
    for key in object.keys() {
        if !rules.required.contains(&key.as_str())
            && !rules.positive_integer_optional.contains(&key.as_str())
        {
            findings.push(Finding::operation(
                index,
                Some(key),
                "is not part of the Public API bulk schema (unknown fields are rejected)",
            ));
        }
    }

    // Enum spellings validated against the documented contract values.
    if let Some(Value::String(target)) = object.get("targetStatus")
        && !WORK_ITEM_STATUSES.contains(&target.as_str())
    {
        findings.push(Finding::operation(
            index,
            Some("targetStatus"),
            format!(
                "{target:?} is not a Work Item status; expected one of: {}",
                WORK_ITEM_STATUSES.join(", ")
            ),
        ));
    }
    if let Some(Value::Object(changes)) = object.get("changes") {
        if changes.is_empty() {
            findings.push(Finding::operation(
                index,
                Some("changes"),
                "must not be empty; specify at least one field to update",
            ));
        }
        if let Some(status) = changes.get("status").and_then(Value::as_str)
            && !WORK_ITEM_STATUSES.contains(&status)
        {
            findings.push(Finding::operation(
                index,
                Some("changes.status"),
                format!(
                    "{status:?} is not a Work Item status; expected one of: {}",
                    WORK_ITEM_STATUSES.join(", ")
                ),
            ));
        }
    }
}

/// Builds the aggregated preflight failure: usage-kind (exit 2), one line per
/// finding, no operation payload echoed back.
fn preflight_error(source: &str, findings: &[Finding]) -> CliError {
    let mut message = format!(
        "{} failed bulk preflight with {} problem{}:",
        source,
        findings.len(),
        if findings.len() == 1 { "" } else { "s" }
    );
    for finding in findings {
        let location = finding.location();
        if location.is_empty() {
            message.push_str(&format!("\n  - {}", finding.message));
        } else {
            message.push_str(&format!("\n  - {location}: {}", finding.message));
        }
    }
    CliError::usage(message)
}

/// The concurrency mode selected for the request, for diagnostics.
pub(crate) fn concurrency_label(mode: Option<&str>) -> Value {
    match mode {
        Some("last-write-wins") => json!({
            "mode": "last-write-wins",
            "revisionRequired": false,
            "note": "revisions are ignored; the server accepts any prior state",
        }),
        _ => json!({
            "mode": "require-revision",
            "revisionRequired": true,
            "note": "every operation must carry a positive revision when it updates a resource",
        }),
    }
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
