// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Idempotency key generation and validation.
//!
//! A single logical mutation gets exactly one key, generated once and reused
//! across every internal retry of that operation.

use thiserror::Error;
use uuid::Uuid;

/// Errors for a client-supplied idempotency key.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum IdempotencyKeyError {
    /// The key is shorter than 8 or longer than 128 characters.
    #[error("idempotency key must be between 8 and 128 characters")]
    InvalidLength,
    /// The key contains characters outside the allowed set.
    #[error("idempotency key may only contain letters, digits, and . _ : -")]
    InvalidCharacters,
}

/// Generates a fresh cryptographically-random idempotency key (UUIDv4).
///
/// The result always satisfies [`validate_key`].
#[must_use]
pub fn generate_key() -> String {
    Uuid::new_v4().to_string()
}

/// Validates an idempotency key against the Public API contract:
/// `^[A-Za-z0-9._:-]{8,128}$`.
pub fn validate_key(key: &str) -> Result<(), IdempotencyKeyError> {
    if key.len() < 8 || key.len() > 128 {
        return Err(IdempotencyKeyError::InvalidLength);
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
    {
        return Err(IdempotencyKeyError::InvalidCharacters);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_are_valid_and_unique() {
        let a = generate_key();
        let b = generate_key();
        assert_ne!(a, b);
        validate_key(&a).unwrap();
        validate_key(&b).unwrap();
    }

    #[test]
    fn accepts_boundary_lengths() {
        validate_key("abcdefgh").unwrap();
        validate_key(&"a".repeat(128)).unwrap();
    }

    #[test]
    fn rejects_too_short() {
        assert_eq!(validate_key("abc"), Err(IdempotencyKeyError::InvalidLength));
    }

    #[test]
    fn rejects_too_long() {
        assert_eq!(
            validate_key(&"a".repeat(129)),
            Err(IdempotencyKeyError::InvalidLength)
        );
    }

    #[test]
    fn rejects_invalid_characters() {
        assert_eq!(
            validate_key("has space!"),
            Err(IdempotencyKeyError::InvalidCharacters)
        );
    }

    #[test]
    fn accepts_allowed_punctuation() {
        validate_key("Aa0._:-9").unwrap();
    }
}
