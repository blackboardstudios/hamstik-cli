// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Git-style external subcommand plugins (`hamstik-<name>`).
//!
//! An executable named `hamstik-<name>` on `PATH` is invoked when the user
//! runs `hamstik <name> [args...]` and `<name>` is not a built-in command —
//! the same model `git` and `gh` use for `git-<name>`/`gh-<name>` subcommands.
//!
//! The resolved context (host, profile, organization, project, context file)
//! and the selected output mode / global flags are passed via documented
//! `HAMSTIK_*` environment variables — never via argv. The raw
//! credential/token value never reaches the plugin: credential-bearing
//! environment variables are stripped before spawning.
//!
//! The core never *runs* a discovered plugin during read-only introspection
//! (`--help`, `commands`); those surfaces only *announce* the plugins and
//! label them as external, per the CLI-33 constraints.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::app::{Selection, Session};
use crate::error::CliError;
use crate::exit::{GENERAL, USAGE};

/// Prefix that identifies external plugin executables on `PATH`.
pub const PLUGIN_PREFIX: &str = "hamstik-";

/// Executable suffixes tried when locating a plugin on Windows; on Unix a
/// plugin is a plain executable with no suffix.
#[cfg(windows)]
const EXECUTABLE_SUFFIXES: &[&str] = &[".exe", ".com", ".bat", ".cmd"];
#[cfg(not(windows))]
const EXECUTABLE_SUFFIXES: &[&str] = &[""];

/// Plugin names discovered on `PATH`: the `<name>` portion of
/// `hamstik-<name>`, with the executable suffix stripped, deduplicated and
/// sorted.
pub fn discover_plugins() -> Vec<String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut plugins: Vec<String> = Vec::new();
    for dir in std::env::split_paths(&path) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let file_name = entry.file_name();
            let name_str = file_name.to_str();
            let Some(rest) = name_str.and_then(|n| n.strip_prefix(PLUGIN_PREFIX)) else {
                continue;
            };
            let name = executable_name(rest);
            if !name.is_empty() && !plugins.contains(&name.to_string()) {
                plugins.push(name.to_string());
            }
        }
    }
    plugins.sort();
    plugins
}

/// Strips a Windows executable suffix (`.exe`, `.com`, `.bat`, `.cmd`) from a
/// discovered plugin name. A no-op on Unix, where the suffix is part of a
/// legitimate executable name.
fn executable_name(name: &str) -> &str {
    #[cfg(windows)]
    {
        name.strip_suffix(".exe")
            .or_else(|| name.strip_suffix(".com"))
            .or_else(|| name.strip_suffix(".bat"))
            .or_else(|| name.strip_suffix(".cmd"))
            .unwrap_or(name)
    }
    #[cfg(not(windows))]
    {
        name
    }
}

/// Locates the full path of a plugin executable on `PATH`.
///
/// Windows candidates try the extensionless name followed by `.exe`,
/// `.com`, `.bat`, and `.cmd`; Unix looks only for the plain name.
fn which(name: &str) -> Result<PathBuf, std::io::Error> {
    let path = std::env::var_os("PATH")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "PATH not set"))?;
    for dir in std::env::split_paths(&path) {
        for suffix in EXECUTABLE_SUFFIXES {
            let candidate = dir.join(format!("{name}{suffix}"));
            if candidate.is_file() && is_executable(&candidate) {
                return Ok(candidate);
            }
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("executable not found for `{name}`"),
    ))
}

/// Whether a file is executable on the current platform.
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(metadata) => metadata.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Environment variables passed to an external plugin.
///
/// Carries the resolved context, output mode, global flags, and plugin
/// identity — nothing else sensitive. The raw credential value never
/// appears here.
fn build_env(
    session: &Session<'_>,
    selection: &Selection,
    plugin_name: &str,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();

    // Same context resolution precedence as any built-in command.
    env.push((
        "HAMSTIK_HOST".to_string(),
        selection.host.as_str().to_string(),
    ));
    if let Some(profile) = &selection.profile {
        env.push(("HAMSTIK_PROFILE".to_string(), profile.clone()));
    }
    if let Some(value) = &selection.organization.value {
        env.push(("HAMSTIK_ORG".to_string(), value.clone()));
    }
    if let Some(value) = &selection.project.value {
        env.push(("HAMSTIK_PROJECT".to_string(), value.clone()));
    }
    if let Some(context_path) = &selection.context_path {
        env.push((
            "HAMSTIK_CONTEXT_PATH".to_string(),
            context_path.to_string_lossy().to_string(),
        ));
    }

    // Output mode so the plugin can match the user's formatting intent.
    let format = match session.out.mode() {
        crate::output::Mode::Json => "json",
        crate::output::Mode::JsonLines => "jsonl",
        crate::output::Mode::Tsv => "tsv",
        crate::output::Mode::Csv => "csv",
        crate::output::Mode::Markdown => "markdown",
        crate::output::Mode::Quiet => "quiet",
        crate::output::Mode::Human => "human",
    };
    env.push(("HAMSTIK_FORMAT".to_string(), format.to_string()));

    // Global flags as boolean knobs.
    for (key, flag) in [
        ("HAMSTIK_QUIET", session.global.quiet),
        ("HAMSTIK_VERBOSE", session.global.verbose),
        ("HAMSTIK_DRY_RUN", session.global.dry_run),
        ("HAMSTIK_NO_INPUT", session.global.no_input),
        ("HAMSTIK_NO_RETRY", session.global.no_retry),
    ] {
        if flag {
            env.push((key.to_string(), "1".to_string()));
        }
    }

    // `--no-color` and `--color=never` are documented as the same decision,
    // so the plugin sees the Hamstik opt-out variable either way. The plugin
    // also inherits the ambient `HAMSTIK_TERM` profile through the parent
    // environment (nothing is stripped below).
    if session.global.color_mode() == crate::terminal::ColorMode::Never {
        env.push(("HAMSTIK_NO_COLOR".to_string(), "1".to_string()));
    }

    // Plugin identity, so the invocation site always knows which plugin runs.
    env.push(("HAMSTIK_PLUGIN".to_string(), plugin_name.to_string()));

    env
}

/// Spawns and waits on a discovered external plugin.
///
/// The plugin's exit code becomes the process exit code. On failure (missing
/// or unlaunchable executable, context-resolution failure, spawn/wait error)
/// a stable exit code is set and the error is printed; the command always
/// returns `Ok`.
pub fn run_plugin(
    session: &mut Session<'_>,
    plugin_name: &str,
    args: &[String],
) -> Result<(), CliError> {
    let executable_name = format!("{}{}", PLUGIN_PREFIX, plugin_name);

    // Context must resolve exactly as it would for any built-in command; a
    // failing configuration must not silently launch a contextless plugin.
    let selection = match session.selection() {
        Ok(selection) => selection,
        Err(err) => {
            eprintln!("error: {err}");
            session.exit_code = err.exit_code();
            return Ok(());
        }
    };

    let executable = match which(&executable_name) {
        Ok(path) => path,
        Err(_) => {
            eprintln!(
                "error: external plugin not found: `{executable_name}` is not on PATH; \
                 place an executable named `{executable_name}` on PATH to use it"
            );
            session.exit_code = USAGE;
            return Ok(());
        }
    };

    // Environment: inherited variables minus the credential-bearing ones,
    // plus the documented HAMSTIK_* variables. The comparison is
    // case-insensitive because Windows environment names are.
    let sensitive: [&str; 5] = [
        "HAMSTIK_TOKEN",
        "AUTHORIZATION",
        "HAMSTIK_API_KEY",
        "HAMSTIK_SECRET",
        "HAMSTIK_PASSWORD",
    ];
    let inherited =
        std::env::vars().filter(|(k, _)| !sensitive.iter().any(|s| s.eq_ignore_ascii_case(k)));
    let env_vars = build_env(session, &selection, plugin_name);
    // Windows: `CreateProcess` cannot launch batch files (`.bat`/`.cmd`) directly,
    // yet `discover_plugins`/`which` list them as candidates. Route them through
    // `cmd.exe /c` so discovery and invocation agree on every platform.
    let is_windows_batch = cfg!(windows)
        && executable.extension().is_some_and(|e| {
            let lower = e.to_string_lossy().to_lowercase();
            lower == "bat" || lower == "cmd"
        });
    let mut command = if is_windows_batch {
        let mut cmd = std::process::Command::new("cmd");
        cmd.arg("/c").arg(&executable);
        cmd
    } else {
        std::process::Command::new(&executable)
    };
    let child = command
        .args(args)
        .env_clear()
        .envs(inherited)
        .envs(env_vars.iter().map(|(k, v)| (k, v.as_str())))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();

    match child {
        Ok(mut child) => match child.wait() {
            Ok(status) => {
                session.exit_code = status.code().unwrap_or(GENERAL);
                Ok(())
            }
            Err(err) => {
                eprintln!("error: failed to wait for `{executable_name}`: {err}");
                session.exit_code = GENERAL;
                Ok(())
            }
        },
        Err(err) => {
            eprintln!("error: failed to execute `{executable_name}`: {err}");
            session.exit_code = GENERAL;
            Ok(())
        }
    }
}

/// Plugin manifest entries for `commands`. Static metadata only — the plugins
/// themselves are never executed here (see module docs).
pub fn plugins_as_json() -> Vec<serde_json::Value> {
    discover_plugins()
        .into_iter()
        .map(|name| {
            serde_json::json!({
                "command": format!("hamstik {}", name),
                "about": format!(
                    "(external plugin; run `hamstik {name} --help` for its own help)"
                ),

                "aliases": Vec::<String>::new(),
                "positionals": Vec::<String>::new(),
                "options": Vec::<String>::new(),
                "hasSubcommands": false,
                "external": true,
                "capabilities": {
                    "json": true,
                    "noInput": true,
                },
                "arguments": Vec::<serde_json::Value>::new(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_prefix_is_constant() {
        assert_eq!(PLUGIN_PREFIX, "hamstik-");
    }

    #[test]
    fn executable_name_strips_windows_suffixes() {
        assert_eq!(executable_name("hello"), "hello");
        #[cfg(windows)]
        {
            assert_eq!(executable_name("hello.exe"), "hello");
            assert_eq!(executable_name("hello.bat"), "hello");
            assert_eq!(executable_name("hello.com"), "hello");
            assert_eq!(executable_name("hello.cmd"), "hello");
        }
    }

    #[test]
    fn discover_plugins_returns_sorted_list() {
        let plugins = discover_plugins();
        let mut sorted = plugins.clone();
        sorted.sort();
        assert_eq!(plugins, sorted, "plugins should be sorted");
    }

    #[test]
    fn discover_plugins_no_duplicates() {
        let plugins = discover_plugins();
        let mut unique = plugins.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(plugins, unique, "plugins should have no duplicates");
    }
}
