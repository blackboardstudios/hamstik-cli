// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Full-lifecycle live acceptance scenario (writes, transitions, conflict
//! handling, bulk, cleanup). Requires `HAMSTIK_ACCEPTANCE_MUTATIONS=true` and
//! a test PAT with write scopes.

use crate::live_acceptance::harness::AcceptanceEnv;
use std::env;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::PathBuf;

/// Mutation suite entry point (`cargo test --test live_acceptance -- ignored`).
#[test]
#[ignore = "opt-in live acceptance mutations; requires the read env vars plus \
           HAMSTIK_ACCEPTANCE_MUTATIONS=true"]
fn live_acceptance_mutations() {
    if env::var("HAMSTIK_TOKEN").is_err()
        || env::var("HAMSTIK_HOST").is_err()
        || env::var("HAMSTIK_ACCEPTANCE_ORG").is_err()
        || env::var("HAMSTIK_ACCEPTANCE_PROJECT").is_err()
    {
        return;
    }
    let env = AcceptanceEnv::load();
    // `run_mutations` always cleans up created Work Items before resuming any
    // panic, so the primary assertion failure is never masked by leftover data.
    let outcome = catch_unwind(AssertUnwindSafe(|| run_mutations(&env)));
    if let Err(payload) = outcome {
        resume_unwind(payload);
    }
    env.cleanup_config();
}

fn run_mutations(env: &AcceptanceEnv) {
    env.require_mutations();
    // The cleanup registry: every Work Item this run creates is removed
    // exactly once, even when a later assertion fails.
    let mut created: Vec<String> = Vec::new();
    let result = catch_unwind(AssertUnwindSafe(|| {
        scenario(env, &mut created);
    }));
    cleanup_work_items(env, &created);
    if let Err(payload) = result {
        resume_unwind(payload);
    }
}

fn scenario(env: &AcceptanceEnv, created: &mut Vec<String>) {
    // ---- Create with an explicit idempotency key; replay returns the same
    // ---- Work Item (server-side idempotency, SPEC §76).
    let title_a = format!("{} acceptance main", env.marker);
    let idempotency_key = format!("{}-main", env.marker);
    let create_args = [
        "work",
        "create",
        "--title",
        title_a.as_str(),
        "--type",
        "task",
        "--priority",
        "medium",
        "--idempotency-key",
        idempotency_key.as_str(),
    ];
    let first_id = env.json(&create_args, "key");
    assert!(
        !first_id.is_empty(),
        "create did not return a Work Item key"
    );
    let replay_id = env.json(&create_args, "key");
    assert_eq!(
        first_id, replay_id,
        "idempotent replay created a second item"
    );
    created.push(first_id.clone());
    let key = first_id.clone();

    // ---- View, context, activity (read-after-write).
    let view = env.json(&["work", "view", &key], "title");
    assert_eq!(view, title_a, "work view title mismatch");
    env.ok(&["work", "context", &key, "--json"]);
    env.ok(&["work", "activity", &key, "--limit", "5"]);

    // ---- Edit (server-side If-Match protection exercised by the CLI).
    let edit_title = format!("{title_a} edited");
    env.ok(&["work", "edit", &key, "--title", &edit_title]);
    let view = env.json(&["work", "view", &key], "title");
    assert_eq!(view, edit_title, "edit did not stick");

    // ---- Real ETag conflict: a stale If-Match must be rejected by the
    // ---- server, then --force (If-Match: *) succeeds.
    let path = format!(
        "/api/v1/organizations/{}/projects/{}/work-items/{}",
        env.org, env.project, key
    );
    let (failed, _, stderr) = env.fails(&[
        "api",
        "request",
        &path,
        "--method",
        "PATCH",
        "--header",
        "If-Match: stale-etag",
        "--field",
        "title=conflict edit",
    ]);
    assert!(
        failed,
        "PATCH with a stale If-Match was expected to fail with a revision \
         conflict but succeeded"
    );
    assert!(
        stderr.contains("conflict") || stderr.contains("409") || stderr.contains("412"),
        "expected a revision-conflict diagnostic, got: {stderr}"
    );
    env.ok(&[
        "work",
        "edit",
        &key,
        "--title",
        &format!("{edit_title} forced"),
        "--force",
    ]);

    // ---- Transitions: list allowed states, then walk start → close.
    env.ok(&["work", "transitions", &key]);
    env.ok(&["work", "start", &key]);
    let status = env.json(&["work", "view", &key], "status");
    assert_eq!(status, "in_progress", "work start did not move the item");
    env.ok(&["work", "close", &key]);
    let status = env.json(&["work", "view", &key], "status");
    assert_eq!(status, "done", "work close did not move the item");

    // ---- Labels: reuse an existing project label; if the project has none,
    // ---- create one with the run marker (labels cannot be deleted via the
    // ---- Public API v1, so this is the only documented residue).
    let mut label_name = env.json(&["label", "list", "--limit", "50"], "items.0.name");
    if label_name.is_empty() {
        label_name = format!("{}-label", env.marker);
        env.ok(&[
            "label",
            "create",
            "--name",
            &label_name,
            "--color",
            "#3b82f6",
        ]);
    }
    env.ok(&["work", "label", "add", &key, "--label", &label_name]);
    env.ok(&["work", "label", "remove", &key, "--label", &label_name]);

    // ---- Comments: add, reply, edit, delete the reply, delete the parent.
    let comment_id = env.json(
        &[
            "work",
            "comment",
            "add",
            &key,
            "--body",
            "acceptance root comment",
        ],
        "id",
    );
    assert!(!comment_id.is_empty(), "comment add returned no id");
    let reply_id = env.json(
        &[
            "work",
            "comment",
            "add",
            &key,
            "--body",
            "acceptance reply",
            "--parent",
            &comment_id,
        ],
        "id",
    );
    assert!(!reply_id.is_empty(), "reply add returned no id");
    env.ok(&[
        "work",
        "comment",
        "edit",
        &key,
        &comment_id,
        "--body",
        "edited root",
    ]);
    env.ok(&["work", "comment", "delete", &key, &reply_id]);
    env.ok(&["work", "comment", "delete", &key, &comment_id]);

    // ---- Links between two items, then delete the link.
    let title_b = format!("{} acceptance linked", env.marker);
    let key_b = env.json(
        &["work", "create", "--title", &title_b, "--type", "task"],
        "key",
    );
    created.push(key_b.clone());
    env.ok(&[
        "work",
        "link",
        "add",
        &key,
        "--target-key",
        &key_b,
        "--relation",
        "blocks",
    ]);
    let link_id = env.json(&["work", "link", "list", &key], "items.0.id");
    assert!(!link_id.is_empty(), "link list returned no id");
    env.ok(&["work", "link", "delete", &key, &link_id]);

    // ---- Watcher state.
    env.ok(&["work", "watcher", "show", &key]);
    env.ok(&["work", "watcher", "watch", &key]);
    env.ok(&["work", "watcher", "mute", &key]);
    env.ok(&["work", "watcher", "unwatch", &key]);

    // ---- Attachments: upload, verify byte-for-byte download, delete.
    let attachment = env.temp_file("acceptance-upload.txt", b"live acceptance payload");
    let upload_name = format!("{}-upload.txt", env.marker);
    let attachment_id = env.json(
        &[
            "work",
            "attachment",
            "upload",
            &key,
            attachment.to_str().expect("temp path"),
            "--file-name",
            &upload_name,
            "--content-type",
            "text/plain",
        ],
        "id",
    );
    assert!(
        !attachment_id.is_empty(),
        "attachment upload returned no id"
    );
    let download = PathBuf::from(format!(
        "{}/downloaded-{}.txt",
        env.temp_config.display(),
        env.marker
    ));
    env.ok(&[
        "work",
        "attachment",
        "download",
        &key,
        &attachment_id,
        "--output",
        download.to_str().expect("temp path"),
    ]);
    assert_eq!(
        std::fs::read(&download).expect("read download"),
        b"live acceptance payload",
        "attachment download content mismatch"
    );
    env.ok(&["work", "attachment", "delete", &key, &attachment_id]);

    // ---- Sprints: create (future), walk active → done. Sprints cannot be
    // ---- deleted via the Public API v1; the uniquely named sprint is the
    // ---- only residue (documented in README.md).
    let sprint_name = format!("{} sprint", env.marker);
    let sprint_id = env.json(&["sprint", "create", "--name", &sprint_name], "id");
    assert!(!sprint_id.is_empty(), "sprint create returned no id");
    env.ok(&["sprint", "transitions", &sprint_id]);
    env.ok(&["sprint", "transition", &sprint_id, "active", "--force"]);
    env.ok(&[
        "sprint",
        "transition",
        &sprint_id,
        "done",
        "--force",
        "--move-to-backlog",
    ]);

    // ---- Bulk create/update/transition with per-item results (run once;
    // ---- bulk create is not idempotent, so no replay here).
    let ops = env.temp_file(
        "bulk-create.json",
        format!(
            "{{\"operations\":[{{\"projectKey\":\"{}\",\"title\":\"{} bulk one\"}},\
              {{\"projectKey\":\"{}\",\"title\":\"{} bulk two\"}}]}}",
            env.project, env.marker, env.project, env.marker
        )
        .as_bytes(),
    );
    let ops_path = ops.to_str().expect("temp path");
    let bulk_key_one = env.json(
        &["work", "bulk", "create", "--operations-file", ops_path],
        "results.0.workItem.key",
    );
    let bulk_key_two = env.json(
        &["work", "bulk", "create", "--operations-file", ops_path],
        "results.1.workItem.key",
    );
    assert!(
        !bulk_key_one.is_empty() && !bulk_key_two.is_empty(),
        "bulk create returned no keys"
    );
    created.push(bulk_key_one.clone());
    created.push(bulk_key_two.clone());
    // Bulk update with explicit per-item revisions (require-revision mode).
    let revision_one = env.json(&["work", "view", &bulk_key_one], "revision");
    let revision_two = env.json(&["work", "view", &bulk_key_two], "revision");
    let update_ops = env.temp_file(
        "bulk-update.json",
        format!(
            "{{\"concurrency\":\"require-revision\",\"operations\":[\
              {{\"projectKey\":\"{}\",\"workItemKey\":\"{}\",\"revision\":{},\"changes\":{{\"priority\":\"high\"}}}},\
              {{\"projectKey\":\"{}\",\"workItemKey\":\"{}\",\"revision\":{},\"changes\":{{\"priority\":\"high\"}}}}]}}",
            env.project, bulk_key_one, revision_one, env.project, bulk_key_two, revision_two
        )
        .as_bytes(),
    );
    env.ok(&[
        "work",
        "bulk",
        "update",
        "--operations-file",
        update_ops.to_str().expect("temp path"),
    ]);
    let bulk_transition = env.temp_file(
        "bulk-transition.json",
        format!(
            "{{\"concurrency\":\"last-write-wins\",\"operations\":[\
              {{\"projectKey\":\"{}\",\"workItemKey\":\"{}\",\"targetStatus\":\"todo\"}},\
              {{\"projectKey\":\"{}\",\"workItemKey\":\"{}\",\"targetStatus\":\"todo\"}}]}}",
            env.project, bulk_key_one, env.project, bulk_key_two
        )
        .as_bytes(),
    );
    env.ok(&[
        "work",
        "bulk",
        "transition",
        "--operations-file",
        bulk_transition.to_str().expect("temp path"),
    ]);

    // ---- SqueakQL inline search against the live server.
    env.ok(&["work", "search", &format!("title = \"{}\"", env.marker)]);
}

fn cleanup_work_items(env: &AcceptanceEnv, created: &[String]) {
    let mut failures: Vec<String> = Vec::new();
    for key in created {
        // Delete first (owner-scoped); fall back to archive when the test PAT
        // is not the Organization owner. Report anything that still leaks.
        let (deleted, _, _) = env.fails(&["work", "delete", key, "--force"]);
        if deleted {
            continue;
        }
        let (archived, _, _) = env.fails(&["work", "archive", key, "--force"]);
        if archived {
            failures.push(format!("{key}: neither delete nor archive succeeded"));
        }
    }
    if failures.is_empty() {
        return;
    }
    panic!(
        "cleanup failed for {} of {} created Work Items:\n  {}",
        failures.len(),
        created.len(),
        failures.join("\n  ")
    );
}
