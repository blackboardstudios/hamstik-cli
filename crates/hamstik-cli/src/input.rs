// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Interactive input seam and long-text resolution.
//!
//! Commands gate *whether* prompting is allowed (SPEC §41); this module only
//! performs the reads. File arguments accept `-` to mean stdin (SPEC §46).
//!
//! Every read is size-capped: the CLI accepts input from pipes, redirected
//! files, and repository-local context files, so an oversized or misdirected
//! stream (`--body-file - < /dev/zero`) must fail with a clear error instead of
//! consuming unbounded memory.

use std::io::{self, BufRead, Read, Write};

use secrecy::SecretString;

/// The largest token accepted from stdin or a prompt (4 KiB).
pub const MAX_TOKEN_BYTES: usize = 4 * 1024;

/// Validates a token string and wraps it in a zeroizing [`SecretString`].
///
/// Trims surrounding whitespace (and NULs injected by sloppy shell quoting),
/// rejects empty values, oversized values, and embedded control characters
/// (which can never be valid PAT material but can corrupt header framing).
///
/// # Errors
/// Returns a descriptive message when the value cannot be a valid token.
pub fn token_to_secret(raw: &str) -> Result<SecretString, String> {
    let trimmed = raw.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    if trimmed.is_empty() {
        return Err("token is empty".to_string());
    }
    if trimmed.len() > MAX_TOKEN_BYTES {
        return Err(format!("token exceeds the {MAX_TOKEN_BYTES} byte limit"));
    }
    if trimmed.chars().any(char::is_control) {
        return Err("token must not contain control characters".to_string());
    }
    Ok(SecretString::new(trimmed.to_owned().into()))
}

/// The largest inline text body accepted (1 MiB) — item descriptions, comment
/// bodies, and similar user text.
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;

/// Reads up to `max` bytes from `reader`, failing once the stream is larger.
///
/// Reading stops at the first oversize byte; the excess is never buffered.
pub fn read_capped(reader: impl Read, max: usize) -> io::Result<String> {
    let buffer = read_bytes_capped(reader, max)?;
    String::from_utf8(buffer)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.utf8_error()))
}

/// Reads up to `max` bytes from `reader` as raw bytes, failing once the stream
/// is larger.
///
/// Binary counterpart of [`read_capped`] for uploads: reading stops at the
/// first oversize byte, so an oversized or misdirected stream is rejected
/// before it can be buffered.
pub fn read_bytes_capped(mut reader: impl Read, max: usize) -> io::Result<Vec<u8>> {
    let mut buffer = Vec::with_capacity(8 * 1024);
    let mut chunk = [0u8; 8 * 1024];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        if buffer.len() + read > max {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("input exceeds the {max} byte limit"),
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok(buffer)
}

/// Reads a first-line token (e.g. a PAT piped via `--with-token`).
///
/// Only the first line is consumed, so accidentally pointing the command at a
/// larger stream neither hangs on it nor buffers it.
pub fn read_token(reader: impl BufRead) -> io::Result<String> {
    let mut line = String::new();
    // One oversized line fails here; the cap applies to the token, and trailing
    // content after the first newline is never read.
    for byte in reader.bytes() {
        let byte = byte?;
        if byte == b'\n' {
            break;
        }
        if byte == b'\r' {
            continue;
        }
        if line.len() >= MAX_TOKEN_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("token exceeds the {MAX_TOKEN_BYTES} byte limit"),
            ));
        }
        line.push(byte as char);
    }
    Ok(line)
}

/// Reads a line or a secret from the user.
pub trait Prompt {
    /// Reads one visible line after printing `prompt`.
    fn read_line(&mut self, prompt: &str) -> io::Result<String>;
    /// Reads one hidden (no-echo) secret after printing `prompt`.
    fn read_secret(&mut self, prompt: &str) -> io::Result<String>;
}

/// Real terminal prompts (visible line + hidden secret).
pub struct TerminalInput;

impl Prompt for TerminalInput {
    fn read_line(&mut self, prompt: &str) -> io::Result<String> {
        print!("{prompt}");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    fn read_secret(&mut self, prompt: &str) -> io::Result<String> {
        let secret = rpassword::prompt_password(prompt)?;
        if secret.len() > MAX_TOKEN_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("token exceeds the {MAX_TOKEN_BYTES} byte limit"),
            ));
        }
        Ok(secret)
    }
}

/// Resolves long text from an inline value, a file, or stdin.
///
/// `file` of `-` reads all of stdin. Precedence is inline value, then file.
/// File and stdin reads are capped at [`MAX_TEXT_BYTES`].
pub fn resolve_text(
    inline: Option<String>,
    file: Option<&str>,
    stdin: &mut dyn Read,
) -> io::Result<Option<String>> {
    if let Some(text) = inline {
        if text.len() > MAX_TEXT_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("text exceeds the {MAX_TEXT_BYTES} byte limit"),
            ));
        }
        return Ok(Some(text));
    }
    match file {
        Some("-") => read_capped(stdin, MAX_TEXT_BYTES).map(Some),
        Some(path) => read_capped(std::fs::File::open(path)?, MAX_TEXT_BYTES).map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn inline_wins() {
        let mut stdin = Cursor::new(b"ignored".to_vec());
        let value = resolve_text(Some("hi".into()), Some("/nope"), &mut stdin).unwrap();
        assert_eq!(value.as_deref(), Some("hi"));
    }

    #[test]
    fn dash_reads_stdin() {
        let mut stdin = Cursor::new(b"from stdin".to_vec());
        let value = resolve_text(None, Some("-"), &mut stdin).unwrap();
        assert_eq!(value.as_deref(), Some("from stdin"));
    }

    #[test]
    fn none_returns_none() {
        let mut stdin = Cursor::new(Vec::new());
        assert!(resolve_text(None, None, &mut stdin).unwrap().is_none());
    }

    #[test]
    fn oversized_stdin_is_rejected() {
        let big = vec![b'x'; MAX_TEXT_BYTES + 1];
        let mut stdin = Cursor::new(big);
        let err = resolve_text(None, Some("-"), &mut stdin).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("byte limit"));
    }

    #[test]
    fn oversized_inline_is_rejected() {
        let big = "x".repeat(MAX_TEXT_BYTES + 1);
        let mut stdin = Cursor::new(Vec::new());
        assert!(resolve_text(Some(big), None, &mut stdin).is_err());
    }

    #[test]
    fn oversized_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        std::fs::write(&path, vec![b'x'; MAX_TEXT_BYTES + 1]).unwrap();
        let mut stdin = Cursor::new(Vec::new());
        let err = resolve_text(None, Some(path.to_str().unwrap()), &mut stdin).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn missing_file_reports_io_error() {
        let mut stdin = Cursor::new(Vec::new());
        assert!(resolve_text(None, Some("/nope/missing"), &mut stdin).is_err());
    }

    #[test]
    fn binary_reads_accept_non_utf8_and_enforce_the_cap() {
        let bytes = read_bytes_capped(Cursor::new(vec![0xff, 0x00, 0xfe]), 8).unwrap();
        assert_eq!(bytes, vec![0xff, 0x00, 0xfe]);
        let err = read_bytes_capped(Cursor::new(vec![b'x'; 9]), 8).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("byte limit"));
    }

    #[test]
    fn token_reads_only_the_first_line() {
        let input = "token-value\nextra-never-read\n";
        let token = read_token(Cursor::new(input)).unwrap();
        assert_eq!(token, "token-value");
    }

    #[test]
    fn token_handles_crlf_and_no_final_newline() {
        assert_eq!(read_token(Cursor::new("tok\r\nrest\n")).unwrap(), "tok");
        assert_eq!(read_token(Cursor::new("tok")).unwrap(), "tok");
        assert_eq!(read_token(Cursor::new("")).unwrap(), "");
    }

    #[test]
    fn oversized_token_is_rejected() {
        let big = "x".repeat(MAX_TOKEN_BYTES + 1);
        let err = read_token(Cursor::new(big)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn token_to_secret_trims_and_rejects_bad_values() {
        use secrecy::ExposeSecret;
        assert_eq!(
            token_to_secret("  tok\n")
                .unwrap()
                .expose_secret()
                .to_string(),
            "tok"
        );
        assert_eq!(
            token_to_secret("tok\0")
                .unwrap()
                .expose_secret()
                .to_string(),
            "tok"
        );
        assert!(token_to_secret("   ").is_err());
        assert!(token_to_secret("").is_err());
        assert!(token_to_secret(&"x".repeat(MAX_TOKEN_BYTES + 1)).is_err());
        assert!(token_to_secret("to\u{7}k").is_err());
    }
}
