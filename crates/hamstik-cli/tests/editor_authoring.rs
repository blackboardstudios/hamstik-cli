// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// Contract tests assert with unwrap/expect by design.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! CLI-7: editor authoring, consistent stdin (`-`) conventions, and
//! HTTP-boundary proofs that the sent body matches the selected source.

use assert_cmd::Command;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base(server: &MockServer, dir: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("hamstik").expect("hamstik binary");
    cmd.env("HAMSTIK_CONFIG", dir.path().join("config.toml"));
    cmd.env("HAMSTIK_HOST", server.uri());
    cmd.env("HAMSTIK_TOKEN", "secret-token");
    cmd.env_remove("HAMSTIK_PROFILE");
    cmd.env_remove("HAMSTIK_ORG");
    cmd.env_remove("HAMSTIK_PROJECT");
    cmd.current_dir(dir.path());
    cmd.arg("--no-retry");
    cmd
}

fn work_item(status: &str, revision: i64) -> Value {
    json!({
        "id": "1", "key": "HAM-1", "projectId": "2", "title": "T", "description": null,
        "type": "task", "status": status, "priority": "low", "assignee": null, "reporter": null,
        "sprint": null, "parent": null, "labels": [],
        "storyPoints": null, "dueDate": null, "archivedAt": null,
        "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z", "revision": revision
    })
}

/// Installs a fake editor script that appends `content` to the file it is
/// given (the standard `$VISUAL <file>` contract).
fn install_editor(dir: &TempDir, name: &str, content: &str) -> String {
    let script = dir.path().join(name);
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s' '{}' >> \"$1\"\n",
            content.replace('\'', "'\\''")
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script.to_string_lossy().to_string()
}

/// Installs an editor that writes nothing (unchanged template).
fn install_noop_editor(dir: &TempDir) -> String {
    let script = dir.path().join("noop-editor.sh");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script.to_string_lossy().to_string()
}

/// Installs a failing editor that records the temp file path it was given.
fn install_failing_editor_recording(dir: &TempDir, record: &std::path::Path) -> String {
    let script = dir.path().join("fail-editor-record.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> {}\nexit 3\n",
            record.to_string_lossy().replace('"', "\\\"")
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script.to_string_lossy().to_string()
}

/// Editor authoring round-trips multiline Markdown + Unicode verbatim (after
/// template-header removal), and the sent body matches the edited file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn editor_authored_description_reaches_the_wire_verbatim() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-1\"")
                .set_body_json(work_item("todo", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_item("todo", 2)))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let editor = install_editor(
        &dir,
        "editor.sh",
        "# Heading\n\nÜnïcödé ✓ line\n\n```rust\nfn main() {}\n```\n",
    );
    let output = base(&server, &dir)
        .env("VISUAL", &editor)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "edit",
            "HAM-1",
            "--description-editor",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);

    // The wire body carries exactly the authored text (template header
    // removed; Markdown '#' headings and trailing newline preserved).
    let request = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.method.as_str() == "PATCH")
        .unwrap();
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        sent["description"], "# Heading\n\nÜnïcödé ✓ line\n\n```rust\nfn main() {}\n```\n",
        "the authored text must reach the wire verbatim (after template removal)"
    );
}

/// The author's content is sent byte-exactly; Unicode + Markdown round-trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn comment_editor_body_matches_the_edited_content() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "44444444-4444-4444-8444-444444444444",
            "workItemId": "33333333-3333-4333-8333-333333333333",
            "author": {"publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "S"},
            "body": "authored",
            "deleted": false,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "editedAt": null,
            "parentCommentId": null
        })))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let editor = install_editor(&dir, "editor.sh", "line one\nline two ✓\n");
    let output = base(&server, &dir)
        .env("VISUAL", &editor)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "work",
            "comment",
            "add",
            "HAM-1",
            "--body-editor",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);

    let request = &server.received_requests().await.unwrap()[0];
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["body"], "line one\nline two ✓\n");
    // Idempotency still applies to comment creation.
    assert!(request.headers.contains_key("idempotency-key"));
}

/// Unchanged/empty editor content is a cancellation: usage error, nothing sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unchanged_editor_content_cancels_without_sending() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let editor = install_noop_editor(&dir);
    let output = base(&server, &dir)
        .env("VISUAL", &editor)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "--json",
            "work",
            "comment",
            "add",
            "HAM-1",
            "--body-editor",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("cancellation"),
        "{body}"
    );
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "a cancellation must not send a mutation"
    );
}

/// A failing editor (nonzero exit) is a clean usage error with no mutation,
/// and the temp file the editor was given is removed afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failing_editor_leaves_nothing_behind() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let record = dir.path().join("recorded-path.txt");
    let editor = install_failing_editor_recording(&dir, &record);
    let output = base(&server, &dir)
        .env("VISUAL", &editor)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "--json",
            "work",
            "comment",
            "add",
            "HAM-1",
            "--body-editor",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("failure status"),
        "{body}"
    );
    assert!(server.received_requests().await.unwrap().is_empty());

    // The temp file the editor saw no longer exists after the run.
    let recorded_path = std::fs::read_to_string(&record).unwrap();
    assert!(
        !std::path::Path::new(recorded_path.trim()).exists(),
        "editor temp file was not removed: {recorded_path}"
    );
}

/// `--no-input` rejects editor-only invocation immediately with a usage
/// error, before any network activity.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_input_rejects_editor_authoring() {
    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "--no-input",
            "--json",
            "work",
            "comment",
            "add",
            "HAM-1",
            "--body-editor",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let body: Value = serde_json::from_slice(&output.stderr).unwrap();
    let message = body["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("--no-input"), "{message}");
    assert!(message.contains("--file -"), "{message}");
    assert!(server.received_requests().await.unwrap().is_empty());
}

/// Source conflicts are rejected at parse time (clap) with exit 2.
#[test]
fn source_conflicts_fail_at_parse_time() {
    let dir = TempDir::new().unwrap();
    for conflicting in [
        vec!["--body", "inline", "--body-editor"],
        vec!["--body", "inline", "--body-file", "-"],
        vec!["--body-file", "-", "--body-editor"],
    ] {
        let mut args = vec![
            "--org".to_string(),
            "acme".to_string(),
            "--project".to_string(),
            "HAM".to_string(),
            "--no-input".to_string(),
            "--json".to_string(),
            "work".to_string(),
            "comment".to_string(),
            "add".to_string(),
            "HAM-1".to_string(),
        ];
        args.extend(conflicting.iter().map(std::string::ToString::to_string));
        let output = Command::cargo_bin("hamstik")
            .unwrap()
            .env("HAMSTIK_CONFIG", dir.path().join("config.toml"))
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(2),
            "conflicting sources must fail at parse time: {args:?}"
        );
    }
}

/// Editor temp files are not world-readable: the helper creates them 0600 on
/// Unix (best-effort on Windows).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn editor_temp_files_are_owner_restricted() {
    // The editor module removes its file before we can inspect it, so verify
    // the contract differently: no hamstik-edit file ever appears with broad
    // permissions during a run. Run a real editor that records its own mode.
    let dir = TempDir::new().unwrap();
    let mode_recorder = dir.path().join("record-mode.sh");
    std::fs::write(
        &mode_recorder,
        "#!/bin/sh\nstat -c '%a' \"$1\" > /tmp/kilo-editor-mode.txt\nprintf 'authored\n' >> \"$1\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&mode_recorder, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let _ = std::fs::remove_file("/tmp/kilo-editor-mode.txt");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1/comments",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "44444444-4444-4444-8444-444444444444",
            "workItemId": "33333333-3333-4333-8333-333333333333",
            "author": {"publicId": "usr_cPbfeqnghA-RLpDVOMQhHg", "name": "S"},
            "body": "authored",
            "deleted": false,
            "createdAt": "2026-01-01T00:00:00Z",
            "updatedAt": "2026-01-01T00:00:00Z",
            "editedAt": null,
            "parentCommentId": null
        })))
        .mount(&server)
        .await;
    let output = base(&server, &dir)
        .env("VISUAL", mode_recorder)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "--json",
            "work",
            "comment",
            "add",
            "HAM-1",
            "--body-editor",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{:?}", output.stderr);
    #[cfg(unix)]
    {
        let mode = std::fs::read_to_string("/tmp/kilo-editor-mode.txt").unwrap();
        assert_eq!(
            mode.trim(),
            "600",
            "editor temp files must be owner-only (0600)"
        );
    }
}

/// `--description-editor` and `--description-file` accept the documented
/// stdin convention `-`: piped stdin is consumed only when explicitly
/// selected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stdin_is_consumed_only_when_explicitly_selected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("ETag", "\"wi-1\"")
                .set_body_json(work_item("todo", 1)),
        )
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(
            "/api/v1/organizations/acme/projects/HAM/work-items/HAM-1",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(work_item("todo", 2)))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "--json",
            "work",
            "edit",
            "HAM-1",
            "--title",
            "T2",
        ])
        .write_stdin("should not be consumed without --description-file -")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let request = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|request| request.method.as_str() == "PATCH")
        .unwrap();
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    assert!(
        sent.get("description").is_none(),
        "stdin must not be consumed without the explicit '-' source: {sent}"
    );

    // Explicit `-` reads stdin. (This run shares the server, so select the
    // most recent PATCH.)
    let dir = TempDir::new().unwrap();
    let output = base(&server, &dir)
        .args([
            "--org",
            "acme",
            "--project",
            "HAM",
            "--json",
            "work",
            "edit",
            "HAM-1",
            "--description-file",
            "-",
        ])
        .write_stdin("piped description")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let request = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method.as_str() == "PATCH")
        .last()
        .unwrap();
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(sent["description"], "piped description");
}
