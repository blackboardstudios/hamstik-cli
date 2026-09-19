// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Global configuration file (`config.toml`).
//!
//! Stores non-secret profile metadata only (SPEC §23). Writes are atomic,
//! durable, and size-capped; unknown fields are rejected so drift from a future
//! CLI is loud.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CliError;
use crate::fsutil;

/// Current configuration schema version.
pub const CONFIG_VERSION: u32 = 2;

/// Minimum supported configuration schema version.
pub const MIN_CONFIG_VERSION: u32 = 1;

/// Maximum accepted config file size (1 MiB).
pub const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Non-secret metadata about one authenticated account on one host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// The host origin this credential belongs to.
    pub host: String,
    /// The Hamstik user id (part of the keyring account key).
    pub user_id: String,
    /// The account email (display only; never used for auth).
    pub email: String,
    /// Default organization slug for this profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_organization: Option<String>,
    /// Default project key for this profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_project: Option<String>,
}

/// Non-secret global settings persisted alongside profiles.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigSettings {
    /// Preferred editor command for `--*-editor` authoring, used when neither
    /// `$VISUAL` nor `$EDITOR` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor: Option<String>,
    /// Preferred pager command for long output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pager: Option<String>,
    /// Preferred output mode (`human`, `json`, `jsonl`, `tsv`, or `quiet`).
    /// Explicit flags always win; the value is validated on write.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Git branch name template used by context-aware commands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch_template: Option<String>,
    /// When `false` the CLI skips the append-only mutation audit log
    /// (`audit.log` in the platform data-local directory). Defaults to
    /// `true`. Audit records contain the command, target identifier, and
    /// server request id — never credentials or request bodies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_log: Option<bool>,
}

/// On-disk configuration document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    /// On-disk schema version.
    pub version: u32,
    /// The profile selected by default, when one is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,
    /// All configured profiles, keyed by profile name.
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    /// Global non-secret CLI defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<ConfigSettings>,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            active_profile: None,
            profiles: BTreeMap::new(),
            settings: None,
        }
    }
}

/// Reads and writes the config file at a fixed location.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// Wraps a fixed path (from config-path resolution).
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file path this store reads and writes.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the config, returning an empty document when the file is absent.
    ///
    /// An absent file is a normal state, never an error. A file that exists but
    /// cannot be understood IS an error: silently starting over would discard
    /// profiles and context defaults. Every failure names the file and says how
    /// to recover, because "invalid configuration" alone is not actionable.
    pub fn load(&self) -> Result<ConfigFile, CliError> {
        if !self.path.exists() {
            return Ok(ConfigFile::default());
        }
        let metadata = fs::metadata(self.path())
            .map_err(|err| self.fail(&format!("cannot read file ({err})")))?;
        if metadata.len() > MAX_CONFIG_BYTES {
            return Err(self.fail("file is too large (exceeds the 1 MiB limit)"));
        }
        let contents = fs::read_to_string(self.path())
            .map_err(|err| self.fail(&format!("cannot read file ({err})")))?;
        let mut config: ConfigFile = toml::from_str(&contents).map_err(|err| {
            self.fail(&format!(
                "invalid configuration (repair the file, or upgrade this CLI if it was \
                 written by a newer version): {err}"
            ))
        })?;
        self.check_version(&config)?;
        self.migrate(&mut config)?;
        Ok(config)
    }

    /// Migrates older schema versions in place.
    fn migrate(&self, config: &mut ConfigFile) -> Result<(), CliError> {
        if config.version < CONFIG_VERSION {
            config.settings.get_or_insert(ConfigSettings::default());
            config.version = CONFIG_VERSION;
        }
        Ok(())
    }

    /// Rejects documents written for a different schema version.
    fn check_version(&self, config: &ConfigFile) -> Result<(), CliError> {
        match config.version.cmp(&CONFIG_VERSION) {
            std::cmp::Ordering::Equal => Ok(()),
            std::cmp::Ordering::Greater => Err(self.fail(&format!(
                "schema version {} is newer than this CLI understands (version \
                 {CONFIG_VERSION}); upgrade the Hamstik CLI",
                config.version
            ))),
            std::cmp::Ordering::Less => {
                if config.version < MIN_CONFIG_VERSION {
                    Err(self.fail(&format!(
                        "schema version {} is no longer supported (this CLI writes version \
                         {CONFIG_VERSION})",
                        config.version
                    )))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Builds a config failure that names the file and offers a way out.
    fn fail(&self, reason: &str) -> CliError {
        CliError::config(format!(
            "{}: {reason}\nhint: repair or remove this file to recover; it holds only non-secret \
             profile metadata (tokens live in the OS credential store)",
            self.path.display()
        ))
    }

    /// Atomically and durably writes the config, creating parent directories
    /// as needed.
    ///
    /// The write goes through a unique temporary file that is synced to stable
    /// storage before an atomic rename publishes it, so a crash can never leave
    /// a torn or empty config behind. Permissions are restricted to the owner.
    pub fn save(&self, config: &ConfigFile) -> Result<(), CliError> {
        let serialized = toml::to_string_pretty(config)
            .map_err(|err| self.fail(&format!("cannot serialize configuration ({err})")))?;
        let bytes = serialized.as_bytes();
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(self.fail("configuration would exceed the 1 MiB limit"));
        }

        fsutil::write_atomic(self.path(), bytes)
            .and_then(|()| fsutil::restrict_permissions(self.path()))
            .map_err(|err| self.fail(&format!("cannot replace file ({err})")))?;
        Ok(())
    }
}

/// One saved SqueakQL query: a name and a plain expression.
///
/// Saved queries are exactly the text sent to the SqueakQL endpoint — never
/// credentials, request headers, or server-side constructs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedQuery {
    /// The plain SqueakQL expression.
    pub query: String,
    /// When the query was saved (RFC 3339, informational).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<String>,
}

/// The `queries.toml` document: named SqueakQL expressions for local reuse.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SavedQueriesFile {
    /// On-disk schema version.
    pub version: u32,
    /// Saved queries keyed by name.
    #[serde(default)]
    pub queries: BTreeMap<String, SavedQuery>,
}

impl Default for SavedQueriesFile {
    fn default() -> Self {
        Self {
            version: SAVED_QUERIES_VERSION,
            queries: BTreeMap::new(),
        }
    }
}

/// Maximum accepted `queries.toml` size (1 MiB).
pub const MAX_SAVED_QUERIES_BYTES: u64 = 1024 * 1024;

/// Current saved-queries schema version.
pub const SAVED_QUERIES_VERSION: u32 = 1;

/// Maximum number of saved queries (keeps the file reviewable and bounded).
pub const MAX_SAVED_QUERIES: usize = 100;

/// Maximum one saved query expression length (matches the API request cap
/// with headroom for formatting).
pub const MAX_SAVED_QUERY_LENGTH: usize = 4096;

/// Reads and writes the saved-queries file (`queries.toml` next to the
/// config).
///
/// Storage model (documented contract): one plain TOML file next to
/// `config.toml` (so `HAMSTIK_CONFIG` relocates it identically), keyed by
/// name, containing only expressions — never tokens or headers. Names are
/// 1–64 characters of letters, digits, `-`, `_`. Writes are atomic and
/// owner-restricted like the config.
#[derive(Debug, Clone)]
pub struct SavedQueryStore {
    path: PathBuf,
}

impl SavedQueryStore {
    /// Builds a store from the config path (the queries file sits next to it).
    #[must_use]
    pub fn adjacent_to(config_path: &Path) -> Self {
        Self {
            path: config_path
                .parent()
                .unwrap_or(Path::new("."))
                .join("queries.toml"),
        }
    }

    /// The file path this store reads and writes.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads saved queries; an absent file is an empty document.
    pub fn load(&self) -> Result<SavedQueriesFile, CliError> {
        if !self.path.exists() {
            return Ok(SavedQueriesFile::default());
        }
        let metadata = fs::metadata(self.path())
            .map_err(|err| self.fail(&format!("cannot read file ({err})")))?;
        if metadata.len() > MAX_SAVED_QUERIES_BYTES {
            return Err(self.fail("file is too large (exceeds the 1 MiB limit)"));
        }
        let contents = fs::read_to_string(self.path())
            .map_err(|err| self.fail(&format!("cannot read file ({err})")))?;
        let file: SavedQueriesFile = toml::from_str(&contents).map_err(|err| {
            self.fail(&format!(
                "invalid saved-queries file (repair it, or delete it to start over; it holds only \
                 plain query text): {err}"
            ))
        })?;
        if file.version != SAVED_QUERIES_VERSION {
            return Err(self.fail(&format!(
                "schema version {} is not supported (this CLI uses version {SAVED_QUERIES_VERSION})",
                file.version
            )));
        }
        Ok(file)
    }

    /// Validates a saved-query name (1–64 chars: letters, digits, `-`, `_`).
    pub fn validate_name(name: &str) -> Result<(), CliError> {
        if name.is_empty() || name.len() > 64 {
            return Err(CliError::usage(
                "saved query name must be between 1 and 64 characters",
            ));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(CliError::usage(
                "saved query names may contain only letters, digits, '-', and '_'",
            ));
        }
        Ok(())
    }

    /// Atomically writes the saved-queries file.
    pub fn save(&self, file: &SavedQueriesFile) -> Result<(), CliError> {
        let serialized = toml::to_string_pretty(file)
            .map_err(|err| self.fail(&format!("cannot serialize saved queries ({err})")))?;
        fsutil::write_atomic(self.path(), serialized.as_bytes())
            .and_then(|()| fsutil::restrict_permissions(self.path()))
            .map_err(|err| self.fail(&format!("cannot write file ({err})")))?;
        Ok(())
    }

    /// Builds a saved-queries failure naming the file.
    fn fail(&self, reason: &str) -> CliError {
        CliError::config(format!(
            "{}: {reason}\nhint: repair or delete this file to recover; it holds only plain \
             SqueakQL expressions",
            self.path.display()
        ))
    }
}

/// Removes a profile from the document and repairs `active_profile`.
///
/// Forgetting the active profile must not silently move the user to a
/// different host, so the slot is only refilled when exactly one profile
/// remains (then there is no ambiguity).
#[must_use]
pub fn forget_profile(config: &mut ConfigFile, name: &str) -> Option<Profile> {
    let removed = config.profiles.remove(name)?;
    if config.active_profile.as_deref() == Some(name) {
        config.active_profile = if config.profiles.len() == 1 {
            config.profiles.keys().next().cloned()
        } else {
            None
        };
    }
    Some(removed)
}

/// Validates a profile name against `^[A-Za-z0-9._-]{1,64}$`.
pub fn validate_profile_name(name: &str) -> Result<(), CliError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(CliError::config(format!(
            "invalid profile name {name:?}: use letters, digits, dot, underscore, or hyphen (1-64 chars)"
        )))
    }
}

/// Replaces characters that are not permitted in a profile name with a hyphen,
/// so auto-generated names (e.g. from hosts that include a port) stay valid.
fn sanitize_profile_segment(segment: &str) -> String {
    segment
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Produces a `<host-without-scheme>-<email-localpart>` profile name, reduced
/// to characters accepted by [`validate_profile_name`] (a host port's colon
/// becomes a hyphen).
#[must_use]
pub fn profile_auto_name(host: &str, email: &str) -> String {
    let authority = host.split_once("://").map(|(_, rest)| rest).unwrap_or(host);
    let authority = authority.trim_matches('/');
    let local = email.split('@').next().unwrap_or(email);
    format!(
        "{}-{}",
        sanitize_profile_segment(authority),
        sanitize_profile_segment(local)
    )
}

/// Returns a name not already used by an unrelated profile.
#[must_use]
pub fn unique_profile_name(config: &ConfigFile, base: &str, user_id: &str, host: &str) -> String {
    let mut candidate = base.to_string();
    let mut index = 2;
    while let Some(existing) = config.profiles.get(&candidate) {
        if existing.user_id == user_id && existing.host == host {
            break;
        }
        candidate = format!("{base}-{index}");
        index += 1;
    }
    candidate
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_config() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.toml"));
        let config = store.load().unwrap();
        assert_eq!(config, ConfigFile::default());
    }

    #[test]
    fn roundtrips_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("nested").join("config.toml"));
        let mut config = ConfigFile {
            active_profile: Some("hamstik.com-steven".to_string()),
            ..Default::default()
        };
        config.profiles.insert(
            "hamstik.com-steven".to_string(),
            Profile {
                host: "https://hamstik.com".to_string(),
                user_id: "u1".to_string(),
                email: "steven@example.com".to_string(),
                default_organization: Some("acme".to_string()),
                default_project: None,
            },
        );
        store.save(&config).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded, config);
    }

    #[test]
    fn rejects_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "version = 1\nmystery = true\n").unwrap();
        let store = ConfigStore::new(path);
        assert!(store.load().is_err());
    }

    #[test]
    fn auto_name_strips_scheme_and_domain() {
        assert_eq!(
            profile_auto_name("https://hamstik.com", "steven@example.com"),
            "hamstik.com-steven"
        );
    }

    #[test]
    fn auto_name_sanitizes_host_port() {
        let name = profile_auto_name("http://localhost:3000", "test@hamstik.dev");
        assert_eq!(name, "localhost-3000-test");
        validate_profile_name(&name).unwrap();
    }

    #[test]
    fn unique_name_appends_suffix_on_collision() {
        let mut config = ConfigFile::default();
        config.profiles.insert(
            "h-steven".to_string(),
            Profile {
                host: "https://a".to_string(),
                user_id: "other".to_string(),
                email: "steven@a".to_string(),
                default_organization: None,
                default_project: None,
            },
        );
        let name = unique_profile_name(&config, "h-steven", "mine", "https://b");
        assert_eq!(name, "h-steven-2");
    }

    #[test]
    fn unique_name_reuses_same_identity() {
        let mut config = ConfigFile::default();
        config.profiles.insert(
            "h-steven".to_string(),
            Profile {
                host: "https://b".to_string(),
                user_id: "mine".to_string(),
                email: "steven@b".to_string(),
                default_organization: None,
                default_project: None,
            },
        );
        let name = unique_profile_name(&config, "h-steven", "mine", "https://b");
        assert_eq!(name, "h-steven");
    }

    #[test]
    fn validates_profile_names() {
        validate_profile_name("hamstik.com-steven").unwrap();
        assert!(validate_profile_name("bad name").is_err());
        assert!(validate_profile_name("").is_err());
    }

    fn sample_profile(host: &str, user_id: &str) -> Profile {
        Profile {
            host: host.to_string(),
            user_id: user_id.to_string(),
            email: format!("{user_id}@example.com"),
            default_organization: None,
            default_project: None,
        }
    }

    #[test]
    fn corrupt_config_error_names_the_file_and_how_to_recover() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "version = 1\n[[broken\n").unwrap();

        let err = ConfigStore::new(&path).load().unwrap_err();
        // The user must be able to tell WHICH file broke and what to do next:
        // HAMSTIK_CONFIG, XDG and the legacy path all exist.
        assert_eq!(err.exit_code(), crate::exit::CONFIGURATION);
        assert!(
            err.message.contains(&path.display().to_string()),
            "error must name the file: {}",
            err.message
        );
        assert!(err.message.contains("invalid configuration"), "{err}");
        assert!(err.message.contains("hint:"), "{err}");
    }

    #[test]
    fn unreadable_config_file_reports_the_failure() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::create_dir(&path).unwrap();

        let err = ConfigStore::new(&path).load().unwrap_err();
        assert_eq!(err.exit_code(), crate::exit::CONFIGURATION);
        assert!(err.message.contains("cannot read file"), "{err}");
        assert!(err.message.contains("hint:"), "{err}");
    }

    #[test]
    fn newer_schema_version_asks_for_an_upgrade() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, format!("version = {}\n", CONFIG_VERSION + 1)).unwrap();

        let err = ConfigStore::new(&path).load().unwrap_err();
        assert!(err.message.contains("newer than this CLI"), "{err}");
        assert!(err.message.contains("upgrade"), "{err}");
    }

    #[test]
    fn unsupported_schema_version_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "version = 0\n").unwrap();

        let err = ConfigStore::new(&path).load().unwrap_err();
        assert!(err.message.contains("no longer supported"), "{err}");
    }

    /// A config with the given active profile and one profile per name.
    fn config_with(active: Option<&str>, names: &[&str]) -> ConfigFile {
        let mut config = ConfigFile {
            active_profile: active.map(str::to_string),
            ..Default::default()
        };
        for name in names {
            config
                .profiles
                .insert((*name).to_string(), sample_profile("https://x.test", name));
        }
        config
    }

    #[test]
    fn forget_profile_removes_the_entry_and_keeps_an_unrelated_active_one() {
        let mut config = config_with(Some("keep"), &["keep", "gone"]);

        let removed = super::forget_profile(&mut config, "gone").expect("profile removed");
        assert_eq!(removed.user_id, "gone");
        assert!(!config.profiles.contains_key("gone"));
        assert_eq!(config.active_profile.as_deref(), Some("keep"));
    }

    #[test]
    fn forgetting_the_active_profile_refills_only_when_unambiguous() {
        // Exactly one profile remains: it becomes active, nothing to guess.
        let mut config = config_with(Some("a"), &["a", "b"]);
        assert!(super::forget_profile(&mut config, "a").is_some());
        assert_eq!(config.active_profile.as_deref(), Some("b"));

        // Several remain: picking one would silently switch hosts, so none does.
        let mut config = config_with(Some("a"), &["a", "b", "c"]);
        assert!(super::forget_profile(&mut config, "a").is_some());
        assert_eq!(config.active_profile, None);
    }

    #[test]
    fn forgetting_an_unknown_profile_changes_nothing() {
        let mut config = config_with(Some("a"), &["a"]);

        assert!(super::forget_profile(&mut config, "nope").is_none());
        assert_eq!(config.active_profile.as_deref(), Some("a"));
        assert_eq!(config.profiles.len(), 1);
    }
}
