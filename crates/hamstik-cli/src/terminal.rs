// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Terminal capability detection, ANSI styling helpers, and the decoration
//! profile negotiated for one invocation.
//!
//! `doctor` uses these probes to report whether the running terminal can
//! render ANSI colors and emoji — the two decorations the CLI puts into human
//! output — so users can verify nothing renders mangled. Detection follows the
//! [`NO_COLOR`], [`CLICOLOR`], and `HAMSTIK_TERM` conventions. `--json` output
//! never contains ANSI codes (SPEC §39), which doctor guarantees by
//! construction: the sample line renders only in human mode.
//!
//! Two independent knobs are negotiated:
//!
//! * [`ColorMode`] — `--color=auto|always|never` (and the legacy `--no-color`)
//!   decide whether SGR color is emitted.
//! * [`TerminalProfile`] — `HAMSTIK_TERM=auto|unicode|ascii` decides whether
//!   Unicode decoration, emoji, and terminal-relative width are used. It lets
//!   CI pin a byte-stable ASCII rendering independent of the host terminal.
//!
//! [`NO_COLOR`]: https://no-color.org/
//! [`CLICOLOR`]: https://bixense.com/clicolors/

use crate::environment::Environment;

/// How colored output is negotiated for this invocation.
///
/// Precedence is `flag > environment > auto` (see [`color_probe`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// Honor the environment and terminal detection (the default).
    Auto,
    /// Force ANSI color on, even when stdout is not a terminal.
    Always,
    /// Disable ANSI color, even on a capable terminal.
    Never,
}

/// The decoration profile selected by `HAMSTIK_TERM`.
///
/// Unlike [`ColorMode`], the profile is environment-only: it has no command-
/// line counterpart, so a script can pin it once for a whole CI job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalProfile {
    /// Detect capabilities from the environment (the default).
    Auto,
    /// Assume a Unicode-capable terminal: Unicode decoration and emoji are
    /// used even where auto-detection would withhold them.
    Unicode,
    /// Assume a minimal ASCII terminal: decoration uses ASCII stand-ins, emoji
    /// are suppressed, and width-sensitive layout uses a fixed,
    /// terminal-independent width so captured logs are byte-stable.
    Ascii,
}

impl TerminalProfile {
    /// Resolves the profile from `HAMSTIK_TERM`.
    ///
    /// Accepted values are case-insensitive and trimmed: `auto` (also unset
    /// or empty), `unicode` (aliases `utf8`, `utf-8`), and `ascii` (alias
    /// `plain`). Any other value falls back to [`TerminalProfile::Auto`] so a
    /// typo cannot silently break output.
    #[must_use]
    pub fn from_env(env: &dyn Environment) -> Self {
        match env
            .var("HAMSTIK_TERM")
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("unicode" | "utf8" | "utf-8") => Self::Unicode,
            Some("ascii" | "plain") => Self::Ascii,
            _ => Self::Auto,
        }
    }
}

/// Whether Unicode decoration may be used under `profile`.
#[must_use]
pub fn unicode_enabled(profile: TerminalProfile) -> bool {
    !matches!(profile, TerminalProfile::Ascii)
}

/// ASCII/Unicode decoration glyphs chosen by the active terminal profile.
///
/// Centralizing the stand-ins keeps `HAMSTIK_TERM=ascii` output free of
/// non-ASCII bytes wherever the CLI decorates human output, without touching
/// server-provided content (which is rendered verbatim).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyphs {
    unicode: bool,
}

impl Glyphs {
    /// Builds the glyph set for the given Unicode decision.
    #[must_use]
    pub fn new(unicode: bool) -> Self {
        Self { unicode }
    }

    /// Builds the glyph set for `profile`.
    #[must_use]
    pub fn for_profile(profile: TerminalProfile) -> Self {
        Self::new(unicode_enabled(profile))
    }

    /// True when Unicode glyphs are selected.
    #[must_use]
    pub fn is_unicode(self) -> bool {
        self.unicode
    }

    /// Em dash separator (`-` in ASCII).
    #[must_use]
    pub fn em_dash(self) -> &'static str {
        if self.unicode { "\u{2014}" } else { "-" }
    }

    /// Middle-dot separator (`|` in ASCII).
    #[must_use]
    pub fn middle_dot(self) -> &'static str {
        if self.unicode { "\u{00b7}" } else { "|" }
    }

    /// Right arrow (`->` in ASCII).
    #[must_use]
    pub fn arrow(self) -> &'static str {
        if self.unicode { "\u{2192}" } else { "->" }
    }

    /// Horizontal ellipsis (`...` in ASCII).
    #[must_use]
    pub fn ellipsis(self) -> &'static str {
        if self.unicode { "\u{2026}" } else { "..." }
    }

    /// Two-cell swatch fill (`##` in ASCII; the bracket frame is unchanged).
    #[must_use]
    pub fn swatch_fill(self) -> &'static str {
        if self.unicode {
            "\u{2588}\u{2588}"
        } else {
            "##"
        }
    }

    /// Copyright sign (`(c)` in ASCII).
    #[must_use]
    pub fn copyright(self) -> &'static str {
        if self.unicode { "\u{00a9}" } else { "(c)" }
    }
}

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

/// Probes ANSI color support for stdout, honoring the negotiated [`ColorMode`].
///
/// Precedence is `flag > environment > auto`:
///
/// 1. an explicit `--color=always`/`--color=never` (or `--no-color`) wins;
/// 2. `HAMSTIK_NO_COLOR`, then `CLICOLOR_FORCE` (force-enable), then
///    `NO_COLOR`, then `CLICOLOR=0` decide;
/// 3. otherwise the terminal is probed: a non-terminal stdout and an unset or
///    `dumb` `TERM` disable color.
///
/// `HAMSTIK_NO_COLOR` outranks `CLICOLOR_FORCE` because it is the
/// Hamstik-specific opt-out; `NO_COLOR` keeps its historical position below
/// `CLICOLOR_FORCE` so existing auto-detection is unchanged.
#[must_use]
pub fn color_probe(env: &dyn Environment, mode: ColorMode, stdout_terminal: bool) -> Probe {
    color_probe_impl(env, mode, stdout_terminal, cfg!(windows))
}

fn color_probe_impl(
    env: &dyn Environment,
    mode: ColorMode,
    stdout_terminal: bool,
    windows: bool,
) -> Probe {
    match mode {
        ColorMode::Never => return Probe::new(false, "disabled (--no-color/--color=never)"),
        ColorMode::Always => return Probe::new(true, "enabled (--color=always)"),
        ColorMode::Auto => {}
    }
    if env
        .var("HAMSTIK_NO_COLOR")
        .is_some_and(|value| !value.is_empty())
    {
        return Probe::new(false, "disabled (HAMSTIK_NO_COLOR is set)");
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

/// Probes emoji support, best-effort, honoring the terminal profile.
///
/// Emoji rendering cannot be detected programmatically — a terminal either
/// substitutes tofu boxes or it does not — so the probe reports likelihood and
/// `doctor` renders a sample line for visual confirmation. An explicit
/// `HAMSTIK_TERM=unicode|ascii` short-circuits the heuristic so CI can pin the
/// decision.
#[must_use]
pub fn emoji_probe(
    env: &dyn Environment,
    stdout_terminal: bool,
    profile: TerminalProfile,
) -> Probe {
    emoji_probe_impl(env, stdout_terminal, cfg!(windows), profile)
}

fn emoji_probe_impl(
    env: &dyn Environment,
    stdout_terminal: bool,
    windows: bool,
    profile: TerminalProfile,
) -> Probe {
    match profile {
        TerminalProfile::Ascii => {
            return Probe::new(false, "disabled (HAMSTIK_TERM=ascii)");
        }
        TerminalProfile::Unicode => {
            return Probe::new(true, "enabled (HAMSTIK_TERM=unicode)");
        }
        TerminalProfile::Auto => {}
    }
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
        let probe = color_probe_impl(&tty_env(), ColorMode::Auto, true, false);
        assert!(probe.ok);
        assert_eq!(probe.detail, "ANSI colors enabled");
    }

    #[test]
    fn flag_never_disables_color_over_everything() {
        let env = tty_env().with_var("CLICOLOR_FORCE", "1");
        let probe = color_probe_impl(&env, ColorMode::Never, true, false);
        assert!(!probe.ok);
        assert!(probe.detail.contains("--no-color"));
        assert!(probe.detail.contains("--color=never"));
    }

    #[test]
    fn flag_always_enables_color_over_no_color_and_piping() {
        let env = MapEnvironment::new()
            .with_var("NO_COLOR", "1")
            .with_var("HAMSTIK_NO_COLOR", "1");
        let probe = color_probe_impl(&env, ColorMode::Always, false, false);
        assert!(probe.ok);
        assert!(probe.detail.contains("--color=always"));
    }

    #[test]
    fn hamstik_no_color_disables() {
        let env = tty_env().with_var("HAMSTIK_NO_COLOR", "1");
        let probe = color_probe_impl(&env, ColorMode::Auto, true, false);
        assert!(!probe.ok);
        assert!(probe.detail.contains("HAMSTIK_NO_COLOR"));
    }

    #[test]
    fn empty_hamstik_no_color_env_is_ignored() {
        let env = tty_env().with_var("HAMSTIK_NO_COLOR", "");
        assert!(color_probe_impl(&env, ColorMode::Auto, true, false).ok);
    }

    #[test]
    fn hamstik_no_color_beats_clicolor_force() {
        // The Hamstik-specific opt-out outranks the generic force variable.
        let env = tty_env()
            .with_var("HAMSTIK_NO_COLOR", "1")
            .with_var("CLICOLOR_FORCE", "1");
        let probe = color_probe_impl(&env, ColorMode::Auto, true, false);
        assert!(!probe.ok);
        assert!(probe.detail.contains("HAMSTIK_NO_COLOR"));
    }

    #[test]
    fn no_color_env_disables() {
        let env = tty_env().with_var("NO_COLOR", "1");
        let probe = color_probe_impl(&env, ColorMode::Auto, true, false);
        assert!(!probe.ok);
        assert!(probe.detail.contains("NO_COLOR"));
    }

    #[test]
    fn empty_no_color_env_is_ignored() {
        let env = tty_env().with_var("NO_COLOR", "");
        assert!(color_probe_impl(&env, ColorMode::Auto, true, false).ok);
    }

    #[test]
    fn clicolor_zero_disables() {
        let env = tty_env().with_var("CLICOLOR", "0");
        assert!(!color_probe_impl(&env, ColorMode::Auto, true, false).ok);
    }

    #[test]
    fn clicolor_force_beats_no_color_and_piping() {
        let env = MapEnvironment::new()
            .with_var("CLICOLOR_FORCE", "1")
            .with_var("NO_COLOR", "1");
        let probe = color_probe_impl(&env, ColorMode::Auto, false, false);
        assert!(probe.ok);
        assert!(probe.detail.contains("CLICOLOR_FORCE"));
    }

    #[test]
    fn clicolor_force_zero_or_empty_is_ignored() {
        let env = MapEnvironment::new().with_var("CLICOLOR_FORCE", "0");
        assert!(!color_probe_impl(&env, ColorMode::Auto, false, false).ok);
        let env = MapEnvironment::new().with_var("CLICOLOR_FORCE", "");
        assert!(!color_probe_impl(&env, ColorMode::Auto, false, false).ok);
    }

    #[test]
    fn dumb_term_disables() {
        let env = tty_env().with_var("TERM", "dumb");
        let probe = color_probe_impl(&env, ColorMode::Auto, true, false);
        assert!(!probe.ok);
        assert!(probe.detail.contains("TERM=dumb"));
    }

    #[test]
    fn unset_term_disables_on_unix_but_not_windows() {
        // Build a tty-like env without TERM: MapEnvironment defaults to no vars.
        let mut env = MapEnvironment::new();
        env.terminals = true;
        assert!(!color_probe_impl(&env, ColorMode::Auto, true, false).ok);
        assert!(color_probe_impl(&env, ColorMode::Auto, true, true).ok);
    }

    #[test]
    fn piped_stdout_disables_unless_forced() {
        assert!(!color_probe_impl(&tty_env(), ColorMode::Auto, false, false).ok);
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
        let probe = emoji_probe_impl(&env, true, false, TerminalProfile::Auto);
        assert!(probe.ok);
        assert!(probe.detail.contains("UTF-8"));
    }

    #[test]
    fn emoji_utf8_detection_is_case_insensitive() {
        let env = tty_env().with_var("LC_ALL", "C.utf8");
        assert!(emoji_probe_impl(&env, true, false, TerminalProfile::Auto).ok);
    }

    #[test]
    fn locale_precedence_lc_all_wins() {
        let env = tty_env()
            .with_var("LC_ALL", "C")
            .with_var("LANG", "en_US.UTF-8");
        assert!(!emoji_probe_impl(&env, true, false, TerminalProfile::Auto).ok);
    }

    #[test]
    fn emoji_unsupported_without_utf8_locale() {
        let env = tty_env().with_var("LANG", "C");
        let probe = emoji_probe_impl(&env, true, false, TerminalProfile::Auto);
        assert!(!probe.ok);
        assert!(probe.detail.contains("no UTF-8 locale"));
    }

    #[test]
    fn emoji_dumb_term_disables_everywhere() {
        let env = tty_env()
            .with_var("TERM", "dumb")
            .with_var("LANG", "C.UTF-8");
        assert!(!emoji_probe_impl(&env, true, false, TerminalProfile::Auto).ok);
        assert!(!emoji_probe_impl(&env, true, true, TerminalProfile::Auto).ok);
    }

    #[test]
    fn emoji_piped_stdout_is_not_verifiable() {
        let env = tty_env().with_var("LANG", "en_US.UTF-8");
        let probe = emoji_probe_impl(&env, false, false, TerminalProfile::Auto);
        assert!(!probe.ok);
        assert!(probe.detail.contains("not verifiable"));
    }

    #[test]
    fn emoji_on_windows_uses_terminal_detection() {
        let env = tty_env();
        let probe = emoji_probe_impl(&env, true, true, TerminalProfile::Auto);
        assert!(probe.ok); // no WT_SESSION: font fallback still likely
        assert!(probe.detail.contains("Windows font fallback"));

        // Windows Terminal detected: reported even without a tty env,
        // but a piped stdout still short-circuits to "not verifiable".
        let env = tty_env().with_var("WT_SESSION", "some-session");
        let probe = emoji_probe_impl(&env, false, true, TerminalProfile::Auto);
        assert!(!probe.ok);
        assert!(probe.detail.contains("not verifiable"));

        let probe = emoji_probe_impl(&env, true, true, TerminalProfile::Auto);
        assert!(probe.ok);
        assert!(probe.detail.contains("Windows Terminal"));
    }

    #[test]
    fn terminal_profile_from_env_parses_known_values() {
        assert_eq!(
            TerminalProfile::from_env(&MapEnvironment::new()),
            TerminalProfile::Auto
        );
        for (value, expected) in [
            ("auto", TerminalProfile::Auto),
            (" Unicode ", TerminalProfile::Unicode),
            ("utf8", TerminalProfile::Unicode),
            ("UTF-8", TerminalProfile::Unicode),
            ("ascii", TerminalProfile::Ascii),
            ("plain", TerminalProfile::Ascii),
            ("bogus", TerminalProfile::Auto),
        ] {
            let env = MapEnvironment::new().with_var("HAMSTIK_TERM", value);
            assert_eq!(TerminalProfile::from_env(&env), expected, "value {value:?}");
        }
    }

    #[test]
    fn ascii_profile_disables_unicode_and_emoji() {
        assert!(!unicode_enabled(TerminalProfile::Ascii));
        let env = tty_env().with_var("LANG", "en_US.UTF-8");
        let probe = emoji_probe_impl(&env, true, false, TerminalProfile::Ascii);
        assert!(!probe.ok);
        assert!(probe.detail.contains("HAMSTIK_TERM=ascii"));
    }

    #[test]
    fn unicode_profile_forces_emoji_even_without_locale_or_tty() {
        let env = MapEnvironment::new().with_var("TERM", "dumb");
        let probe = emoji_probe_impl(&env, false, false, TerminalProfile::Unicode);
        assert!(probe.ok);
        assert!(probe.detail.contains("HAMSTIK_TERM=unicode"));
        assert!(unicode_enabled(TerminalProfile::Unicode));
        assert!(unicode_enabled(TerminalProfile::Auto));
    }

    #[test]
    fn glyphs_fall_back_to_ascii_for_the_ascii_profile() {
        let ascii = Glyphs::for_profile(TerminalProfile::Ascii);
        assert!(!ascii.is_unicode());
        assert_eq!(ascii.em_dash(), "-");
        assert_eq!(ascii.middle_dot(), "|");
        assert_eq!(ascii.arrow(), "->");
        assert_eq!(ascii.ellipsis(), "...");
        assert_eq!(ascii.swatch_fill(), "##");
        assert_eq!(ascii.copyright(), "(c)");
        for glyph in [
            ascii.em_dash(),
            ascii.middle_dot(),
            ascii.arrow(),
            ascii.ellipsis(),
            ascii.swatch_fill(),
            ascii.copyright(),
        ] {
            assert!(glyph.is_ascii(), "glyph {glyph:?} must be ASCII");
        }

        let unicode = Glyphs::for_profile(TerminalProfile::Unicode);
        assert!(unicode.is_unicode());
        assert_eq!(unicode.em_dash(), "\u{2014}");
        assert_eq!(unicode.middle_dot(), "\u{00b7}");
        assert_eq!(unicode.arrow(), "\u{2192}");
        assert_eq!(unicode.ellipsis(), "\u{2026}");
        assert_eq!(unicode.swatch_fill(), "\u{2588}\u{2588}");
        assert_eq!(unicode.copyright(), "\u{00a9}");
    }

    #[test]
    fn probe_detail_never_contains_ansi() {
        let env = tty_env();
        for probe in [
            color_probe_impl(&env, ColorMode::Auto, true, false),
            color_probe_impl(&env, ColorMode::Never, true, false),
            color_probe_impl(&env, ColorMode::Always, true, false),
            emoji_probe_impl(&env, true, false, TerminalProfile::Auto),
        ] {
            assert!(!probe.detail.contains('\x1b'));
        }
    }
}
