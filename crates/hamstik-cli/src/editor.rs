// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Editor-based authoring for long-form text.
//!
//! `--editor` launches `$VISUAL` (then `$EDITOR`, then platform defaults)
//! on a secure temporary file, and uses the resulting content as the
//! argument value. The template carries instructions as leading `#`
//! comments, which are stripped from the final text. An unchanged file is
//! treated as a cancellation: the command fails with a usage error and
//! nothing is sent.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::Command as Process;

use crate::environment::Environment;
use crate::error::CliError;

/// Editor candidates in precedence order: `$VISUAL` then `$EDITOR`, then the
/// per-platform fallbacks.
fn editor_command(env: &dyn crate::environment::Environment) -> Option<String> {
    for key in ["VISUAL", "EDITOR"] {
        if let Some(value) = env
            .var(key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return Some(value);
        }
    }
    // Sensible per-platform fallbacks; `vi` is POSIX-mandated, `notepad`
    // exists on every Windows install.
    if cfg!(windows) {
        Some("notepad".to_string())
    } else {
        Some("vi".to_string())
    }
}

/// Launches the configured editor on a secure temporary file and returns the
/// edited content.
///
/// Security: the temp file is created with owner-only permissions (0600 on
/// Unix) in the system temp directory, and removed on every exit path.
/// Editor failure (nonzero exit, missing binary) is a usage-class error;
/// unchanged template content is treated as a deliberate cancellation.
pub fn edit_text(env: &dyn Environment, no_input: bool, what: &str) -> Result<String, CliError> {
    if no_input {
        return Err(CliError::usage(
            "editor authoring is disabled under --no-input; provide the text with --file - (stdin), \
             a --*-file path, or inline",
        ));
    }
    let editor = editor_command(env).ok_or_else(|| {
        CliError::usage("no editor configured; set $VISUAL or $EDITOR, or use --*-file")
    })?;

    let header = template_header(what);
    let mut temp = tempfile_in_default_dir()?;
    write!(temp.handle, "{header}")
        .map_err(|err| CliError::general(format!("cannot write temporary file: {err}")))?;

    let launch = launch_editor(&editor, &temp);
    let result = launch.and_then(|status| {
        if status_success(&status) {
            Ok(())
        } else {
            Err(CliError::usage(format!(
                "editor {editor:?} exited with a failure status; nothing was sent"
            )))
        }
    });

    // Read the content back before cleanup so a failed read is also a clean
    // failure, and the temp file never outlives this function.
    let outcome = result.and_then(|()| {
        let content = std::fs::read_to_string(&temp.path)
            .map_err(|err| CliError::general(format!("cannot read edited file: {err}")))?;
        let edited = strip_template(&content, &header);
        if edited.trim().is_empty() {
            return Err(CliError::usage(
                "editor content was left empty; treating this as a cancellation (nothing was sent)",
            ));
        }
        // Content passes through verbatim after template-header removal:
        // Unicode, Markdown, and the author's trailing newline survive.
        Ok(edited)
    });

    // Remove the temp file on success and failure alike.
    let _ = std::fs::remove_file(&temp.path);
    outcome
}

/// The instructional header written into the editor buffer.
///
/// Markdown headings (`# Title`) are user content, so only these exact
/// template lines are recognized and removed — not arbitrary `#` lines.
fn template_header(what: &str) -> String {
    format!("# Authoring {what}. Lines starting with '#' are removed on save.\n")
}

/// Removes only the template's own header lines (matched exactly), leaving
/// user content — including Markdown `#` headings — untouched.
fn strip_template(content: &str, header: &str) -> String {
    let header_lines: Vec<&str> = header.lines().collect();
    let mut lines = content.lines().peekable();
    // Skip the template lines when they appear at the very top, in order.
    for expected in &header_lines {
        match lines.peek() {
            Some(line) if line == expected => {
                lines.next();
            }
            // The user may have deleted some guidance; stop at first mismatch.
            _ => break,
        }
    }
    let rest = lines.collect::<Vec<_>>().join("\n");
    // Preserve the author's trailing newline; `lines()` discards it.
    if content.ends_with('\n') && !rest.is_empty() && !rest.ends_with('\n') {
        format!("{rest}\n")
    } else {
        rest
    }
}

/// A secure temporary file that removes itself when dropped as a fallback.
struct TempFile {
    path: PathBuf,
    handle: std::fs::File,
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Creates a uniquely named temp file with owner-only permissions.
fn tempfile_in_default_dir() -> Result<TempFile, CliError> {
    let base = std::env::temp_dir();
    let unique = format!(
        "hamstik-edit-{}-{}.md",
        std::process::id(),
        crate::commands::timestamp_rfc3339().replace([':', '-'], "")
    );
    let path = base.join(unique);
    // Create exclusively so a pre-planted symlink cannot redirect the write;
    // 0600 on Unix, default ACLs elsewhere (no broader permissions requested).
    let handle = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|err| CliError::general(format!("cannot create temporary file: {err}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = handle.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    Ok(TempFile { path, handle })
}

/// Launches the editor command on the temp file and waits for it.
fn launch_editor(editor: &str, temp: &TempFile) -> Result<std::process::ExitStatus, CliError> {
    // The editor string is split on whitespace (VISUAL conventions like
    // "code -w"); the temp path is always the final argument.
    let mut parts = editor.split_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| CliError::usage("configured editor is empty"))?;
    let arguments: Vec<&str> = parts.collect();
    Process::new(program)
        .args(&arguments)
        .arg(&temp.path)
        .status()
        .map_err(|err| {
            CliError::usage(format!(
                "cannot launch editor {program:?} ({err}); set $VISUAL or $EDITOR to an available editor"
            ))
        })
}

fn status_success(status: &std::process::ExitStatus) -> bool {
    status.success()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::environment::MapEnvironment;

    #[test]
    fn editor_precedence_visual_then_editor() {
        let env = MapEnvironment::new()
            .with_var("VISUAL", "nvim")
            .with_var("EDITOR", "vim");
        assert_eq!(editor_command(&env).as_deref(), Some("nvim"));
        let env = MapEnvironment::new().with_var("EDITOR", "nano");
        assert_eq!(editor_command(&env).as_deref(), Some("nano"));
    }

    #[test]
    fn empty_editor_env_falls_back() {
        let env = MapEnvironment::new().with_var("VISUAL", "  ");
        let command = editor_command(&env).unwrap();
        assert!(!command.is_empty());
    }

    #[test]
    fn template_header_is_removed_user_content_kept() {
        let header = template_header("a description");
        let content = format!("{header}Hello\nWorld\n");
        let stripped = strip_template(&content, &header);
        assert_eq!(stripped, "Hello\nWorld\n");
        assert!(!stripped.contains("Authoring"));
    }

    #[test]
    fn template_header_removal_tolerates_edited_guidance() {
        let header = template_header("a description");
        // The user deleted the guidance entirely: content still resolves.
        let stripped = strip_template("Hello\n", &header);
        assert_eq!(stripped, "Hello\n");
    }

    #[test]
    fn trailing_newline_is_preserved() {
        let header = template_header("a description");
        assert_eq!(
            strip_template(&format!("{header}Body\n"), &header),
            "Body\n"
        );
        assert_eq!(strip_template("Body", &header), "Body");
        assert_eq!(strip_template(&header, &header), "");
    }

    #[test]
    fn markdown_headings_and_unicode_survive() {
        let header = template_header("a description");
        let content =
            format!("{header}# Heading\n\nÜnïcödé ✓ — em-dash\n\n```rust\nfn main() {{}}\n```\n");
        let stripped = strip_template(&content, &header);
        assert_eq!(
            stripped, "# Heading\n\nÜnïcödé ✓ — em-dash\n\n```rust\nfn main() {}\n```\n",
            "Markdown '#' headings are user content and must be verbatim"
        );
    }
}
