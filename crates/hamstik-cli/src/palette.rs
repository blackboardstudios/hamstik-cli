// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Hex-color swatch rendering for human output.
//!
//! Lists that carry a color (projects, labels) render a small bracketed
//! swatch — `[██]` — with the block glyphs painted in the resource's actual
//! color. The frame keeps the swatch legible even when the fill is invisible
//! on the terminal's background (black-on-black, white-on-white); the hex text
//! beside it always renders in plain foreground so the exact value stays
//! readable. Swatches are decoration: `--json`, `--quiet`, and color-disabled
//! output show the plain hex text instead.
//!
//! The block glyph U+2588 is drawn in the *foreground* color, so the swatch
//! sets the foreground to the resource color. The background is set to the
//! same color so terminals that render block glyphs with font seams render a
//! solid rectangle. Under the ASCII terminal profile
//! (`HAMSTIK_TERM=ascii`) the fill is `##` instead, so captured logs stay free
//! of non-ASCII bytes.

use crate::terminal::Glyphs;

/// Parses `#RRGGBB` / `RRGGBB` into RGB components.
#[must_use]
pub fn parse_hex(color: &str) -> Option<(u8, u8, u8)> {
    let hex = color.trim().strip_prefix('#').unwrap_or(color);
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

/// SGR sequence painting both the foreground (the block glyphs) and the
/// background (fills font seams) with the color's true RGB.
#[must_use]
pub fn swatch_sgr(color: &str) -> Option<String> {
    let (r, g, b) = parse_hex(color)?;
    Some(format!("\x1b[38;2;{r};{g};{b};48;2;{r};{g};{b}m"))
}

/// The closest xterm-256 palette index for a hex color (downsampling without
/// the full cube walk; good enough for a 2-cell swatch).
#[must_use]
pub fn nearest_256_index(color: &str) -> Option<u8> {
    let (r, g, b) = parse_hex(color)?;
    // Grays map onto the 24-step grayscale ramp (232..=255) when the channel
    // spread is small; otherwise the 6x6x6 cube (16..=231).
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max - min < 10 {
        let step = (r as u16 * 23 + 127) / 255; // 0..=23
        return Some((232 + step) as u8);
    }
    let index = 16
        + 36 * ((r as u16 * 5 + 127) / 255)
        + 6 * ((g as u16 * 5 + 127) / 255)
        + ((b as u16 * 5 + 127) / 255);
    Some(index as u8)
}

/// Renders a two-cell filled swatch with a bracket frame:
/// `[██]` in both modes (the frame keeps extreme fills legible).
///
/// When color is disabled the fill renders as plain blocks so the cell's
/// visible width is identical to the colored variant, keeping table columns
/// aligned across environments.
#[must_use]
pub fn swatch(color_enabled: bool, color: &str, glyphs: Glyphs) -> String {
    let fill = glyphs.swatch_fill();
    if !color_enabled || parse_hex(color).is_none() {
        return format!("[{fill}]");
    }
    render_truecolor(color, fill)
}

#[must_use]
fn render_truecolor(color: &str, fill: &str) -> String {
    let sgr = swatch_sgr(color).unwrap_or_default();
    format!("[{sgr}{fill}\x1b[0m]")
}

/// Renders a bracketed swatch using the 256-color palette (same foreground +
/// background pairing as the truecolor variant).
#[must_use]
pub fn swatch_256(color_enabled: bool, color: &str, glyphs: Glyphs) -> String {
    let fill = glyphs.swatch_fill();
    if !color_enabled {
        return format!("[{fill}]");
    }
    match nearest_256_index(color) {
        Some(index) => format!("[\x1b[38;5;{index};48;5;{index}m{fill}\x1b[0m]"),
        None => format!("[{fill}]"),
    }
}

/// The display cell for a color value: swatch followed by the plain hex text.
#[must_use]
pub fn color_cell(
    color_enabled: bool,
    color: &str,
    supports_truecolor: bool,
    glyphs: Glyphs,
) -> String {
    let swatch = if supports_truecolor {
        swatch(color_enabled, color, glyphs)
    } else {
        swatch_256(color_enabled, color, glyphs)
    };
    format!("{swatch} {color}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::terminal::TerminalProfile;

    /// Unicode glyphs (the default/auto profile).
    fn uni() -> Glyphs {
        Glyphs::for_profile(TerminalProfile::Unicode)
    }

    /// ASCII glyphs (`HAMSTIK_TERM=ascii`).
    fn ascii() -> Glyphs {
        Glyphs::for_profile(TerminalProfile::Ascii)
    }

    #[test]
    fn parses_six_digit_hex_with_optional_hash() {
        assert_eq!(parse_hex("#6366f1"), Some((0x63, 0x66, 0xf1)));
        assert_eq!(parse_hex("F97316"), Some((0xf9, 0x73, 0x16)));
        assert_eq!(parse_hex("#000000"), Some((0, 0, 0)));
        assert_eq!(parse_hex(""), None);
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#12345g"), None);
        assert_eq!(parse_hex("zzzzzz"), None);
    }

    #[test]
    fn swatch_sgr_builds_foreground_and_background_sequence() {
        assert_eq!(
            swatch_sgr("#6366f1").as_deref(),
            Some("\x1b[38;2;99;102;241;48;2;99;102;241m")
        );
        assert_eq!(swatch_sgr("nope"), None);
    }

    #[test]
    fn nearest_256_index_downsamples_channels() {
        // Pure red -> cube corner 196.
        assert_eq!(nearest_256_index("#ff0000"), Some(196));
        // Pure blue -> cube corner 21.
        assert_eq!(nearest_256_index("#0000ff"), Some(21));
        // Mid gray -> grayscale ramp.
        assert_eq!(nearest_256_index("#808080"), Some(244));
        // Unparseable input falls back to None.
        assert_eq!(nearest_256_index("nope"), None);
    }

    #[test]
    fn swatch_without_color_shows_plain_frame() {
        let cell = swatch(false, "#6366f1", uni());
        assert_eq!(cell, "[██]");
        assert!(!cell.contains('\u{1b}'));
    }

    #[test]
    fn swatch_with_color_wraps_fill_in_foreground_and_background() {
        let cell = swatch(true, "#6366f1", uni());
        assert!(cell.starts_with("[\x1b[38;2;99;102;241;48;2;99;102;241m"));
        assert!(cell.ends_with("\x1b[0m]"));
        assert!(cell.contains('\u{2588}'));
    }

    #[test]
    fn swatch_256_uses_palette_foreground_and_background() {
        let cell = swatch_256(true, "#ff0000", uni());
        assert_eq!(cell, "[\x1b[38;5;196;48;5;196m██\x1b[0m]");
    }

    #[test]
    fn swatch_256_falls_back_to_plain_frame_on_bad_color() {
        let cell = swatch_256(true, "notacolor", uni());
        assert_eq!(cell, "[██]");
    }

    #[test]
    fn black_swatch_keeps_extent_via_frame() {
        // The whole point of the frame: fill may be invisible but the cell
        // width and brackets stay legible.
        let cell = swatch(true, "#000000", uni());
        assert!(cell.starts_with('['));
        assert!(cell.ends_with(']'));
        assert_eq!(cell.chars().filter(|c| *c == '\u{2588}').count(), 2);
    }

    #[test]
    fn ascii_profile_uses_hash_fill() {
        let cell = swatch(false, "#6366f1", ascii());
        assert_eq!(cell, "[##]");
        assert!(cell.is_ascii());
        let colored = swatch_256(true, "#ff0000", ascii());
        assert_eq!(colored, "[\x1b[38;5;196;48;5;196m##\x1b[0m]");
    }

    #[test]
    fn color_cell_pairs_swatch_with_hex() {
        let cell = color_cell(true, "#f97316", true, uni());
        assert!(cell.contains("#f97316"));
        assert!(cell.contains('\u{2588}'));
        let plain = color_cell(false, "#f97316", true, uni());
        assert!(!plain.contains('\u{1b}'));
        assert_eq!(plain, "[██] #f97316");
    }

    #[test]
    fn swatch_width_is_stable_across_modes() {
        // Alignment depends on the visible width being identical: 4 cells
        // (bracket, 2 blocks, bracket) plus the hex text in both modes.
        let colored = color_cell(true, "#6366f1", true, uni());
        let plain = color_cell(false, "#6366f1", true, uni());
        assert_eq!(strip_ansi_len(&colored), strip_ansi_len(&plain));
    }

    #[test]
    fn ascii_swatch_width_matches_unicode_swatch_width() {
        // The ASCII stand-in must keep the same two-cell fill width so table
        // alignment does not shift when the profile changes.
        let unicode = color_cell(false, "#6366f1", true, uni());
        let ascii = color_cell(false, "#6366f1", true, ascii());
        assert_eq!(strip_ansi_len(&unicode), strip_ansi_len(&ascii));
    }

    /// Counts characters outside SGR sequences (approximate visible width).
    fn strip_ansi_len(text: &str) -> usize {
        let mut count = 0;
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for next in chars.by_ref() {
                    if next == 'm' {
                        break;
                    }
                }
            } else {
                count += 1;
            }
        }
        count
    }
}
