// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Terminal capability detection and ANSI styling helpers.
//!
//! `doctor` uses these probes to report whether the running terminal can
//! render ANSI colors and emoji — the two decorations the CLI puts into human
//! output — so users can verify nothing renders mangled. Detection follows the
//! [`NO_COLOR`] and [`CLICOLOR`] conventions. `--json` output never contains
//! ANSI codes (SPEC §39), which doctor guarantees by construction: the sample
//! line renders only in human mode.
//!
//! [`NO_COLOR`]: https://no-color.org/
//! [`CLICOLOR`]: https://bixense.com/clicolors/

use crate::environment::Environment;

/// SGR sequence for green foreground (used for `ok` markers).
pub const SGR_GREEN: &str = "\x1b[32m";
/// SGR sequence for red foreground (used for `FAIL` markers).
pub const SGR_RED: &str = "\x1b[31m";
/// SGR sequence for bright-yellow foreground (used for `WARN`/`skip` markers
/// and the doctor color sample).
///
/// Bright yellow (SGR 93) is used instead of normal yellow (SGR 33) because
/// several common terminal themes — notably GNOME Terminal's default palettes
/// — render the normal-yellow slot as brown/ochre, which defeats the warning
/// semantics. SGR 93 still resolves through the user's palette; no RGB or
/// truecolor sequences are involved.
pub const SGR_YELLOW: &str = "\x1b[93m";
const SGR_RESET: &str = "\x1b[0m";

/// The result of a capability probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// Whether the capability is considered available.
    pub ok: bool,
    /// Human explanation, including the deciding factor.
    pub detail: String,
}

impl Probe {
    fn new(ok: bool, detail: impl Into<String>) -> Self {
        Self {
            ok,
            detail: detail.into(),
        }
    }
}

/// Wraps `text` in an SGR color when `color` is enabled; returns it verbatim
/// otherwise.
#[must_use]
pub fn paint(color: bool, sgr: &str, text: &str) -> String {
    if color {
        format!("{sgr}{text}{SGR_RESET}")
    } else {
        text.to_string()
    }
}

/// Probes ANSI color support for stdout, honoring the `--no-color` flag.
///
/// Precedence: the explicit `--no-color` flag wins over everything, then
/// `CLICOLOR_FORCE` (force-enable no matter what), then `NO_COLOR`, then
/// `CLICOLOR=0`, then a non-terminal stdout, then `TERM` (unset or `dumb`).
#[must_use]
pub fn color_probe(env: &dyn Environment, flag_no_color: bool, stdout_terminal: bool) -> Probe {
    color_probe_impl(env, flag_no_color, stdout_terminal, cfg!(windows))
}

fn color_probe_impl(
    env: &dyn Environment,
    flag_no_color: bool,
    stdout_terminal: bool,
    windows: bool,
) -> Probe {
    if flag_no_color {
        return Probe::new(false, "disabled (--no-color flag)");
    }
    let forced = env
        .var("CLICOLOR_FORCE")
        .is_some_and(|value| !value.is_empty() && value != "0");
    if forced {
        return Probe::new(true, "enabled (CLICOLOR_FORCE is set)");
    }
    if env.var("NO_COLOR").is_some_and(|value| !value.is_empty()) {
        return Probe::new(false, "disabled (NO_COLOR is set)");
    }
    if env.var("CLICOLOR").is_some_and(|value| value == "0") {
        return Probe::new(false, "disabled (CLICOLOR=0)");
    }
    if !stdout_terminal {
        return Probe::new(false, "disabled (stdout is not a terminal)");
    }
    let term = env.var("TERM");
    if term_disables_color(term.as_deref(), windows) {
        return if term.is_none() {
            Probe::new(false, "disabled (TERM is not set)")
        } else {
            Probe::new(false, "disabled (TERM=dumb)")
        };
    }
    Probe::new(true, "ANSI colors enabled")
}

/// True when `TERM` rules out color.
///
/// `dumb` always disables. An unset `TERM` disables on Unix (no capability
/// data), while on Windows TERM is usually unset and colors still work through
/// the console API.
fn term_disables_color(term: Option<&str>, windows: bool) -> bool {
    match term {
        Some(term) => term == "dumb",
        None => !windows,
    }
}

/// Probes emoji support, best-effort.
///
/// Emoji rendering cannot be detected programmatically — a terminal either
/// substitutes tofu boxes or it does not — so the probe reports likelihood and
/// `doctor` renders a sample line for visual confirmation.
#[must_use]
pub fn emoji_probe(env: &dyn Environment, stdout_terminal: bool) -> Probe {
    emoji_probe_impl(env, stdout_terminal, cfg!(windows))
}

fn emoji_probe_impl(env: &dyn Environment, stdout_terminal: bool, windows: bool) -> Probe {
    if env.var("TERM").as_deref() == Some("dumb") {
        return Probe::new(false, "emoji may not render (TERM=dumb)");
    }
    if !stdout_terminal {
        // Applies on every platform: without a terminal there is nothing to
        // render into, so no verdict is meaningful.
        return Probe::new(
            false,
            "emoji support not verifiable (stdout is not a terminal)",
        );
    }
    if windows {
        return if env.var("WT_SESSION").is_some() {
            Probe::new(true, "emoji supported (Windows Terminal)")
        } else {
            Probe::new(true, "emoji likely supported (Windows font fallback)")
        };
    }
    if utf8_locale(env) {
        Probe::new(true, "emoji likely supported (UTF-8 locale)")
    } else {
        Probe::new(false, "emoji may not render (no UTF-8 locale detected)")
    }
}

/// True when the locale environment indicates a UTF-8 encoding.
///
/// Locale variables take precedence `LC_ALL` > `LC_CTYPE` > `LANG`; the first
/// non-empty one decides.
fn utf8_locale(env: &dyn Environment) -> bool {
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Some(value) = env.var(key).filter(|value| !value.is_empty()) {
            let lower = value.to_ascii_lowercase();
            return lower.contains("utf-8") || lower.contains("utf8");
        }
    }
    false
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::environment::MapEnvironment;

    /// A tty-like environment with a color-capable TERM.
    fn tty_env() -> MapEnvironment {
        let mut env = MapEnvironment::new();
        env.terminals = true;
        env.with_var("TERM", "xterm-256color")
    }

    #[test]
    fn color_enabled_on_tty_with_term() {
        let probe = color_probe_impl(&tty_env(), false, true, false);
        assert!(probe.ok);
        assert_eq!(probe.detail, "ANSI colors enabled");
    }

    #[test]
    fn flag_disables_color_over_everything() {
        let env = tty_env().with_var("CLICOLOR_FORCE", "1");
        let probe = color_probe_impl(&env, true, true, false);
        assert!(!probe.ok);
        assert!(probe.detail.contains("--no-color"));
    }

    #[test]
    fn no_color_env_disables() {
        let env = tty_env().with_var("NO_COLOR", "1");
        let probe = color_probe_impl(&env, false, true, false);
        assert!(!probe.ok);
        assert!(probe.detail.contains("NO_COLOR"));
    }

    #[test]
    fn empty_no_color_env_is_ignored() {
        let env = tty_env().with_var("NO_COLOR", "");
        assert!(color_probe_impl(&env, false, true, false).ok);
    }

    #[test]
    fn clicolor_zero_disables() {
        let env = tty_env().with_var("CLICOLOR", "0");
        assert!(!color_probe_impl(&env, false, true, false).ok);
    }

    #[test]
    fn clicolor_force_beats_no_color_and_piping() {
        let env = MapEnvironment::new()
            .with_var("CLICOLOR_FORCE", "1")
            .with_var("NO_COLOR", "1");
        let probe = color_probe_impl(&env, false, false, false);
        assert!(probe.ok);
        assert!(probe.detail.contains("CLICOLOR_FORCE"));
    }

    #[test]
    fn clicolor_force_zero_or_empty_is_ignored() {
        let env = MapEnvironment::new().with_var("CLICOLOR_FORCE", "0");
        assert!(!color_probe_impl(&env, false, false, false).ok);
        let env = MapEnvironment::new().with_var("CLICOLOR_FORCE", "");
        assert!(!color_probe_impl(&env, false, false, false).ok);
    }

    #[test]
    fn dumb_term_disables() {
        let env = tty_env().with_var("TERM", "dumb");
        let probe = color_probe_impl(&env, false, true, false);
        assert!(!probe.ok);
        assert!(probe.detail.contains("TERM=dumb"));
    }

    #[test]
    fn unset_term_disables_on_unix_but_not_windows() {
        // Build a tty-like env without TERM: MapEnvironment defaults to no vars.
        let mut env = MapEnvironment::new();
        env.terminals = true;
        assert!(!color_probe_impl(&env, false, true, false).ok);
        assert!(color_probe_impl(&env, false, true, true).ok);
    }

    #[test]
    fn piped_stdout_disables_unless_forced() {
        assert!(!color_probe_impl(&tty_env(), false, false, false).ok);
    }

    #[test]
    fn paint_wraps_only_when_enabled() {
        assert_eq!(paint(true, SGR_GREEN, "ok"), "\x1b[32mok\x1b[0m");
        assert_eq!(paint(false, SGR_RED, "FAIL"), "FAIL");
    }

    #[test]
    fn semantic_yellow_is_bright_yellow_not_normal_yellow() {
        // Several common themes (e.g. GNOME Terminal's defaults) render the
        // normal-yellow slot (SGR 33) as brown/ochre; semantic warnings must
        // request the bright-yellow slot instead, still via the user palette.
        assert_eq!(SGR_YELLOW, "\x1b[93m");
        assert_eq!(paint(true, SGR_YELLOW, "skip"), "\x1b[93mskip\x1b[0m");
        assert!(!SGR_YELLOW.contains("\x1b[33m"));
    }

    #[test]
    fn emoji_supported_with_utf8_locale() {
        let env = tty_env().with_var("LANG", "en_US.UTF-8");
        let probe = emoji_probe_impl(&env, true, false);
        assert!(probe.ok);
        assert!(probe.detail.contains("UTF-8"));
    }

    #[test]
    fn emoji_utf8_detection_is_case_insensitive() {
        let env = tty_env().with_var("LC_ALL", "C.utf8");
        assert!(emoji_probe_impl(&env, true, false).ok);
    }

    #[test]
    fn locale_precedence_lc_all_wins() {
        let env = tty_env()
            .with_var("LC_ALL", "C")
            .with_var("LANG", "en_US.UTF-8");
        assert!(!emoji_probe_impl(&env, true, false).ok);
    }

    #[test]
    fn emoji_unsupported_without_utf8_locale() {
        let env = tty_env().with_var("LANG", "C");
        let probe = emoji_probe_impl(&env, true, false);
        assert!(!probe.ok);
        assert!(probe.detail.contains("no UTF-8 locale"));
    }

    #[test]
    fn emoji_dumb_term_disables_everywhere() {
        let env = tty_env()
            .with_var("TERM", "dumb")
            .with_var("LANG", "C.UTF-8");
        assert!(!emoji_probe_impl(&env, true, false).ok);
        assert!(!emoji_probe_impl(&env, true, true).ok);
    }

    #[test]
    fn emoji_piped_stdout_is_not_verifiable() {
        let env = tty_env().with_var("LANG", "en_US.UTF-8");
        let probe = emoji_probe_impl(&env, false, false);
        assert!(!probe.ok);
        assert!(probe.detail.contains("not verifiable"));
    }

    #[test]
    fn emoji_on_windows_uses_terminal_detection() {
        let env = tty_env();
        let probe = emoji_probe_impl(&env, true, true);
        assert!(probe.ok); // no WT_SESSION: font fallback still likely
        assert!(probe.detail.contains("Windows font fallback"));

        // Windows Terminal detected: reported even without a tty env,
        // but a piped stdout still short-circuits to "not verifiable".
        let env = tty_env().with_var("WT_SESSION", "some-session");
        let probe = emoji_probe_impl(&env, false, true);
        assert!(!probe.ok);
        assert!(probe.detail.contains("not verifiable"));

        let probe = emoji_probe_impl(&env, true, true);
        assert!(probe.ok);
        assert!(probe.detail.contains("Windows Terminal"));
    }

    #[test]
    fn probe_detail_never_contains_ansi() {
        let env = tty_env();
        for probe in [
            color_probe_impl(&env, false, true, false),
            color_probe_impl(&env, true, true, false),
            emoji_probe_impl(&env, true, false),
        ] {
            assert!(!probe.detail.contains('\x1b'));
        }
    }
}
