// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik commands` — the machine-readable command manifest.
//!
//! Deterministically derived from the authoritative clap command tree. The
//! manifest covers every command and subcommand with its aliases, options,
//! positional arguments, statically knowable enum choices, defaults, and
//! per-command capability metadata (`--json`/`--no-input` support), and is
//! stable input for the Agent Skill's command-surface validation and the
//! generated reference documentation. With `--cookbook` the same command
//! prints the copy-pasteable workflow examples instead (see
//! [`super::cookbook`]).

use clap::CommandFactory;
use serde_json::{Value, json};

use crate::app::Session;
use crate::args::{Cli, CommandsArgs};
use crate::error::CliError;

use super::emit_json;
use super::manifest;

/// The manifest envelope's schema version; bump on layout changes.
pub(crate) const MANIFEST_VERSION: u64 = 1;

/// Runs `hamstik commands`.
pub fn run(session: &mut Session<'_>, args: &CommandsArgs) -> Result<(), CliError> {
    if args.cookbook {
        return super::cookbook::run(session);
    }
    let manifest = build()?;
    emit_json(session, &manifest)
}

/// Builds the manifest document from the authoritative command tree.
pub(crate) fn build() -> Result<Value, CliError> {
    let cli = Cli::command();
    let nodes = manifest::collect(&cli);

    let mut commands = Vec::new();
    for node in &nodes {
        // Effective options include ancestors' declarations: global flags
        // (`--json`, `--no-input`, …) apply to every subcommand through
        // ancestry even when declared only on the root.
        let effective_options = {
            let mut all: Vec<String> = node.options.clone();
            let parts: Vec<&str> = node.path.split_whitespace().collect();
            for end in (1..parts.len()).rev() {
                let parent_path = parts[..end].join(" ");
                if let Some(parent) = nodes.iter().find(|c| c.path == parent_path) {
                    for option in &parent.options {
                        if !all.contains(option) {
                            all.push(option.clone());
                        }
                    }
                }
            }
            all
        };

        let mut capabilities = serde_json::Map::new();
        capabilities.insert(
            "json".to_string(),
            json!(effective_options.iter().any(|o| o == "--json")),
        );
        capabilities.insert(
            "noInput".to_string(),
            json!(effective_options.iter().any(|o| o == "--no-input")),
        );

        let mut entry = json!({
            "command": node.path,
            "about": node.about,
            "aliases": node.aliases,
            "positionals": node.positionals,
            "options": node.options,
            "hasSubcommands": node.has_subcommands,
            "capabilities": Value::Object(capabilities),
        });
        if let Some(long_about) = &node.long_about {
            entry["longAbout"] = json!(long_about);
        }

        // Enum choices + defaults + help per argument, from the tree.
        let mut arguments = Vec::new();
        if let Some(command) = find_command(&cli, &node.path) {
            for arg in command.get_arguments() {
                let mut detail = serde_json::Map::new();
                if let Some(long) = arg.get_long() {
                    detail.insert("long".to_string(), json!(format!("--{long}")));
                }
                if let Some(short) = arg.get_short() {
                    detail.insert("short".to_string(), json!(format!("-{short}")));
                }
                detail.insert(
                    "help".to_string(),
                    json!(
                        arg.get_help()
                            .map(|help| help.to_string())
                            .unwrap_or_default()
                    ),
                );
                if arg.is_positional() {
                    detail.insert("kind".to_string(), json!("positional"));
                    detail.insert("name".to_string(), json!(arg.get_id().to_string()));
                    if let Some([value_name]) = arg.get_value_names() {
                        detail.insert("valueName".to_string(), json!(value_name.as_str()));
                    }
                } else {
                    detail.insert("kind".to_string(), json!("option"));
                }
                let choices = manifest::static_choices(arg);
                if !choices.is_empty() {
                    detail.insert("choices".to_string(), json!(choices));
                }
                if let Some(default) = arg.get_default_values().first() {
                    detail.insert("default".to_string(), json!(default.to_str().unwrap_or("")));
                }
                if arg.is_hide_set() {
                    detail.insert("hidden".to_string(), json!(true));
                }
                arguments.push(Value::Object(detail));
            }
        }
        entry["arguments"] = Value::Array(arguments);
        commands.push(entry);
    }

    // Append discovered external plugins.
    for ext in super::external::plugins_as_json() {
        commands.push(ext);
    }

    Ok(json!({
        "manifestVersion": MANIFEST_VERSION,
        "binary": "hamstik",
        "commands": commands,
    }))
}

/// Finds the command node at a dotted command path ("hamstik work list").
fn find_command<'a>(root: &'a clap::Command, path: &str) -> Option<&'a clap::Command> {
    let parts: Vec<&str> = path.split_whitespace().skip(1).collect();
    let mut current = root;
    for part in parts {
        current = current.find_subcommand(part)?;
    }
    Some(current)
}
