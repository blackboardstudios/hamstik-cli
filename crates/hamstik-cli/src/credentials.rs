// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Credential storage behind a seam.
//!
//! The production store delegates to the OS credential store (SPEC §25) via the
//! `keyring` crate. Tests inject [`MemoryCredentialStore`]. A missing entry is
//! reported as `Ok(None)` and is *not* an error — only genuine store failures
//! map to exit code 10.

use std::collections::HashMap;
use std::sync::Mutex;

use keyring::mock::MockCredential;
use keyring::{Entry, Error as KeyringError};
use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

/// Stable service identifier for stored Hamstik credentials.
pub const SERVICE: &str = "Hamstik CLI";

/// Guidance shown when no persistent credential store is reachable (SPEC §26).
pub const NO_STORE_HINT: &str = "no secure credential store is available; for headless \
automation set HAMSTIK_TOKEN for the process or configure an OS credential service";

/// Account used only to probe which keyring backend was compiled in.
const BACKEND_PROBE: &str = "backend-probe";

/// Credential-store failures (distinct from Hamstik authentication errors).
#[derive(Debug, Error)]
pub enum CredentialError {
    /// The store could not be reached or the operation failed.
    #[error("{0}")]
    Unavailable(String),
}

/// Reads and writes PATs in a secure store.
pub trait CredentialStore: Send + Sync {
    /// Reads the secret for `account`; `Ok(None)` when absent.
    fn get(&self, account: &str) -> Result<Option<SecretString>, CredentialError>;
    /// Stores (or replaces) the secret for `account`.
    fn set(&self, account: &str, secret: &SecretString) -> Result<(), CredentialError>;
    /// Removes the secret for `account`; absent is fine.
    fn delete(&self, account: &str) -> Result<(), CredentialError>;
}

/// Builds the collision-resistant account key: host + Hamstik user id.
#[must_use]
pub fn account_key(host: &str, user_id: &str) -> String {
    format!("host={host};user={user_id}")
}

/// OS credential store backed by the `keyring` crate.
pub struct KeyringCredentialStore;

impl KeyringCredentialStore {
    /// Runs one keyring operation on a throwaway thread.
    ///
    /// The Linux Secret Service backend talks D-Bus through a blocking API that
    /// enters its own async runtime, and doing that on a thread which already
    /// drives a runtime panics. Every CLI command runs inside `tokio`, so the
    /// hop is mandatory here; keeping it inside this module preserves the
    /// synchronous [`CredentialStore`] seam for the rest of the code.
    ///
    /// A panic in the operation is not swallowed: its payload (the D-Bus or
    /// backend message, usually a `&str`) is folded into the returned error so
    /// the underlying failure stays diagnosable.
    fn on_store_thread<T, F>(operation: F) -> Result<T, CredentialError>
    where
        T: Send,
        F: FnOnce() -> Result<T, CredentialError> + Send,
    {
        std::thread::scope(|scope| match scope.spawn(operation).join() {
            Ok(result) => result,
            Err(panic_payload) => Err(CredentialError::Unavailable(
                match panic_payload.downcast_ref::<&str>() {
                    Some(message) => (*message).to_string(),
                    None => match panic_payload.downcast_ref::<String>() {
                        Some(message) => message.clone(),
                        None => "credential store worker failed".to_string(),
                    },
                },
            )),
        })
    }
}

/// Builds an entry, rejecting backends that cannot persist anything.
fn entry(account: &str) -> Result<Entry, CredentialError> {
    let entry = Entry::new(SERVICE, account)
        .map_err(|err| CredentialError::Unavailable(err.to_string()))?;
    if is_mock(&entry) {
        // A mock backend accepts writes that die with the process, so a
        // successful-looking login would silently lose the PAT.
        return Err(CredentialError::Unavailable(NO_STORE_HINT.to_string()));
    }
    Ok(entry)
}

/// Reports whether the compiled-in keyring backend actually persists secrets.
///
/// `keyring` has no default features: with no platform backend enabled it
/// silently resolves to its in-process mock store. Callers must never treat
/// that store as usable (SPEC §25, §26).
#[must_use]
pub fn store_is_persistent() -> bool {
    KeyringCredentialStore::on_store_thread(|| match Entry::new(SERVICE, BACKEND_PROBE) {
        // If even entry construction fails, nothing usable is reachable.
        Ok(entry) => Ok(!is_mock(&entry)),
        Err(_) => Ok(false),
    })
    .unwrap_or(false)
}

fn is_mock(entry: &Entry) -> bool {
    entry
        .get_credential()
        .downcast_ref::<MockCredential>()
        .is_some()
}

impl CredentialStore for KeyringCredentialStore {
    fn get(&self, account: &str) -> Result<Option<SecretString>, CredentialError> {
        Self::on_store_thread(|| match entry(account)?.get_password() {
            Ok(password) => Ok(Some(SecretString::from(password))),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(err) => Err(CredentialError::Unavailable(err.to_string())),
        })
    }

    fn set(&self, account: &str, secret: &SecretString) -> Result<(), CredentialError> {
        Self::on_store_thread(|| {
            entry(account)?
                .set_password(secret.expose_secret())
                .map_err(|err| CredentialError::Unavailable(err.to_string()))
        })
    }

    fn delete(&self, account: &str) -> Result<(), CredentialError> {
        Self::on_store_thread(|| match entry(account)?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(err) => Err(CredentialError::Unavailable(err.to_string())),
        })
    }
}

/// In-memory store for tests (never used in production).
///
/// The mutex is expected (not unwrapped) with lock-poisoning tolerated: a
/// poisoned lock here means a test thread panicked, and returning a store
/// error beats cascading a panic.
#[derive(Default)]
pub struct MemoryCredentialStore {
    entries: Mutex<HashMap<String, String>>,
}

impl MemoryCredentialStore {
    /// Creates an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seeds a value for tests that pre-populate credentials.
    pub fn seed(&self, account: &str, secret: &str) {
        self.lock().insert(account.to_string(), secret.to_string());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn get(&self, account: &str) -> Result<Option<SecretString>, CredentialError> {
        let value = self.lock().get(account).cloned();
        Ok(value.map(SecretString::from))
    }

    fn set(&self, account: &str, secret: &SecretString) -> Result<(), CredentialError> {
        self.lock()
            .insert(account.to_string(), secret.expose_secret().to_string());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), CredentialError> {
        self.lock().remove(account);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_roundtrip() {
        let store = MemoryCredentialStore::new();
        assert!(store.get("acct").unwrap().is_none());
        store.set("acct", &SecretString::from("tok")).unwrap();
        assert_eq!(store.get("acct").unwrap().unwrap().expose_secret(), "tok");
        store.delete("acct").unwrap();
        assert!(store.get("acct").unwrap().is_none());
    }

    #[test]
    fn delete_missing_is_ok() {
        let store = MemoryCredentialStore::new();
        store.delete("nope").unwrap();
    }

    #[test]
    fn account_key_includes_host_and_user() {
        let key = account_key("https://hamstik.com", "abc");
        assert!(key.contains("https://hamstik.com"));
        assert!(key.contains("abc"));
    }

    /// Regression guard for the `keyring` Cargo features.
    ///
    /// Without a platform backend feature, `keyring` resolves to its
    /// in-process mock store: login/logout appear to work while nothing is
    /// ever persisted. The Cargo manifest must therefore keep enabling the
    /// native store on every supported platform.
    #[test]
    fn production_backend_is_not_the_in_memory_mock() {
        assert!(
            store_is_persistent(),
            "keyring was compiled without a platform backend; \
             enable apple-native / windows-native / async-secret-service \
             for the target platform"
        );
    }

    #[test]
    fn mock_backends_are_recognised() {
        // The guard that turns keyring's silent mock fallback into a real error
        // depends on being able to recognise a mock credential.
        let mock = Entry::new_with_credential(Box::new(MockCredential::default()));
        assert!(is_mock(&mock));
    }

    /// Round-trips a throwaway secret through the real OS store from inside an
    /// async runtime context (the way the CLI actually calls it).
    ///
    /// Ignored by default because it needs an unlocked OS credential store;
    /// run `cargo test -p hamstik-cli --lib -- --ignored` when one is present.
    #[test]
    #[ignore = "requires an unlocked OS credential store"]
    fn keyring_round_trips_inside_async_context() {
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let account = account_key("https://keyring-probe.invalid", "probe-user");
        let secret = SecretString::from("probe-token-value".to_string());

        runtime.block_on(async {
            KeyringCredentialStore.set(&account, &secret).unwrap();
            let read = KeyringCredentialStore.get(&account).unwrap();
            assert_eq!(
                read.expect("stored secret").expose_secret(),
                "probe-token-value"
            );
            KeyringCredentialStore.delete(&account).unwrap();
            assert!(KeyringCredentialStore.get(&account).unwrap().is_none());
        });
    }
}
