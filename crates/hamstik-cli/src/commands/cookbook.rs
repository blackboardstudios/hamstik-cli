// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik commands --cookbook` — copy-pasteable API workflow examples.
//!
//! The cookbook is not hand-maintained here: it is the marker-delimited
//! section of the canonical bundled Agent Skill
//! (`skills/hamstik/SKILL.md`). Deriving it from the skill means the CLI can
//! never print examples the skill does not also teach, and
//! `hamstik agent skill check` validates every referenced command and option
//! against the real command tree. Replace the `<PLACEHOLDER>` tokens; the
//! examples carry safe automation defaults (`--json --no-input`) and explicit
//! Organization/Project context.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::app::Session;
use crate::error::CliError;

use super::agent_skill::CANONICAL_SKILL;
use super::emit_json;

/// Marker opening the marker-delimited cookbook section of the skill.
const COOKBOOK_START: &str = "<!-- cookbook:start -->";
/// Marker closing the marker-delimited cookbook section of the skill.
const COOKBOOK_END: &str = "<!-- cookbook:end -->";

/// The cookbook schema version; bump on layout changes.
pub(crate) const COOKBOOK_VERSION: u64 = 1;

/// One titled group of example commands.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CookbookStep {
    /// The step heading (the text of its `#` comment line).
    pub title: String,
    /// Logical (continuation-joined) command lines.
    pub commands: Vec<String>,
}

/// The parsed cookbook.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Cookbook {
    /// The fenced bash block between the markers, verbatim (fences removed).
    pub raw: &'static str,
    /// The titled steps parsed from the block.
    pub steps: Vec<CookbookStep>,
}

/// Runs `hamstik commands --cookbook`.
pub(crate) fn run(session: &mut Session<'_>) -> Result<(), CliError> {
    let cookbook = extract(CANONICAL_SKILL)?;
    if session.json() {
        let steps: Vec<Value> = cookbook
            .steps
            .iter()
            .map(|step| {
                json!({
                    "title": step.title,
                    "commands": step.commands,
                })
            })
            .collect();
        emit_json(
            session,
            &json!({
                "cookbookVersion": COOKBOOK_VERSION,
                "source": "skills/hamstik/SKILL.md",
                "placeholders": placeholders(&cookbook),
                "steps": steps,
            }),
        )
    } else {
        let mut body = String::from(
            "Hamstik API cookbook\n\
             \n\
             Replace the <PLACEHOLDER> values and run the commands in order.\n\
             Every example keeps the automation-safe defaults (`--json --no-input`)\n\
             and passes `--org`/`--project` explicitly instead of relying on stored\n\
             defaults.\n\
             \n",
        );
        body.push_str(cookbook.raw);
        body.push('\n');
        session.out.raw(&body).map_err(CliError::general)
    }
}

/// Extracts and parses the marker-delimited cookbook from the canonical skill.
pub(crate) fn extract(skill: &'static str) -> Result<Cookbook, CliError> {
    let section = section(skill)?;
    let raw = extract_fenced_bash(section)?;
    let steps = parse_steps(raw);
    if steps.is_empty() {
        return Err(missing());
    }
    Ok(Cookbook { raw, steps })
}

/// The text between the cookbook markers, or a clear error when absent.
fn section(skill: &'static str) -> Result<&'static str, CliError> {
    let start = skill.find(COOKBOOK_START).ok_or_else(missing)?;
    let after_start = &skill[start + COOKBOOK_START.len()..];
    let end = after_start.find(COOKBOOK_END).ok_or_else(missing)?;
    Ok(&after_start[..end])
}

/// The body of the section's single fenced bash block.
fn extract_fenced_bash(section: &'static str) -> Result<&'static str, CliError> {
    let fence = section.find("```").ok_or_else(missing)?;
    let after_open = &section[fence + 3..];
    let body_start = after_open.find('\n').ok_or_else(missing)? + 1;
    let body = &after_open[body_start..];
    let close = body.rfind("\n```").ok_or_else(missing)?;
    Ok(body[..close].trim_end_matches(['\r', '\n']))
}

/// Parses `#`-titled steps and `\`-continued command lines from the block.
fn parse_steps(raw: &str) -> Vec<CookbookStep> {
    let mut steps: Vec<CookbookStep> = Vec::new();
    let mut logical = String::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // A comment is a step heading only when it starts a new logical line;
        // mid-continuation it would be part of the command.
        if logical.is_empty()
            && let Some(title) = trimmed.strip_prefix('#')
        {
            let title = title.trim();
            if !title.is_empty() {
                steps.push(CookbookStep {
                    title: title.to_string(),
                    commands: Vec::new(),
                });
            }
            continue;
        }
        if !logical.is_empty() {
            logical.push(' ');
        }
        let continuation = trimmed.ends_with('\\');
        let part = if continuation {
            trimmed.trim_end_matches('\\').trim_end()
        } else {
            trimmed
        };
        logical.push_str(part);
        if !continuation {
            push_command(&mut steps, std::mem::take(&mut logical));
        }
    }
    if !logical.is_empty() {
        push_command(&mut steps, logical);
    }
    steps
}

/// Appends one command to the current step (creating one when none exists).
fn push_command(steps: &mut Vec<CookbookStep>, command: String) {
    if command.is_empty() {
        return;
    }
    match steps.last_mut() {
        Some(step) => step.commands.push(command),
        None => steps.push(CookbookStep {
            title: String::new(),
            commands: vec![command],
        }),
    }
}

/// The distinct `<PLACEHOLDER>` tokens referenced by the examples, sorted.
fn placeholders(cookbook: &Cookbook) -> Vec<String> {
    let mut found: BTreeSet<String> = BTreeSet::new();
    for step in &cookbook.steps {
        for command in &step.commands {
            for token in command.split_whitespace() {
                let token =
                    token.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | '(' | ')'));
                if token.len() > 2 && token.starts_with('<') && token.ends_with('>') {
                    found.insert(token.to_string());
                }
            }
        }
    }
    found.into_iter().collect()
}

/// The error returned when the bundled skill loses its cookbook markers.
fn missing() -> CliError {
    CliError::general(
        "the bundled Agent Skill is missing its `<!-- cookbook:start -->` cookbook section",
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn canonical_skill_cookbook_parses_into_titled_steps() {
        let cookbook = extract(CANONICAL_SKILL).expect("bundled cookbook");
        assert!(cookbook.raw.contains("work view <ITEM-KEY>"));
        assert!(!cookbook.raw.contains("```"));
        assert_eq!(cookbook.steps.len(), 6);
        for step in &cookbook.steps {
            assert!(!step.title.is_empty());
            assert!(!step.commands.is_empty());
        }
    }

    #[test]
    fn placeholders_are_deduplicated_and_sorted() {
        let cookbook = extract(CANONICAL_SKILL).expect("bundled cookbook");
        let placeholders = placeholders(&cookbook);
        assert_eq!(
            placeholders,
            vec![
                "<COMMENT.md>".to_string(),
                "<ITEM-KEY>".to_string(),
                "<KEY>".to_string(),
                "<ORG>".to_string(),
            ]
        );
    }

    #[test]
    fn missing_markers_are_an_error() {
        let error = extract("---\n# no cookbook here\n").unwrap_err();
        assert!(error.to_string().contains("cookbook"), "{error}");
    }
}
