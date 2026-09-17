// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Read-only live smoke scenario: proves auth, TLS, context resolution, and
//! the documented Public API v1 read routes work end-to-end against the real
//! test deployment, without creating anything.

use crate::live_acceptance::harness::AcceptanceEnv;
use std::env;

/// Read-only smoke suite entry point (`cargo test --test live_acceptance --
/// --test-threads=1 -- ignored`).
#[test]
#[ignore = "opt-in live acceptance; requires HAMSTIK_TOKEN + HAMSTIK_HOST + \
           HAMSTIK_ACCEPTANCE_ORG + HAMSTIK_ACCEPTANCE_PROJECT"]
fn live_smoke_read_only() {
    if env::var("HAMSTIK_TOKEN").is_err()
        || env::var("HAMSTIK_HOST").is_err()
        || env::var("HAMSTIK_ACCEPTANCE_ORG").is_err()
        || env::var("HAMSTIK_ACCEPTANCE_PROJECT").is_err()
    {
        return;
    }
    let env = AcceptanceEnv::load();
    run_smoke(&env);
    env.cleanup_config();
}

fn run_smoke(env: &AcceptanceEnv) {
    // Credential and context resolution against the real server.
    env.ok(&["me"]);
    env.ok(&["context", "show", "--explain"]);

    // Organization read routes.
    let orgs = env.ok(&["org", "list", "--limit", "20"]);
    let org = env.org.clone();
    assert!(
        orgs.contains(org.as_str()),
        "org list did not contain {org:?}"
    );
    env.ok(&["org", "view", &env.org]);
    env.ok(&["org", "members", &env.org, "--limit", "5"]);

    // Project read routes.
    let projects = env.ok(&["project", "list", "--limit", "50"]);
    let project = env.project.clone();
    assert!(
        projects.contains(project.as_str()),
        "project list did not contain {project:?}"
    );
    env.ok(&["project", "view", &env.project]);
    env.ok(&[
        "project",
        "activity",
        "--project",
        &env.project,
        "--limit",
        "5",
    ]);

    // Sprint/label collections.
    env.ok(&["sprint", "list", "--limit", "10"]);
    env.ok(&["label", "list", "--limit", "20"]);

    // Work Item collections and filters.
    env.ok(&["work", "list", "--limit", "5"]);
    env.ok(&["work", "list", "--scope", "open", "--limit", "5"]);
    env.ok(&[
        "work",
        "list",
        "--type",
        "story",
        "--priority",
        "high",
        "--limit",
        "5",
    ]);
    env.ok(&["work", "my", "--limit", "5"]);
    env.ok(&["work", "list", "--scope", "closed", "--limit", "5"]);
    env.ok(&["org", "work", "--limit", "5"]);
    env.ok(&["org", "members", &env.org, "--limit", "5"]);

    // Cursor pagination: --all follows every page internally; the test never
    // touches an opaque cursor.
    env.ok(&["work", "list", "--limit", "1", "--all"]);

    // SqueakQL validation (offline path) and the API contract probe.
    env.ok(&[
        "squeakql",
        "validate",
        "project.key = \"hamstik\" and status = \"open\"",
    ]);
    env.ok(&["api", "openapi"]);
    env.ok(&["api", "request", "/api/v1/me"]);
}
