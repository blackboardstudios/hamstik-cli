// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Command-tree introspection for the `commands` manifest.
//!
//! Everything here derives from the authoritative clap tree
//! (`Cli::command()`): names, aliases, options, defaults, value enums,
//! deprecation notes, help text. There is no hand-maintained duplication —
//! if the command tree changes, the manifest changes with it.

use clap::Command as ClapCommand;

/// A rendered manifest node: one command (root or subcommand).
#[derive(Debug, Clone)]
pub(crate) struct ManifestCommand {
    /// Command path, e.g. `hamstik work list` (`hamstik` for the root).
    pub path: String,
    /// The command's short help text.
    pub about: String,
    /// The long help, when present.
    pub long_about: Option<String>,
    /// Hidden aliases that also invoke the command.
    pub aliases: Vec<String>,
    /// Positional argument names, in order.
    pub positionals: Vec<String>,
    /// Option names (long form), with leading dashes.
    pub options: Vec<String>,
    /// Whether the command declares a subcommand set.
    pub has_subcommands: bool,
}

/// Walks the whole clap tree depth-first, returning every command.
pub(crate) fn collect(cli: &ClapCommand) -> Vec<ManifestCommand> {
    let mut nodes = Vec::new();
    walk(cli, "hamstik", &mut nodes);
    nodes
}

fn walk(command: &ClapCommand, path: &str, out: &mut Vec<ManifestCommand>) {
    let aliases = command
        .get_all_aliases()
        .map(str::to_string)
        .collect::<Vec<_>>();
    out.push(ManifestCommand {
        path: path.to_string(),
        about: command
            .get_about()
            .map(|about| about.to_string())
            .unwrap_or_default(),
        long_about: command.get_long_about().map(|about| about.to_string()),
        aliases,
        positionals: command
            .get_positionals()
            .map(|arg| arg.get_id().to_string())
            .collect(),
        options: command
            .get_arguments()
            .filter(|arg| arg.get_long().is_some())
            .filter_map(|arg| arg.get_long().map(|long| format!("--{long}")))
            .collect(),
        has_subcommands: command.has_subcommands(),
    });
    for sub in command.get_subcommands() {
        walk(sub, &format!("{path} {}", sub.get_name()), out);
    }
}

/// True when an argument is a flag-style value enum (choices known statically).
pub(crate) fn static_choices(arg: &clap::Arg) -> Vec<String> {
    Some(arg.get_possible_values())
        .map(|values| {
            values
                .iter()
                .filter(|v| !v.is_hide_set())
                .map(|v| v.get_name().to_string())
                .collect()
        })
        .unwrap_or_default()
}
