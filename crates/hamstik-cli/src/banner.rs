// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Canonical CLI identity banner.
//!
//! The banner (ASCII hamster + `hamstik cli` wordmark + copyright footer) is
//! shown only for the *root* help and human version surfaces: `hamstik`,
//! `hamstik --help`/`-h`, `hamstik --version`/`-V`, and `hamstik version`. It is
//! deliberately absent from `--json` machine output (PRD §24 forbids decorative
//! banners there), from subcommand help, and from completion scripts.
//!
//! [`banner`] is the single source of truth for the art; [`root_help_template`]
//! derives the root help layout from it so the art is never duplicated.

/// The ASCII hamster + wordmark. Leading spaces are load-bearing (they align
/// the hamster against the wordmark); per-line trailing spaces are trimmed so
/// the source passes `git diff --check`. The content has no `{`/`}` and no
/// `"#` sequence, so it is safe both as a raw literal and as literal clap help
/// template text (which is never word-wrapped).
const ART: &str = r#"              _                         _   _ _
             | |__   __ _ _ __ ___  ___| |_(_) | __
    (\___/)  | '_ \ / _` | '_ ` _ \/ __| __| | |/ /
    (='.'=)  | | | | (_| | | | | | \__ \ |_| |   <
    (")_(")  |_| |_|\__,_|_| |_| |_|___/\__|_|_|\_"#;

/// clap's default help template, reproduced verbatim so prepending the banner
/// leaves the rest of the root help body byte-identical. The template's
/// `{before-help}` placeholder stays empty in the template itself; `main.rs` may
/// prepend a plain, already-wrapped block there for discovered external
/// plugins (which clap would otherwise word-wrap and reflow).
const DEFAULT_HELP_TEMPLATE: &str = r#"{before-help}{about-with-newline}
{usage-heading} {usage}

{all-args}{after-help}"#;

/// Builds the banner: the art, a blank separator line, then a versioned
/// copyright footer. No trailing newline (callers add their own).
///
/// The version is baked in at compile time from `CARGO_PKG_VERSION`. The `©`
/// is a plain U+00A9 copyright sign (no emoji variation selector) so it renders
/// monochrome without depending on an emoji font.
#[must_use]
pub fn banner() -> String {
    let version = env!("CARGO_PKG_VERSION");
    format!("{ART}\n\n🐹 hamstik cli v{version}      © Blackboard Studios LLC")
}

/// The root command's help template: the banner followed by a blank line and
/// then clap's stock help body. Apply via `Command::help_template`.
#[must_use]
pub fn root_help_template() -> String {
    let banner = banner();
    format!("{banner}\n\n{DEFAULT_HELP_TEMPLATE}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn banner_includes_art_and_versioned_footer() {
        let rendered = banner();
        let version = env!("CARGO_PKG_VERSION");
        assert!(rendered.starts_with("              _"), "art top missing");
        assert!(rendered.contains(r"(\___/)"), "hamster paws missing");
        assert!(rendered.contains("(='.'=)"), "hamster face missing");
        assert!(
            rendered.contains(&format!("🐹 hamstik cli v{version}")),
            "versioned footer missing"
        );
        assert!(
            rendered.contains("© Blackboard Studios LLC"),
            "copyright footer missing"
        );
    }

    #[test]
    fn banner_has_no_trailing_newline_and_one_blank_separator() {
        let rendered = banner();
        assert!(!rendered.ends_with('\n'), "banner must not end in newline");
        let lines: Vec<&str> = rendered.split('\n').collect();
        assert_eq!(lines.len(), 7, "expected 5 art + blank + footer");
        assert_eq!(lines[5], "", "line 6 must be the blank separator");
    }

    #[test]
    fn banner_has_no_ansi_escapes() {
        assert!(!banner().contains('\x1b'), "banner must be plain text");
    }

    #[test]
    fn copyright_is_plain_sign_without_variation_selector() {
        assert!(!banner().contains('\u{fe0f}'), "must not use VS16");
    }

    #[test]
    fn root_help_template_prepends_banner_and_keeps_default_body() {
        let template = root_help_template();
        assert!(template.starts_with(&banner()));
        assert!(template.ends_with(DEFAULT_HELP_TEMPLATE));
        assert!(template.contains("{usage-heading} {usage}"));
    }
}
