// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Inline terminal image protocols (CLI-40).
//!
//! Detects an image-capable terminal from capability markers in the process
//! environment and encodes downloaded image bytes into the corresponding
//! escape-sequence payload. Detection is deliberately conservative: an unknown
//! terminal yields [`None`], and the caller falls back to the documented file
//! download behavior rather than emitting raw bytes into a text stream.
//!
//! No escaping or `--jq` filtering applies to these payloads; they are only
//! written on the human, TTY, non-structured path (see the `attachment view`
//! command).

use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::environment::Environment;

/// The inline image protocol selected for the current terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Protocol {
    /// Kitty graphics protocol (`ESC_G`), also implemented by Ghostty.
    Kitty,
    /// iTerm2 inline images (`OSC 1337;File`), also implemented by WezTerm
    /// and mintty.
    Iterm2,
    /// SIXEL (`DCS ... q`), encoded in pure Rust by `icy_sixel`.
    Sixel,
}

impl Protocol {
    /// The stable name used in diagnostics and the `HAMSTIK_IMAGE_PROTOCOL`
    /// override.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Kitty => "kitty",
            Self::Iterm2 => "iterm2",
            Self::Sixel => "sixel",
        }
    }
}

/// Returns the inline protocol the current terminal advertises, if any.
///
/// Detection is capability-based:
///
/// * `HAMSTIK_IMAGE_PROTOCOL` is an explicit override (also the escape hatch
///   for terminals whose support is not advertised through the environment).
///   `none`/`off` disables inline preview; an unrecognized value is treated
///   the same as no support.
/// * Kitty: `KITTY_WINDOW_ID`, a `TERM`/`TERM_PROGRAM` marker for kitty or
///   Ghostty.
/// * iTerm2: `TERM_PROGRAM=iTerm.app|WezTerm|mintty` or `LC_TERMINAL=iTerm2`.
/// * SIXEL: a `sixel` marker in `TERM`, or the foot/mlterm terminal programs.
///
/// This never guesses from the operating system or from an unrelated terminal
/// name; an unknown environment returns `None`.
pub(crate) fn detect(env: &dyn Environment) -> Option<Protocol> {
    if let Some(forced) = env.var("HAMSTIK_IMAGE_PROTOCOL") {
        return match forced.trim().to_ascii_lowercase().as_str() {
            "kitty" => Some(Protocol::Kitty),
            "iterm2" | "iterm" => Some(Protocol::Iterm2),
            "sixel" => Some(Protocol::Sixel),
            _ => None,
        };
    }

    let term = env.var("TERM").unwrap_or_default().to_ascii_lowercase();
    let term_program = env.var("TERM_PROGRAM").unwrap_or_default();

    if env.var("KITTY_WINDOW_ID").is_some()
        || term.contains("kitty")
        || term.contains("ghostty")
        || term_program.eq_ignore_ascii_case("ghostty")
    {
        return Some(Protocol::Kitty);
    }

    if matches!(term_program.as_str(), "iTerm.app" | "WezTerm" | "mintty")
        || env
            .var("LC_TERMINAL")
            .is_some_and(|value| value.eq_ignore_ascii_case("iTerm2"))
    {
        return Some(Protocol::Iterm2);
    }

    if term.contains("sixel")
        || term_program.eq_ignore_ascii_case("foot")
        || term.eq_ignore_ascii_case("mlterm")
    {
        return Some(Protocol::Sixel);
    }

    None
}

/// True when the declared content type (or file extension) is an image.
///
/// The content type is authoritative when present; the file name is only used
/// as a fallback for servers that omit or under-specify `Content-Type`.
pub(crate) fn is_image(content_type: Option<&str>, file_name: Option<&str>) -> bool {
    if let Some(value) = content_type {
        let mime = value
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        // The declared content type is authoritative: an attachment the
        // server does not declare as `image/*` is never rendered inline,
        // whatever its file extension suggests.
        return mime.starts_with("image/");
    }
    let Some(name) = file_name else {
        return false;
    };
    let extension = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
    )
}

/// Encodes `bytes` for `protocol`, returning the exact bytes to write to a TTY.
///
/// Decoding is required for every protocol: Kitty and iTerm2 only accept PNG
/// on the wire, and SIXEL needs pixel data. A decode or encode failure is a
/// soft error so the caller can fall back to a file download.
pub(crate) fn render(
    protocol: Protocol,
    bytes: &[u8],
    file_name: Option<&str>,
) -> Result<Vec<u8>, String> {
    let image =
        image::load_from_memory(bytes).map_err(|err| format!("cannot decode image: {err}"))?;
    match protocol {
        Protocol::Kitty => Ok(encode_kitty(&encode_png(&image)?)),
        Protocol::Iterm2 => Ok(encode_iterm2(&encode_png(&image)?, file_name)),
        Protocol::Sixel => encode_sixel(&image),
    }
}

/// Re-encodes a decoded image as PNG for the Kitty and iTerm2 protocols.
fn encode_png(image: &image::DynamicImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .map_err(|err| format!("cannot encode PNG: {err}"))?;
    Ok(out)
}

/// Kitty graphics protocol: base64 PNG, chunked at 4096 bytes.
///
/// `q=2` suppresses the terminal's success response so it can never leak into
/// stdout. Control data precedes the `;`, payload follows, and every chunk is
/// terminated with ST.
fn encode_kitty(png: &[u8]) -> Vec<u8> {
    const CHUNK: usize = 4096;
    let payload = BASE64.encode(png).into_bytes();
    let mut out = Vec::with_capacity(payload.len() + 64);
    if payload.is_empty() {
        out.extend_from_slice(b"\x1b_Ga=T,f=100,q=2,m=0;\x1b\\");
        return out;
    }
    let mut offset = 0;
    let mut first = true;
    while offset < payload.len() {
        let end = (offset + CHUNK).min(payload.len());
        let more = u8::from(end < payload.len());
        if first {
            out.extend_from_slice(format!("\x1b_Ga=T,f=100,q=2,m={more};").as_bytes());
            first = false;
        } else {
            out.extend_from_slice(format!("\x1b_Gm={more};").as_bytes());
        }
        out.extend_from_slice(&payload[offset..end]);
        out.extend_from_slice(b"\x1b\\");
        offset = end;
    }
    out
}

/// iTerm2 inline image protocol: `OSC 1337;File=<args>:<base64> BEL`.
fn encode_iterm2(png: &[u8], file_name: Option<&str>) -> Vec<u8> {
    let mut args = format!("size={};inline=1", png.len());
    if let Some(name) = file_name {
        args = format!("name={};{args}", BASE64.encode(name.as_bytes()));
    }
    let mut out = Vec::with_capacity(png.len() * 2 + 64);
    out.extend_from_slice(b"\x1b]1337;File=");
    out.extend_from_slice(args.as_bytes());
    out.push(b':');
    out.extend_from_slice(BASE64.encode(png).as_bytes());
    out.push(0x07);
    out
}

/// SIXEL: a complete DCS payload produced by the pure-Rust encoder.
fn encode_sixel(image: &image::DynamicImage) -> Result<Vec<u8>, String> {
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let encoded = icy_sixel::sixel_encode(
        rgba.as_raw(),
        width as usize,
        height as usize,
        &icy_sixel::EncodeOptions::default(),
    )
    .map_err(|err| format!("cannot encode SIXEL: {err}"))?;
    Ok(encoded.into_bytes())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::environment::MapEnvironment;

    #[test]
    fn detects_kitty_from_capability_marker() {
        let env = MapEnvironment::new().with_var("KITTY_WINDOW_ID", "1");
        assert_eq!(detect(&env), Some(Protocol::Kitty));
    }

    #[test]
    fn detects_iterm2_from_term_program() {
        let env = MapEnvironment::new().with_var("TERM_PROGRAM", "iTerm.app");
        assert_eq!(detect(&env), Some(Protocol::Iterm2));
    }

    #[test]
    fn detects_sixel_from_term_marker() {
        let env = MapEnvironment::new().with_var("TERM", "xterm-sixel");
        assert_eq!(detect(&env), Some(Protocol::Sixel));
    }

    #[test]
    fn explicit_override_wins_and_can_disable() {
        let env = MapEnvironment::new()
            .with_var("KITTY_WINDOW_ID", "1")
            .with_var("HAMSTIK_IMAGE_PROTOCOL", "sixel");
        assert_eq!(detect(&env), Some(Protocol::Sixel));
        let off = MapEnvironment::new()
            .with_var("KITTY_WINDOW_ID", "1")
            .with_var("HAMSTIK_IMAGE_PROTOCOL", "none");
        assert_eq!(detect(&off), None);
    }

    #[test]
    fn unknown_terminal_is_not_guessed() {
        let env = MapEnvironment::new()
            .with_var("TERM", "xterm-256color")
            .with_var("TERM_PROGRAM", "Apple_Terminal");
        assert_eq!(detect(&env), None);
    }

    #[test]
    fn image_detection_prefers_declared_content_type() {
        assert!(is_image(Some("image/png; charset=binary"), None));
        assert!(is_image(Some("IMAGE/JPEG"), None));
        assert!(is_image(None, Some("photo.JPEG")));
        assert!(!is_image(
            Some("application/octet-stream"),
            Some("notes.txt")
        ));
        // A non-image content type is authoritative even over an image
        // file extension (CLI-40 scope: image content types only).
        assert!(!is_image(
            Some("application/octet-stream"),
            Some("photo.png")
        ));
        assert!(!is_image(None, None));
    }

    fn tiny_png() -> Vec<u8> {
        // 1x1 opaque red PNG.
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn kitty_payload_is_base64_chunked_and_terminated() {
        let encoded = render(Protocol::Kitty, &tiny_png(), None).unwrap();
        let text = String::from_utf8(encoded).unwrap();
        assert!(text.starts_with("\x1b_Ga=T,f=100,q=2,m=0;"));
        assert!(text.ends_with("\x1b\\"));
        // The payload round-trips as base64 after the control data.
        let payload = text
            .trim_start_matches("\x1b_Ga=T,f=100,q=2,m=0;")
            .trim_end_matches("\x1b\\");
        let decoded = BASE64.decode(payload).unwrap();
        assert_eq!(image::load_from_memory(&decoded).unwrap().width(), 1);
    }

    #[test]
    fn iterm2_payload_carries_name_and_inline_flag() {
        let encoded = render(Protocol::Iterm2, &tiny_png(), Some("design.png")).unwrap();
        let text = String::from_utf8(encoded).unwrap();
        assert!(text.starts_with("\x1b]1337;File="));
        assert!(text.contains("inline=1"));
        assert!(text.contains(&format!("name={}", BASE64.encode("design.png"))));
        assert!(text.ends_with('\x07'));
    }

    #[test]
    fn sixel_payload_is_wrapped_in_dcs() {
        let encoded = render(Protocol::Sixel, &tiny_png(), None).unwrap();
        let text = String::from_utf8(encoded).unwrap();
        assert!(text.starts_with("\x1bP"));
        assert!(text.ends_with("\x1b\\"));
    }

    #[test]
    fn non_image_bytes_are_a_soft_error() {
        assert!(render(Protocol::Kitty, b"not an image", None).is_err());
    }
}
