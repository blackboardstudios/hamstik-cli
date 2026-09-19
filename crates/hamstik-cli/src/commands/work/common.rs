// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Shared `work` helpers: idempotency keys and long-text sources.

use hamstik_api_client::{generate_key, validate_key};

use crate::app::Session;
use crate::error::CliError;
use crate::input::resolve_text;
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
pub(super) fn assignee_fields(value: &Option<String>) -> (Option<String>, Option<String>) {
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
