// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Release gate binary: `release-gate --tag vX.Y.Z` run from the repository
//! root. Fails closed (exit 1) with an actionable message when the tag, the
//! workspace version, and the changelog release section disagree.
//!
//! Used by `.github/workflows/release.yml` before any release artifact is
//! built or published; see `design/RELEASE.md`.

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use release_gate::run_gate;

const USAGE: &str = "usage: release-gate --tag <vX.Y.Z> [--manifest <Cargo.toml>] \
[--changelog <CHANGELOG.md>]";

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1).collect()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let manifest = read_or_fail(&args.manifest, "workspace Cargo.toml");
    let changelog = read_or_fail(&args.changelog, "CHANGELOG.md");

    match run_gate(&args.tag, &manifest, &changelog) {
        Ok(parsed) => {
            println!(
                "release gate: tag {} matches workspace version {} and the \
                 CHANGELOG.md release section",
                parsed.tag_text(),
                parsed.version
            );
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("release gate: {message}");
            ExitCode::FAILURE
        }
    }
}

struct GateArgs {
    tag: String,
    manifest: PathBuf,
    changelog: PathBuf,
}

/// Parses the small argument surface; unknown flags or a missing `--tag` are
/// usage errors so the workflow never gates against the wrong inputs.
fn parse_args(args: Vec<String>) -> Result<GateArgs, String> {
    let mut tag: Option<String> = None;
    let mut manifest = PathBuf::from("Cargo.toml");
    let mut changelog = PathBuf::from("CHANGELOG.md");
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--tag" => {
                tag = Some(
                    iter.next()
                        .ok_or_else(|| "--tag requires a value".to_string())?,
                );
            }
            "--manifest" => {
                manifest = PathBuf::from(
                    iter.next()
                        .ok_or_else(|| "--manifest requires a path".to_string())?,
                );
            }
            "--changelog" => {
                changelog = PathBuf::from(
                    iter.next()
                        .ok_or_else(|| "--changelog requires a path".to_string())?,
                );
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let tag = tag.ok_or_else(|| "--tag <vX.Y.Z> is required".to_string())?;
    Ok(GateArgs {
        tag,
        manifest,
        changelog,
    })
}

fn read_or_fail(path: &PathBuf, label: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| {
        eprintln!(
            "release gate: cannot read {label} at {}: {error}",
            path.display()
        );
        std::process::exit(1);
    })
}
