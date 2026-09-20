// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik completion <shell>` — generate shell completion scripts.
//!
//! Extends the default `clap_complete` output with dynamic completion callbacks
//! for live values (Organization slugs, Project keys, Work Item keys, Status,
//! Type, and Label names). The dynamic callbacks shell out to `hamstik _hamstik_dyn_complete`
//! which resolves context, queries the API, and returns matching values.
//!
//! Acceptance criteria (CLI-32):
//! - Tab-completing `--org <TAB>` and `--project <TAB>` offers live values.
//! - Completion never issues a mutating request and never hangs when offline.
//! - Existing static flag/subcommand completion continues to work unchanged.

use clap::CommandFactory;
use clap_complete::Shell;

use crate::app::Session;
use crate::args::{Cli, CompletionArgs};
use crate::error::CliError;

/// Prints the completion script for the requested shell, augmented with
/// dynamic completion for live values.
pub fn run(session: &mut Session<'_>, args: &CompletionArgs) -> Result<(), CliError> {
    let mut command = Cli::command();
    // Hide the internal `_hamstik_dyn_complete` command from user-facing help.
    command = command.mut_subcommand("_hamstik_dyn_complete", |sub| {
        sub.hide(true).visible_alias(None::<&str>)
    });

    let mut out = Vec::new();
    clap_complete::generate(args.shell, &mut command, "hamstik", &mut out);
    let mut rendered = String::from_utf8(out).map_err(|err| {
        CliError::general(format!("completion script was not valid UTF-8: {err}"))
    })?;

    // Append dynamic completion helpers for supported shells.
    match args.shell {
        Shell::Bash => {
            rendered.push_str(DYNAMIC_BASH);
        }
        Shell::Zsh => {
            rendered.push_str(DYNAMIC_ZSH);
        }
        Shell::Fish => {
            rendered.push_str(DYNAMIC_FISH);
        }
        Shell::PowerShell => {
            // Dynamic completion is not supported for PowerShell yet.
            // Static completion from clap_complete is sufficient.
        }
        Shell::Elvish => {
            // Dynamic completion is not supported for Elvish yet.
            // Static completion from clap_complete is sufficient.
        }
        _ => {
            // Future shell variants: fall back to static completion.
        }
    }

    session.out.raw(&rendered).map_err(CliError::general)
}

/// Bash dynamic completion helper appended to the generated script.
const DYNAMIC_BASH: &str = r#"
# ──────────────────────────────────────────────────────────────────────
# Dynamic completion for live values (CLI-32)
# ──────────────────────────────────────────────────────────────────────

# Shell out to the hidden `hamstik _hamstik_dyn_complete` command to fetch
# live completion candidates. Any failure (offline, no credentials, no
# context, API error) produces no output, which the shell reads as "no
# candidates".
_hamstik_dynamic_complete() {
    local type_="$1"
    local prefix="${2:-}"
    hamstik _hamstik_dyn_complete "$type_" "$prefix" 2>/dev/null
}

# The clap-generated static completion function is named `_hamstik`; preserve
# it as `_hamstik_completion` so the wrapper below can fall back to it.
# Only the function-definition line is rewritten, so the case-pattern
# strings inside the body (e.g. `hamstik,_hamstik_dyn_complete`) are left
# untouched.
if declare -F _hamstik >/dev/null 2>&1; then
    eval "$(declare -f _hamstik | sed -e 's/^_hamstik() {/_hamstik_completion() {/')"
fi

# Dynamic-aware wrapper for the `hamstik` completion function. The generated
# script registered `_hamstik` for the `hamstik` command, so redefining it
# here is sufficient.
_hamstik() {
    local cur prev flag_type

    prev="$3"
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi

    # The word being completed is the value of a live flag.
    case "${prev}" in
        --org)
            flag_type="org"
            ;;
        --project)
            flag_type="project"
            ;;
        --status)
            flag_type="status"
            ;;
        --type)
            flag_type="type"
            ;;
        --label-name)
            flag_type="label"
            ;;
        *)
            flag_type=""
            ;;
    esac
    if [[ -n "${flag_type}" ]]; then
        COMPREPLY=( $(_hamstik_dynamic_complete "${flag_type}" "${cur}") )
        return 0
    fi

    # Work item key position: `hamstik work <subcommand> [flags] <key>`, or the
    # two-level form `hamstik work label|attachment|link|watcher <action>
    # [flags] <key>`. Only flags may appear between the key-accepting
    # subcommand and the cursor.
    if _hamstik_work_key_position; then
        COMPREPLY=( $(_hamstik_dynamic_complete work-item-key "${cur}") )
        return 0
    fi

    # No dynamic context: fall back to the generated static completion.
    _hamstik_completion "$@"
}

# Returns 0 when the current completion position is a work item key slot.
_hamstik_work_key_position() {
    local cword sub i
    cword=$(( COMP_CWORD - 1 ))
    (( cword >= 2 )) || return 1
    [[ "${COMP_WORDS[0]}" == hamstik && "${COMP_WORDS[1]}" == work ]] || return 1
    sub="${COMP_WORDS[2]}"
    case "${sub}" in
        label|attachment|link|watcher)
            (( cword >= 3 )) || return 1
            case "${COMP_WORDS[3]}" in
                add|remove|list|upload|download|delete|show|watch|unwatch|mute|unmute)
                    ;;
                *)
                    return 1
                    ;;
            esac
            for (( i = 4; i < cword; i++ )); do
                [[ "${COMP_WORDS[i]}" == -* ]] || return 1
            done
            return 0
            ;;
        view|edit|transition|start|close|archive|unarchive|delete|await|activity)
            for (( i = 3; i < cword; i++ )); do
                [[ "${COMP_WORDS[i]}" == -* ]] || return 1
            done
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}
"#;

/// Zsh dynamic completion helper appended to the generated script.
const DYNAMIC_ZSH: &str = r#"
# ──────────────────────────────────────────────────────────────────────
# Dynamic completion for live values (CLI-32)
# ──────────────────────────────────────────────────────────────────────

# Shell out to the hidden `hamstik _hamstik_dyn_complete` command to fetch
# live completion candidates. Any failure (offline, no credentials, no
# context, API error) produces no output, which the shell reads as "no
# candidates".
_hamstik_dynamic_complete() {
    local type_ prefix candidates
    type_="$1"
    prefix="${2:-}"
    candidates=( $(_hamstik_dynamic_complete_for "${type_}" "${prefix}") )
    # Placeholder; replaced below.
    return 0
}

# The clap-generated static completion function is named `_hamstik`; preserve
# it as `_hamstik_completion` so the wrapper below can fall back to it.
# Only the function-definition line is rewritten, so the helper functions and
# case-pattern strings inside the generated script are left untouched.
if [ "$funcstack[1]" != "_hamstik" ] && [ -n "$(typeset -f _hamstik 2>/dev/null)" ]; then
    eval "$(typeset -f _hamstik | sed -e 's/^_hamstik() {/_hamstik_completion() {/')"
fi
"#;
