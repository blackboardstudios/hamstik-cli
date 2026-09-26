// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Portable Agent Skill installation and update behavior.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

const FILES: [&str; 4] = [
    "SKILL.md",
    "references/automation.md",
    "references/diagnostics.md",
    "references/platform-features.md",
];

fn command(project: &TempDir, global_home: &Path) -> Command {
    let mut command = Command::cargo_bin("hamstik").unwrap();
    command.current_dir(project.path());
    command.env("HAMSTIK_SKILL_HOME", global_home);
    command
}

fn assert_complete_copy(destination: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/hamstik");
    for relative in FILES {
        assert_eq!(
            fs::read(destination.join(relative)).unwrap(),
            fs::read(source.join(relative)).unwrap(),
            "installed {relative} must match the canonical source"
        );
    }
}

#[test]
fn global_install_copies_entire_skill_and_checks_overridden_home() {
    let project = TempDir::new().unwrap();
    let global_home = project.path().join("custom-agents");
    command(&project, &global_home)
        .args(["agent", "skill", "install", "--global"])
        .assert()
        .success();

    let destination = global_home.join("skills/hamstik");
    assert_complete_copy(&destination);
    assert!(!global_home.join(".agents").exists());
    command(&project, &global_home)
        .args(["agent", "skill", "check"])
        .assert()
        .success();
}

#[test]
fn project_install_preserves_modified_reference_without_partial_replacement() {
    let project = TempDir::new().unwrap();
    let global_home = project.path().join("custom-agents");
    let destination = project.path().join(".agents/skills/hamstik");
    command(&project, &global_home)
        .args(["agent", "skill", "install"])
        .assert()
        .success();
    assert_complete_copy(&destination);

    let reference = destination.join("references/automation.md");
    fs::write(&reference, "local changes\n").unwrap();
    fs::remove_file(destination.join("SKILL.md")).unwrap();
    command(&project, &global_home)
        .args(["agent", "skill", "install"])
        .assert()
        .failure();
    assert!(!destination.join("SKILL.md").exists());
    assert_eq!(fs::read_to_string(&reference).unwrap(), "local changes\n");

    command(&project, &global_home)
        .args(["agent", "skill", "install", "--force"])
        .assert()
        .success();
    assert_complete_copy(&destination);
    command(&project, &global_home)
        .args(["agent", "skill", "check"])
        .assert()
        .success();
}
