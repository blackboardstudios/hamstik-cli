// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! `hamstik completion <shell>` — generate shell completion scripts.
//!
//! Extends the default `clap_complete` output with dynamic completion callbacks
//! for live values (Organization slugs, Project keys, Work Item keys, and Label
//! names). Bash and fish callbacks shell out to `hamstik _hamstik_dyn_complete`,
//! which resolves context, performs bounded read-only API lookups, and returns
//! matching values. Zsh, PowerShell, and Elvish remain static-only.
//!
//! Acceptance criteria (CLI-32):
//! - Tab-completing `--org <TAB>` and `--project <TAB>` offers live values.
//! - Completion never issues a mutating request and degrades silently offline.
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

    let mut out = Vec::new();
    clap_complete::generate(args.shell, &mut command, "hamstik", &mut out);
    let mut rendered = String::from_utf8(out).map_err(|err| {
        CliError::general(format!("completion script was not valid UTF-8: {err}"))
    })?;

    // clap_complete currently emits the hidden command in its candidate lists.
    // Remove only those user-visible strings; unreachable internal branches are
    // harmless. This is coupled to clap_complete's output format and guarded
    // by generated-script regression tests.
    strip_hidden_command_candidates(args.shell, &mut rendered);

    // The generated Bash function is `_hamstik() {` today. Rename that first
    // definition in Rust so the wrapper can call the static implementation
    // without depending on Bash's canonical `declare -f` formatting.
    match args.shell {
        Shell::Bash => {
            rename_bash_static_function(&mut rendered);
            rendered.push_str(DYNAMIC_BASH);
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

const INTERNAL_COMMAND: &str = "_hamstik_dyn_complete";

fn rename_bash_static_function(rendered: &mut String) {
    const STATIC_MARKER: &str = "_hamstik() {";
    const RENAMED_MARKER: &str = "_hamstik_completion() {";
    if let Some(index) = rendered.find(STATIC_MARKER) {
        rendered.replace_range(index..index + STATIC_MARKER.len(), RENAMED_MARKER);
    }
}

fn strip_hidden_command_candidates(shell: Shell, rendered: &mut String) {
    let mut stripped = String::with_capacity(rendered.len());
    for line in rendered.split_inclusive('\n') {
        let trimmed = line.trim();
        let remove = match shell {
            // Bash exposes root and `help` candidates through `opts` words.
            Shell::Bash => trimmed.starts_with("opts=") && line.contains(INTERNAL_COMMAND),
            // Fish emits each candidate as a separate `-a` argument.
            Shell::Fish => line.contains(&format!("-a \"{INTERNAL_COMMAND}\"")),
            // Zsh emits command candidates as `_describe` entries with a colon.
            Shell::Zsh => line.contains(&format!("'{INTERNAL_COMMAND}:")),
            // PowerShell candidate entries are removed in the first pass;
            // their now-unreachable switch branches are removed below.
            Shell::PowerShell => {
                line.contains(&format!("CompletionResult]::new('{INTERNAL_COMMAND}'"))
            }
            _ => false,
        };
        if !remove {
            stripped.push_str(line);
        } else if shell == Shell::Bash {
            stripped.push_str(&line.replace(&format!(" {INTERNAL_COMMAND}"), ""));
        }
    }
    *rendered = stripped;

    if shell == Shell::PowerShell {
        // The case bodies for the hidden command are not candidates, but
        // leaving them would retain internal command strings. Remove each
        // complete generated switch arm surgically.
        let mut cleaned = String::with_capacity(rendered.len());
        let mut skip_arm = false;
        for line in rendered.split_inclusive('\n') {
            let trimmed = line.trim();
            if skip_arm {
                if trimmed == "}" {
                    skip_arm = false;
                }
                continue;
            }
            if trimmed.starts_with('\'')
                && trimmed.ends_with("{")
                && line.contains(INTERNAL_COMMAND)
            {
                skip_arm = true;
                continue;
            }
            cleaned.push_str(line);
        }
        *rendered = cleaned;
    }
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
    shift 2
    command hamstik _hamstik_dyn_complete "$type_" "$prefix" "$@" 2>/dev/null
}

# Dynamic-aware wrapper for the `hamstik` completion function. The generated
# script registered `_hamstik` for the `hamstik` command, so redefining it
# here is sufficient.
_hamstik() {
    local cur prev flag_type value_prefix equals_flag
    local org_value project_value word
    local i
    local -a context_args

    prev="$3"
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi

    # Forward the most recently typed context flags. Scan only words before
    # the current word so `--org=<TAB>` remains a value being completed.
    context_args=()
    org_value=""
    project_value=""
    for (( i = 0; i < COMP_CWORD; i++ )); do
        word="${COMP_WORDS[i]}"
        case "${word}" in
            --org=*)
                org_value="${word#--org=}"
                ;;
            --project=*)
                project_value="${word#--project=}"
                ;;
            --org)
                if (( i + 1 < COMP_CWORD )); then
                    (( i++ ))
                    org_value="${COMP_WORDS[i]}"
                fi
                ;;
            --project)
                if (( i + 1 < COMP_CWORD )); then
                    (( i++ ))
                    project_value="${COMP_WORDS[i]}"
                fi
                ;;
        esac
    done
    if [[ -n "${org_value}" ]]; then
        context_args+=(--org "${org_value}")
    fi
    if [[ -n "${project_value}" ]]; then
        context_args+=(--project "${project_value}")
    fi

    # Complete the embedded value in `--org=<TAB>` / `--project=<TAB>` form.
    equals_flag=""
    case "${cur}" in
        --org=*)
            flag_type="org"
            equals_flag="--org="
            value_prefix="${cur#--org=}"
            ;;
        --project=*)
            flag_type="project"
            equals_flag="--project="
            value_prefix="${cur#--project=}"
            ;;
        *)
            flag_type=""
            value_prefix="${cur}"
            ;;
    esac
    if [[ -n "${equals_flag}" ]]; then
        local candidate
        COMPREPLY=()
        for candidate in $(_hamstik_dynamic_complete "${flag_type}" "${value_prefix}" "${context_args[@]}"); do
            COMPREPLY+=("${equals_flag}${candidate}")
        done
        return 0
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
        COMPREPLY=( $(_hamstik_dynamic_complete "${flag_type}" "${cur}" "${context_args[@]}") )
        return 0
    fi

    # Work item key position: `hamstik work <subcommand> [flags] <key>`, or the
    # two-level form `hamstik work label|attachment|link|watcher <action>
    # [flags] <key>`. Only flags may appear between the key-accepting
    # subcommand and the cursor.
    if _hamstik_work_key_position; then
        COMPREPLY=( $(_hamstik_dynamic_complete work-item-key "${cur}" "${context_args[@]}") )
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

/// Fish dynamic completion helper appended to the generated script.
const DYNAMIC_FISH: &str = r#"
# ──────────────────────────────────────────────────────────────────────
# Dynamic completion for live values (CLI-32)
# ──────────────────────────────────────────────────────────────────────

# Shell out to the hidden `hamstik _hamstik_dyn_complete` command to fetch
# live completion candidates. Any failure (offline, no credentials, no
# context, API error) produces no output, which fish reads as "no
# candidates".
function _hamstik_dynamic_complete
    set -l type_ "$argv[1]"
    set -l prefix "$argv[2]"
    set -l org_value ""
    set -l project_value ""
    set -l words (commandline -opc)
    set -l i 1
    while test $i -le (count $words)
        set -l word "$words[$i]"
        switch $word
            case '--org=*'
                set org_value (string sub --start 7 -- "$word")
            case '--project=*'
                set project_value (string sub --start 11 -- "$word")
            case '--org'
                set i (math $i + 1)
                if test $i -le (count $words)
                    set org_value "$words[$i]"
                end
                continue
            case '--project'
                set i (math $i + 1)
                if test $i -le (count $words)
                    set project_value "$words[$i]"
                end
                continue
        end
        set i (math $i + 1)
    end

    set -l context_args
    if test -n "$org_value"
        set -a context_args --org "$org_value"
    end
    if test -n "$project_value"
        set -a context_args --project "$project_value"
    end
    command hamstik _hamstik_dyn_complete "$type_" "$prefix" $context_args 2>/dev/null
end

# Register dynamic candidates for the live-value flags. Fish merges these
# with the static completions generated above, so flag/subcommand completion
# continues to work unchanged.
complete -c hamstik -l org -f -a '(_hamstik_dynamic_complete org (commandline -ct))'
complete -c hamstik -l project -f -a '(_hamstik_dynamic_complete project (commandline -ct))'
complete -c hamstik -l status -f -a '(_hamstik_dynamic_complete status (commandline -ct))'
complete -c hamstik -l type -f -a '(_hamstik_dynamic_complete type (commandline -ct))'
complete -c hamstik -l label-name -f -a '(_hamstik_dynamic_complete label (commandline -ct))'
"#;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn clap_bash_output_contains_the_static_function_marker() {
        let mut command = Cli::command();
        let mut output = Vec::new();
        clap_complete::generate(Shell::Bash, &mut command, "hamstik", &mut output);
        let rendered = String::from_utf8(output).expect("clap Bash output is UTF-8");
        assert!(
            rendered.contains("_hamstik() {"),
            "clap_complete changed the Bash function marker; update the Rust rename and tests"
        );
    }
}
