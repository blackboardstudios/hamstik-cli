// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Binary entrypoint.
//!
//! Parses arguments, assembles the production services, and hands off to
//! `app::run`. All logic lives in the library crate so it can be tested.

use std::io::{self, Write};
use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{CommandFactory, FromArgMatches};

use hamstik_cli::app::{self, ProductionApiFactory, Services};
use hamstik_cli::args::Cli;
use hamstik_cli::banner;
use hamstik_cli::config::ConfigStore;
use hamstik_cli::credentials::KeyringCredentialStore;
use hamstik_cli::environment::SystemEnvironment;
use hamstik_cli::input::TerminalInput;

fn main() {
    // The CLI is strictly sequential: one command, no concurrency. A
    // current-thread runtime starts faster and carries no worker threads.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("error: failed to start runtime: {err}");
            std::process::exit(hamstik_cli::exit::GENERAL);
        }
    };
    let code = match runtime.block_on(entry()) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            hamstik_cli::exit::CONFIGURATION
        }
    };
    let _ = io::stdout().flush();
    std::process::exit(code);
}

async fn entry() -> Result<i32, String> {
    let mut command = Cli::command().help_template(banner::root_help_template());

    // Announce discovered external plugins in the root help. They are
    // labeled as external (not built-in) and never executed during
    // introspection (see commands/external.rs).
    let plugins = hamstik_cli::commands::external::discover_plugins();
    if !plugins.is_empty() {
        let listing = plugins
            .iter()
            .map(|name| format!("  hamstik {}  (external plugin)", name))
            .collect::<Vec<_>>()
            .join("\n");
        command = command.before_help(format!("\nExternal plugins:\n{listing}\n"));
    }

    let matches = match command.try_get_matches() {
        Ok(matches) => matches,
        Err(err) => return Ok(render_clap_error(&err)),
    };
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(err) => return Ok(render_clap_error(&err)),
    };
    let config_path = resolve_config_path()?;
    let config = ConfigStore::new(config_path);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let environment = SystemEnvironment;
    let store = KeyringCredentialStore;
    let factory = ProductionApiFactory;
    let mut prompt = TerminalInput;

    let services = Services {
        env: &environment,
        store: &store,
        config,
        factory: &factory,
        prompt: &mut prompt,
        cwd,
        stdout: Box::new(io::stdout()),
        stderr: Box::new(io::stderr()),
    };

    Ok(app::run(cli, services).await)
}

/// Renders clap-generated errors, keeping the banner on stdout only.
///
/// - `--version`/`-V`: one terse `hamstik <version>` line on stdout (the
///   machine-parsed surface), exit success. The full banner stays on the
///   human `hamstik version` path and the root help.
/// - Help (root/subcommand `--help`): printed to stdout while preserving
///   clap's numeric exit code. The banner rides in the root help template.
/// - Usage errors and the missing-subcommand help: clap's own rendering on
///   stderr, so failure output is never mistaken for success content.
fn render_clap_error(err: &clap::Error) -> i32 {
    match err.kind() {
        ErrorKind::DisplayVersion => {
            println!("hamstik {}", env!("CARGO_PKG_VERSION"));
            hamstik_cli::exit::SUCCESS
        }
        ErrorKind::DisplayHelp => {
            print!("{err}");
            err.exit_code()
        }
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            // A bare `hamstik` or `hamstik <unknown>` is an error, not help:
            // keep the exit code and clap's stderr rendering.
            err.exit()
        }
        _ => err.exit(),
    }
}

/// Resolves the config file path (SPEC §23).
///
/// Unlike every other input this is not overridable per-command and cannot be
/// repaired with a flag, so an undeterminable home directory fails with
/// guidance instead of silently scattering config into the working directory.
fn resolve_config_path() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("HAMSTIK_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    directories::ProjectDirs::from("com", "blackboard", "hamstik")
        .map(|dirs| dirs.config_dir().join("config.toml"))
        .ok_or_else(|| {
            "cannot determine the configuration directory (no home directory?); \
             set HAMSTIK_CONFIG to an explicit path"
                .to_string()
        })
}
