// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `work create --from`/`--template` convenience sources.
//!
//! Both sources resolve to a [`StartPoint`]: an allow-listed subset of fields
//! that seed a new Work Item. Explicit `create` flags always take precedence,
//! and identity/ownership/state fields are never copied. The template parser
//! rejects unknown frontmatter keys outright so an `assignee`, `reporter`, or
//! `status` line can never be silently carried over.
//!
//! The allow-list is `title`, `type`, `priority`, `description`, and
//! `labels`. Labels are not part of the `CreateWorkItemRequest` body (the
//! frozen Public API v1 contract forbids unknown properties), so they are
//! resolved here and attached after the create request succeeds.

use hamstik_api_client::WorkItem;
use serde::Deserialize;

use crate::args::{PriorityArg, TypeArg};
use crate::error::CliError;
use crate::input::{MAX_TEXT_BYTES, read_capped};

/// The allow-listed fields copied from a `--from` Work Item or `--template`
/// file.
#[derive(Debug, Default, Clone)]
pub(super) struct StartPoint {
    /// Copied title, when the source supplied one.
    pub title: Option<String>,
    /// Copied Work Item type (already validated against the documented set).
    pub item_type: Option<String>,
    /// Copied priority (already validated against the documented set).
    pub priority: Option<String>,
    /// Copied description.
    pub description: Option<String>,
    /// Labels to attach after the item is created.
    pub labels: Vec<LabelRef>,
}

/// One label copied from a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LabelRef {
    /// The project label id, when the source exposed one (`--from`).
    pub id: Option<String>,
    /// The label's display name, also the wire selector when `id` is absent.
    pub name: String,
}

impl LabelRef {
    /// The human-readable label identity: the name when known, else the id.
    pub(super) fn display(&self) -> &str {
        if self.name.is_empty() {
            self.id.as_deref().unwrap_or_default()
        } else {
            &self.name
        }
    }
}

/// Resolves an existing Work Item into a [`StartPoint`].
pub(super) fn from_work_item(item: &WorkItem) -> StartPoint {
    StartPoint {
        title: Some(item.title.clone()),
        item_type: Some(item.item_type.clone()),
        priority: Some(item.priority.clone()),
        description: item.description.clone(),
        labels: item
            .labels
            .iter()
            .map(|label| LabelRef {
                id: Some(label.id.clone()),
                name: label.name.clone(),
            })
            .collect(),
    }
}

/// Reads a `--template` file (or `-` for stdin) and resolves its frontmatter.
pub(super) fn from_file(file: &str) -> Result<StartPoint, CliError> {
    let text = if file == "-" {
        let stdin = std::io::stdin();
        read_capped(stdin.lock(), MAX_TEXT_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read template from stdin: {err}")))?
    } else {
        let handle = std::fs::File::open(file)
            .map_err(|err| CliError::usage(format!("cannot open template file {file}: {err}")))?;
        read_capped(handle, MAX_TEXT_BYTES)
            .map_err(|err| CliError::usage(format!("cannot read template file {file}: {err}")))?
    };
    parse(&text).map_err(|message| CliError::usage(format!("invalid template {file}: {message}")))
}

/// The YAML frontmatter shape. Unknown keys are rejected.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frontmatter {
    #[serde(default)]
    title: Option<String>,
    #[serde(default, rename = "type")]
    item_type: Option<String>,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    labels: Option<Vec<String>>,
    #[serde(default)]
    description: Option<String>,
}

/// Parses a Markdown document with YAML frontmatter into a [`StartPoint`].
fn parse(text: &str) -> Result<StartPoint, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (yaml, body) = split_frontmatter(text)?;

    let frontmatter: Frontmatter = if yaml.trim().is_empty() {
        Frontmatter::default()
    } else {
        serde_norway::from_str(yaml)
            .map_err(|err| format!("frontmatter is not valid YAML: {err}"))?
    };

    let item_type = frontmatter
        .item_type
        .as_deref()
        .map(normalize_type)
        .transpose()?;
    let priority = frontmatter
        .priority
        .as_deref()
        .map(normalize_priority)
        .transpose()?;

    let labels = frontmatter.labels.unwrap_or_default();
    if labels.iter().any(|label| label.trim().is_empty()) {
        return Err("labels must be non-empty strings".to_string());
    }
    let labels = labels
        .into_iter()
        .map(|label| {
            if super::common::is_uuid(&label) {
                LabelRef {
                    id: Some(label),
                    name: String::new(),
                }
            } else {
                LabelRef {
                    id: None,
                    name: label,
                }
            }
        })
        .collect();

    let body = body.trim_matches(['\r', '\n']);
    let body_description = if body.trim().is_empty() {
        None
    } else {
        Some(body.to_string())
    };
    let description = match (frontmatter.description, body_description) {
        (Some(_), Some(_)) => {
            return Err(
                "define the description either in the frontmatter or as the Markdown body, \
                 not both"
                    .to_string(),
            );
        }
        (Some(description), None) => Some(description),
        (None, Some(body)) => Some(body),
        (None, None) => None,
    };

    Ok(StartPoint {
        title: frontmatter.title,
        item_type,
        priority,
        description,
        labels,
    })
}

/// Splits a Markdown document into its YAML frontmatter and body.
///
/// Any byte-order mark must already be stripped by the caller; a leading
/// `---` line opens the frontmatter and a later `---` line closes it;
/// everything after the closing line is the body.
pub(super) fn split_frontmatter(text: &str) -> Result<(&str, &str), String> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .ok_or_else(|| "expected a leading `---` YAML frontmatter delimiter".to_string())?;

    let mut offset = 0;
    while offset <= rest.len() {
        let line_end = rest[offset..]
            .find('\n')
            .map_or(rest.len(), |index| offset + index);
        let line = rest[offset..line_end].trim_end_matches('\r');
        if line == "---" {
            let yaml = &rest[..offset];
            let body = if line_end < rest.len() {
                &rest[line_end + 1..]
            } else {
                ""
            };
            return Ok((yaml, body));
        }
        if line_end >= rest.len() {
            break;
        }
        offset = line_end + 1;
    }
    Err("missing closing `---` YAML frontmatter delimiter".to_string())
}

/// Validates a template `type` against the documented choices.
pub(super) fn normalize_type(raw: &str) -> Result<String, String> {
    let value = raw.trim().to_ascii_lowercase();
    TypeArg::ALL
        .iter()
        .copied()
        .find(|kind| kind.as_str() == value)
        .map(|kind| kind.as_str().to_string())
        .ok_or_else(|| {
            let allowed: Vec<&str> = TypeArg::ALL.iter().map(|kind| kind.as_str()).collect();
            format!(
                "unknown type {raw:?}; expected one of {}",
                allowed.join(", ")
            )
        })
}

/// Validates a template `priority` against the documented choices.
pub(super) fn normalize_priority(raw: &str) -> Result<String, String> {
    let value = raw.trim().to_ascii_lowercase();
    PriorityArg::ALL
        .iter()
        .copied()
        .find(|priority| priority.as_str() == value)
        .map(|priority| priority.as_str().to_string())
        .ok_or_else(|| {
            let allowed: Vec<&str> = PriorityArg::ALL
                .iter()
                .map(|priority| priority.as_str())
                .collect();
            format!(
                "unknown priority {raw:?}; expected one of {}",
                allowed.join(", ")
            )
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_body() {
        let template = "---\n\
            title: Bug report\n\
            type: bug\n\
            priority: high\n\
            labels:\n  - regression\n  - backend\n\
            ---\n\
            \n## Steps\n\n1. Do the thing\n";
        let start = parse(template).unwrap();
        assert_eq!(start.title.as_deref(), Some("Bug report"));
        assert_eq!(start.item_type.as_deref(), Some("bug"));
        assert_eq!(start.priority.as_deref(), Some("high"));
        assert_eq!(
            start.labels,
            vec![
                LabelRef {
                    id: None,
                    name: "regression".to_string()
                },
                LabelRef {
                    id: None,
                    name: "backend".to_string()
                },
            ]
        );
        assert_eq!(
            start.description.as_deref(),
            Some("## Steps\n\n1. Do the thing")
        );
    }

    #[test]
    fn accepts_frontmatter_only_description() {
        let template = "---\ntitle: T\ndescription: From frontmatter\n---\n";
        let start = parse(template).unwrap();
        assert_eq!(start.description.as_deref(), Some("From frontmatter"));
    }

    #[test]
    fn rejects_description_in_both_places() {
        let template = "---\ntitle: T\ndescription: Front\n---\nBody\n";
        let err = parse(template).unwrap_err();
        assert!(err.contains("not both"), "{err}");
    }

    #[test]
    fn rejects_unknown_frontmatter_keys() {
        for key in ["assignee", "reporter", "status", "id", "key", "revision"] {
            let template = format!("---\ntitle: T\n{key}: nope\n---\n");
            let err = parse(&template).unwrap_err();
            assert!(
                err.contains("unknown field") || err.contains(key),
                "template with {key} must be rejected, got: {err}"
            );
        }
    }

    #[test]
    fn rejects_unknown_type_and_priority() {
        assert!(parse("---\ntitle: T\ntype: gizmo\n---\n").is_err());
        assert!(parse("---\ntitle: T\npriority: whenever\n---\n").is_err());
        // Case-insensitive values are normalized to the wire spelling.
        let start = parse("---\ntitle: T\ntype: BUG\npriority: URGENT\n---\n").unwrap();
        assert_eq!(start.item_type.as_deref(), Some("bug"));
        assert_eq!(start.priority.as_deref(), Some("urgent"));
    }

    #[test]
    fn recognizes_uuid_labels() {
        let template = "---\ntitle: T\nlabels:\n  - 11111111-1111-4111-8111-111111111111\n---\n";
        let start = parse(template).unwrap();
        assert_eq!(
            start.labels,
            vec![LabelRef {
                id: Some("11111111-1111-4111-8111-111111111111".to_string()),
                name: String::new(),
            }]
        );
    }

    #[test]
    fn requires_frontmatter_delimiters() {
        assert!(parse("# just markdown\n").is_err());
        assert!(parse("---\ntitle: T\n").is_err());
    }

    #[test]
    fn handles_crlf_and_bom() {
        let template = "\u{feff}---\r\ntitle: T\r\n---\r\nbody\r\n";
        let start = parse(template).unwrap();
        assert_eq!(start.title.as_deref(), Some("T"));
        assert_eq!(start.description.as_deref(), Some("body"));
    }

    #[test]
    fn empty_frontmatter_is_allowed_when_flags_supply_fields() {
        let start = parse("---\n---\n").unwrap();
        assert!(start.title.is_none());
        assert!(start.description.is_none());
    }
}
