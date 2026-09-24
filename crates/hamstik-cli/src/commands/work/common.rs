// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Shared `work` helpers: idempotency keys and long-text sources.

use hamstik_api_client::{WorkItemAttributeChange, generate_key, validate_key};

use crate::app::Session;
use crate::error::CliError;
use crate::input::resolve_text;

/// Converts the typed Attribute flags into the sparse Public API change list.
/// Repeated option flags for the same key form a multi-select value; boolean
/// and clear forms stay distinct, including `false` versus omission.
pub(super) fn attribute_changes(
    option_values: &[String],
    boolean_values: &[String],
    clear_keys: &[String],
) -> Result<Option<Vec<WorkItemAttributeChange>>, CliError> {
    let mut changes: Vec<WorkItemAttributeChange> = Vec::new();
    let mut option_groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut used_keys = std::collections::BTreeSet::new();

    for value in option_values {
        let (key, option) = split_attribute_pair(value, "--attribute-option")?;
        if key.is_empty() || option.is_empty() {
            return Err(CliError::usage(
                "--attribute-option expects ATTRIBUTE_KEY=OPTION_KEY",
            ));
        }
        let group = if let Some(index) = option_groups.iter().position(|(known, _)| known == key) {
            index
        } else {
            if !used_keys.insert(key.to_string()) {
                return Err(attribute_key_conflict(key));
            }
            option_groups.push((key.to_string(), Vec::new()));
            option_groups.len() - 1
        };
        let values = &mut option_groups[group].1;
        if values.iter().any(|known| known == option) {
            return Err(CliError::usage(format!(
                "duplicate option `{option}` for Attribute `{key}`"
            )));
        }
        values.push(option.to_string());
        if values.len() > 10 {
            return Err(CliError::usage(
                "an Attribute assignment accepts at most 10 option keys",
            ));
        }
    }

    for (key, option_keys) in option_groups {
        changes.push(WorkItemAttributeChange {
            key,
            clear: None,
            boolean_value: None,
            option_keys: Some(option_keys),
        });
    }

    for value in boolean_values {
        let (key, raw) = split_attribute_pair(value, "--attribute-boolean")?;
        if key.is_empty() {
            return Err(CliError::usage(
                "--attribute-boolean expects ATTRIBUTE_KEY=true|false",
            ));
        }
        let boolean_value = match raw {
            "true" => true,
            "false" => false,
            _ => {
                return Err(CliError::usage(format!(
                    "invalid boolean Attribute value `{raw}` for `{key}`; use true or false"
                )));
            }
        };
        if !used_keys.insert(key.to_string()) {
            return Err(attribute_key_conflict(key));
        }
        changes.push(WorkItemAttributeChange {
            key: key.to_string(),
            clear: None,
            boolean_value: Some(boolean_value),
            option_keys: None,
        });
    }

    for key in clear_keys {
        if key.trim().is_empty() {
            return Err(CliError::usage(
                "--clear-attribute requires a non-empty Attribute key",
            ));
        }
        if !used_keys.insert(key.clone()) {
            return Err(attribute_key_conflict(key));
        }
        changes.push(WorkItemAttributeChange {
            key: key.clone(),
            clear: Some(true),
            boolean_value: None,
            option_keys: None,
        });
    }

    if changes.len() > 25 {
        return Err(CliError::usage(
            "one Work Item request accepts at most 25 Attribute changes",
        ));
    }
    Ok((!changes.is_empty()).then_some(changes))
}

fn split_attribute_pair<'a>(value: &'a str, flag: &str) -> Result<(&'a str, &'a str), CliError> {
    let (key, assigned) = value
        .split_once('=')
        .ok_or_else(|| CliError::usage(format!("{flag} expects KEY=VALUE")))?;
    if assigned.contains('=') {
        return Err(CliError::usage(format!(
            "{flag} expects KEY=VALUE with one `=` separator"
        )));
    }
    Ok((key, assigned))
}

fn attribute_key_conflict(key: &str) -> CliError {
    CliError::usage(format!(
        "Attribute `{key}` was assigned more than once; use repeated --attribute-option values only for a multi-select"
    ))
}
pub(super) fn idem_key(flag: Option<String>) -> Result<String, CliError> {
    match flag {
        Some(key) => {
            validate_key(&key).map_err(|err| CliError::usage(err.to_string()))?;
            Ok(key)
        }
        None => Ok(generate_key()),
    }
}

pub(super) fn idem_key_ref(flag: Option<&str>) -> Result<String, CliError> {
    match flag {
        Some(key) => {
            validate_key(key).map_err(|err| CliError::usage(err.to_string()))?;
            Ok(key.to_string())
        }
        None => Ok(generate_key()),
    }
}

pub(super) fn read_text(
    inline: Option<String>,
    file: Option<&str>,
) -> Result<Option<String>, CliError> {
    let mut stdin = std::io::stdin();
    resolve_text(inline, file, &mut stdin)
        .map_err(|err| CliError::general(format!("cannot read text: {err}")))
}
/// Resolves long-form text from inline, `--*-file`/stdin, or the editor.
///
/// Source conflicts are already rejected at argument-parse time (`conflicts_with`),
/// so at most one source can be present here. `--editor` launches
/// `$VISUAL`/`$EDITOR` on a secure temporary file; the editor's content
/// becomes the value (see [`crate::editor::edit_text`]).
/// Resolves long-form text from inline, `--*-file`/stdin, or the editor.
///
/// Source conflicts are already rejected at argument-parse time (`conflicts_with`),
/// so at most one source can be present here. `--editor` launches
/// `$VISUAL`/`$EDITOR` (then the `editor` config setting) on a secure temporary
/// file; the editor's content
/// becomes the value (see [`crate::editor::edit_text`]).
pub(super) fn read_long_text(
    session: &Session<'_>,
    inline: Option<String>,
    file: Option<&str>,
    editor: bool,
    what: &str,
) -> Result<Option<String>, CliError> {
    if editor {
        return crate::editor::edit_text(
            session.env,
            session.configured_editor()?.as_deref(),
            session.global.no_input,
            what,
        )
        .map(Some);
    }
    read_text(inline, file)
}
/// Resolves the `--assignee` value into the preferred wire form: a `usr_`
/// public ID goes to `assigneePublicId`, everything else to legacy
/// `assigneeId` (UUID, `me`, or `none`).
/// Resolves the `--assignee` value into the preferred wire form: a `usr_`
/// public ID goes to `assigneePublicId`, everything else to legacy
/// `assigneeId` (UUID, `me`, or `none`).
pub(crate) fn assignee_fields(value: &Option<String>) -> (Option<String>, Option<String>) {
    match value {
        Some(id) if id.starts_with("usr_") => (None, Some(id.clone())),
        other => (other.clone(), None),
    }
}
/// True when `value` parses as a UUID (the wire form for label ids).
/// True when `value` parses as a UUID (the wire form for label ids).
pub(super) fn is_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}
