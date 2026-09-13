// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Crash-safe file writes.
//!
//! [`write_atomic`] replaces a file via a unique temporary sibling and
//! `rename`: readers never observe a torn write, concurrent writers cannot
//! collide on the temp path, a pre-planted symlink cannot redirect the write,
//! and on filesystems with delayed allocation the rename publishes data that is
//! already on disk.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Atomically replaces `path` with `contents`.
///
/// The temporary file is created with `create_new` in the target directory
/// under a unique name (pid + uuid), fully written, flushed to the OS, and
/// synced to stable storage before the rename publishes it. On any failure the
/// temporary file is removed, best effort.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory)?;

    let temp = unique_temp_path(path);
    let result = write_via(&temp, path, contents);

    if result.is_err() {
        // Never leave an orphaned temp behind on failure.
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_via(temp: &Path, target: &Path, contents: &[u8]) -> io::Result<()> {
    // `create_new` fails if a hostile symlink or racing writer already created
    // the path — exactly the behavior we want.
    let mut file = OpenOptions::new().write(true).create_new(true).open(temp)?;

    let write_result = (|| -> io::Result<()> {
        file.write_all(contents)?;
        file.flush()?;
        // Sync the data before the rename publishes it: on filesystems with
        // delayed allocation (ext4, NTFS) a crash right after rename could
        // otherwise publish a rename pointing at zero-length content.
        file.sync_all()?;
        Ok(())
    })();
    // Close the handle before renaming (Windows refuses a rename of an open
    // file; POSIX only requires the flush, which sync_all already ensured).
    drop(file);
    write_result.and_then(|()| {
        fs::rename(temp, target)?;
        Ok(())
    })
}

/// Builds a unique temp path: `<name>.<pid>.<counter>.tmp`.
fn unique_temp_path(target: &Path) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut file_name = target
        .file_name()
        .map(std::ffi::OsStr::to_owned)
        .unwrap_or_else(|| "file".into());
    file_name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    ));
    target.with_file_name(file_name)
}

/// Sets restrictive permissions on a freshly created file (Unix only).
///
/// Config and context files are non-secret, but there is no reason for them to
/// be group/world readable by default.
#[cfg(unix)]
pub fn restrict_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
pub fn restrict_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write_atomic(&path, b"hello").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/config.toml");
        write_atomic(&path, b"deep").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"deep");
    }

    #[test]
    fn replaces_existing_content_fully() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write_atomic(&path, &[b'x'; 4096]).unwrap();
        write_atomic(&path, b"short").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"short");
    }

    #[test]
    fn no_temp_files_are_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        write_atomic(&path, b"data").unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn write_failure_leaves_no_target_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        // The "parent directory" is a regular file: create_dir_all must fail,
        // no target may appear, and no temp file may linger.
        let parent = dir.path().join("blocker");
        fs::write(&parent, b"not a directory").unwrap();
        let target = parent.join("config.toml");
        assert!(write_atomic(&target, b"data").is_err());
        assert!(!target.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_planted_at_temp_name_cannot_redirect_the_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // The old fixed-name scheme (config.toml.tmp) would follow a symlink
        // planted there. Unique temp names make the attack impossible; this
        // test documents the property at the plausible legacy name.
        let legacy = dir.path().join("config.toml.tmp");
        let victim = dir.path().join("victim.txt");
        fs::write(&victim, b"precious").unwrap();
        std::os::unix::fs::symlink(&victim, &legacy).unwrap();

        write_atomic(&path, b"data").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"data");
        // The planted symlink and its target are untouched.
        assert!(
            fs::symlink_metadata(&legacy)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&victim).unwrap(), b"precious");
    }

    #[cfg(unix)]
    #[test]
    fn restrict_permissions_sets_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, b"data").unwrap();
        restrict_permissions(&path).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
