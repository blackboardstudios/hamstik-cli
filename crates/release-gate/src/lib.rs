// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Fail-closed release gate for the Hamstik CLI (CLI-27).
//!
//! Enforces the version/tag contract of [`design/VERSIONING.md`] in the
//! release workflow *before* anything is built or published:
//!
//! 1. the release tag is exactly `v<version>` (SemVer core with an optional
//!    prerelease suffix, no build metadata);
//! 2. the tag version equals the single version source, the
//!    `[workspace.package]` `version` in the root `Cargo.toml` (§1);
//! 3. `CHANGELOG.md` contains the matching `## [<version>]` release section
//!    (Keep a Changelog; §8/§9 — the `Unreleased` section must have been
//!    renamed before tagging).
//!
//! A mismatch is a hard error: a tag `v0.2.0` must never silently release a
//! binary reporting `0.1.0`. The logic is pure and unit-tested; the binary is
//! a thin file-reading wrapper used by `.github/workflows/release.yml`.
//!
//! [`design/VERSIONING.md`]: ../../design/VERSIONING.md

/// The normalized version text from a release tag.
///
/// `v1.2.3` yields `1.2.3`; a prerelease tag `v1.2.3-rc.1` yields
/// `1.2.3-rc.1`. Build metadata is rejected: it is ignored by Cargo/semver
/// comparison and would make "which release is this?" ambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagVersion {
    /// The tag text without the leading `v` (e.g. `0.1.0-rc.1`).
    pub version: String,
}

impl TagVersion {
    /// Parses a release tag, failing closed on anything that is not
    /// `v<MAJOR>.<MINOR>.<PATCH>[-<prerelease>]`.
    pub fn parse(tag: &str) -> Result<Self, String> {
        let Some(body) = tag.strip_prefix('v') else {
            return Err(format!(
                "tag {tag:?} does not start with 'v'; release tags follow \
                 vMAJOR.MINOR.PATCH (design/VERSIONING.md §9)"
            ));
        };
        if body.is_empty() {
            return Err("empty version after 'v'".to_string());
        }
        if body.contains('+') {
            return Err(format!(
                "tag {tag:?} carries build metadata; release tags must not \
                 (the version is the single release identity)"
            ));
        }
        let (core, prerelease) = match body.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (body, None),
        };
        let components: Vec<&str> = core.split('.').collect();
        if components.len() != 3 {
            return Err(format!(
                "tag {tag:?} is not vMAJOR.MINOR.PATCH (got {components_len} numeric \
                 component(s) before the prerelease suffix)",
                components_len = components.len()
            ));
        }
        for (index, component) in components.iter().enumerate() {
            if component.is_empty() || !component.bytes().all(|b| b.is_ascii_digit()) {
                return Err(format!(
                    "tag {tag:?}: version component {} ({component:?}) is not numeric",
                    index + 1
                ));
            }
        }
        if let Some(pre) = prerelease
            && (pre.is_empty()
                || !pre
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'))
        {
            return Err(format!(
                "tag {tag:?}: prerelease suffix contains invalid characters"
            ));
        }
        Ok(Self {
            version: body.to_string(),
        })
    }

    /// The tag text as it would be typed on the command line (`v`-prefixed).
    #[must_use]
    pub fn tag_text(&self) -> String {
        format!("v{}", self.version)
    }
}

/// Extracts the single version source: the `[workspace.package]` `version`
/// field of the root `Cargo.toml` (design/VERSIONING.md §1).
///
/// Only the workspace package table is honored — per-crate tables inherit it
/// (`version.workspace = true`), so anything else means the version source
/// has been split and the gate must refuse the release.
pub fn workspace_version(cargo_toml: &str) -> Result<String, String> {
    const TABLE: &str = "[workspace.package]";
    let table_start = cargo_toml
        .find(TABLE)
        .ok_or_else(|| "root Cargo.toml has no [workspace.package] table".to_string())?;
    let table_body = &cargo_toml[table_start + TABLE.len()..];
    let table_end = table_body
        .find("\n[")
        .map_or(table_body.len(), |index| index + 1);
    let table = &table_body[..table_end];
    for line in table.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("version") else {
            continue;
        };
        let rest = rest.trim_start();
        if !rest.starts_with('=') {
            continue;
        }
        let value = rest[1..].trim();
        let value = value
            .strip_prefix('"')
            .and_then(|stripped| stripped.strip_suffix('"'))
            .ok_or_else(|| {
                format!(
                    "[workspace.package] version = {value:?} is not a quoted string; \
                     the gate only reads the single version source form \
                     version = \"X.Y.Z\""
                )
            })?;
        if value.is_empty() {
            return Err("[workspace.package] version is empty".to_string());
        }
        return Ok(value.to_string());
    }
    Err(
        "[workspace.package] has no `version` key; design/VERSIONING.md §1 \
         requires the workspace package version as the single version source"
            .to_string(),
    )
}

/// True when the changelog contains the release section `## [<version>]`.
///
/// Matches the Keep a Changelog heading prefix only (`## [0.1.0]` also
/// matches `## [0.1.0] - 2026-09-14`), and never matches other versions or
/// the `## [Unreleased]` section.
#[must_use]
pub fn changelog_has_release(changelog: &str, version: &str) -> bool {
    let heading = format!("## [{version}]");
    changelog.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with(&heading)
    })
}

/// Runs the full gate: tag parse, workspace version equality, changelog
/// release section. Returns precise, actionable failure messages so a failed
/// release explains exactly which contract broke.
pub fn run_gate(tag: &str, cargo_toml: &str, changelog: &str) -> Result<TagVersion, String> {
    let parsed = TagVersion::parse(tag)?;
    let workspace = workspace_version(cargo_toml)?;
    if workspace != parsed.version {
        return Err(format!(
            "tag {} does not match the workspace version {workspace}; bump \
             [workspace.package] version in the root Cargo.toml (single version \
             source, design/VERSIONING.md §1) before tagging",
            parsed.tag_text()
        ));
    }
    if !changelog_has_release(changelog, &parsed.version) {
        let v = parsed.version;
        return Err(format!(
            "CHANGELOG.md has no `## [{v}]` release section; rename \
             `## [Unreleased]` to `## [{v}] - <date>` per design/VERSIONING.md \
             §8–§9 before tagging v{v}"
        ));
    }
    Ok(parsed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parses_strict_v_tags() {
        assert_eq!(TagVersion::parse("v0.1.0").unwrap().version, "0.1.0");
        assert_eq!(TagVersion::parse("v1.2.3").unwrap().version, "1.2.3");
        assert_eq!(
            TagVersion::parse("v0.2.0-rc.1").unwrap().version,
            "0.2.0-rc.1"
        );
        assert_eq!(TagVersion::parse("v0.1.0").unwrap().tag_text(), "v0.1.0");
    }

    #[test]
    fn rejects_non_v_and_malformed_tags() {
        for tag in [
            "0.1.0",
            "",
            "v",
            "v0.1",
            "v0.1.0.1",
            "vlatest",
            "v0.x.0",
            "v01.a.0",
            "release/v0.1.0",
            "hamstik-cli/0.1.0",
            "v0.1.0+build.7",
            "v0.1.0-",
            "v0.1.0-rc.1!",
            "v 0.1.0",
        ] {
            assert!(TagVersion::parse(tag).is_err(), "tag {tag:?} must fail");
        }
    }

    #[test]
    fn parses_prerelease_suffixes_with_dots_and_hyphens() {
        assert_eq!(
            TagVersion::parse("v1.0.0-alpha-beta.2").unwrap().version,
            "1.0.0-alpha-beta.2"
        );
    }

    const CARGO_TOML: &str = "[workspace]\nmembers = [\"a\"]\nresolver = \"2\"\n\n\
[workspace.package]\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
[workspace.dependencies]\nserde = \"1\"\n";

    #[test]
    fn reads_workspace_package_version() {
        assert_eq!(
            workspace_version(CARGO_TOML).unwrap(),
            "0.1.0",
            "version inside [workspace.package] must be found"
        );
    }

    #[test]
    fn ignores_version_keys_outside_the_workspace_package_table() {
        // A [package] table before [workspace.package] must not win, and a
        // version line after the table must be out of scope.
        let tricky = "[package]\nversion = \"9.9.9\"\n\n[workspace.package]\n\
version = \"0.1.0\"\n\n[dependencies]\nversion-note = \"x\"\n";
        assert_eq!(workspace_version(tricky).unwrap(), "0.1.0");
    }

    #[test]
    fn rejects_missing_or_unquoted_workspace_version() {
        assert!(workspace_version("[workspace.package]\nedition = \"2024\"\n").is_err());
        assert!(workspace_version("[workspace.package]\nversion = 0.1.0\n").is_err());
        assert!(workspace_version("nothing here").is_err());
    }

    #[test]
    fn changelog_heading_matches_release_section() {
        let changelog = "# Changelog\n\n## [Unreleased]\n\n### Added\n- x\n\n\
## [0.1.0] - 2026-09-14\n\n### Added\n- y\n\n[0.1.0]: https://example.com\n";
        assert!(changelog_has_release(changelog, "0.1.0"));
        assert!(!changelog_has_release(changelog, "0.2.0"));
        // The gate can never pass "Unreleased" as a version — TagVersion
        // rejects non-numeric components — so an Unreleased-only changelog
        // fails the gate (see unreleased_only_changelog_fails_the_gate).
        // A longer version must not satisfy a shorter heading.
        let with_rc = "# C\n\n## [0.2.0-rc.1] - 2026-09-14\n";
        assert!(changelog_has_release(with_rc, "0.2.0-rc.1"));
        assert!(!changelog_has_release(with_rc, "0.2.0"));
    }

    #[test]
    fn unreleased_only_changelog_fails_the_gate() {
        let changelog = "# Changelog\n\n## [Unreleased]\n\n### Added\n- x\n";
        let error = run_gate("v0.1.0", CARGO_TOML, changelog).unwrap_err();
        assert!(error.contains("## [0.1.0]"), "error: {error}");
        assert!(error.contains("Unreleased"));
    }

    #[test]
    fn version_mismatch_fails_the_gate_with_guidance() {
        let error = run_gate("v0.2.0", CARGO_TOML, "# C\n\n## [0.2.0] - 2026-01-01\n").unwrap_err();
        assert!(
            error.contains("0.2.0") && error.contains("0.1.0"),
            "{error}"
        );
        assert!(error.contains("Cargo.toml"));
    }

    #[test]
    fn consistent_inputs_pass_the_gate() {
        let changelog = "# C\n\n## [Unreleased]\n\n## [0.1.0] - 2026-09-14\n";
        let parsed = run_gate("v0.1.0", CARGO_TOML, changelog).unwrap();
        assert_eq!(parsed.version, "0.1.0");
    }

    #[test]
    fn malformed_tag_fails_before_other_checks() {
        let changelog = "# C\n\n## [0.1.0] - 2026-09-14\n";
        assert!(run_gate("wrong", CARGO_TOML, changelog).is_err());
    }
}
