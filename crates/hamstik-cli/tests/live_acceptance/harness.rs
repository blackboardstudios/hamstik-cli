// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Harness for the opt-in live acceptance suite.
//!
//! The suite is gated behind required environment variables and only runs when
//! explicitly asked for (`cargo test --test live_acceptance -- --ignored`).
//! It drives the real CLI binary against a dedicated Hamstik test
//! Organization/Project over the documented Public API v1 (`/api/v1`).
//!
//! See `tests/live_acceptance/README.md` for the full runbook.

// Integration tests assert with unwrap/expect by design; the crate root of
// the test binary carries the lint allow, repeated here for clarity.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

/// The compiled `hamstik` binary under test.
const HAMSTIK_BIN: &str = env!("CARGO_BIN_EXE_hamstik");

/// The complete, validated live-acceptance environment.
pub struct AcceptanceEnv {
    /// The dedicated test PAT (ephemeral, never written to disk).
    pub token: String,
    /// The test deployment base URL.
    pub host: String,
    /// Dedicated test Organization slug.
    pub org: String,
    /// Dedicated test Project key.
    pub project: String,
    /// Run-unique marker: `acc-<unix-ts>-<pid>`, stamped on every created
    /// resource so cleanup removes exactly what this run created.
    pub marker: String,
    /// Throwaway temp dir backing `HAMSTIK_CONFIG` (never a real profile).
    pub temp_config: PathBuf,
    /// True when `HAMSTIK_ACCEPTANCE_MUTATIONS=true` grants the write scopes.
    mutations: bool,
}

impl AcceptanceEnv {
    /// Loads and validates the required environment variables. Exits the
    /// test process with an actionable message when anything is missing.
    pub fn load() -> Self {
        let missing = |name: &str| {
            eprintln!("live acceptance suite disabled: set {name} to enable it");
            std::process::exit(0);
        };
        let token = match env::var("HAMSTIK_TOKEN") {
            Ok(token) if !token.trim().is_empty() => token.trim().to_string(),
            _ => missing("HAMSTIK_TOKEN (a dedicated test PAT, never your real credential)"),
        };
        let host = match env::var("HAMSTIK_HOST") {
            Ok(host) if !host.trim().is_empty() => host.trim().trim_end_matches('/').to_string(),
            _ => missing("HAMSTIK_HOST (https://<test-instance>)"),
        };
        let org = match env::var("HAMSTIK_ACCEPTANCE_ORG") {
            Ok(org) if !org.trim().is_empty() => org.trim().to_string(),
            _ => missing("HAMSTIK_ACCEPTANCE_ORG (dedicated test Organization slug)"),
        };
        let project = match env::var("HAMSTIK_ACCEPTANCE_PROJECT") {
            Ok(project) if !project.trim().is_empty() => project.trim().to_string(),
            _ => missing("HAMSTIK_ACCEPTANCE_PROJECT (dedicated test Project key)"),
        };
        let mutations = match env::var("HAMSTIK_ACCEPTANCE_MUTATIONS") {
            Ok(value) => matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            Err(_) => false,
        };

        let marker = format!(
            "acc-{}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_secs(),
            std::process::id()
        );
        let temp_config =
            env::temp_dir().join(format!("hamstik-acceptance-{}", std::process::id()));
        fs::create_dir_all(&temp_config).expect("create throwaway config dir");
        // No env mutation here (the workspace forbids unsafe code): every CLI
        // invocation instead receives `HAMSTIK_CONFIG` (the throwaway dir) and
        // `HAMSTIK_TOKEN` explicitly, and never a real profile path.

        Self {
            token,
            host,
            org,
            project,
            marker,
            temp_config,
            mutations,
        }
    }

    /// Whether the run is allowed to mutate: the smoke scenario only reads,
    /// everything else requires explicit mutation grants.
    #[allow(dead_code)]
    pub fn mutations(&self) -> bool {
        self.mutations
    }

    /// Rejects a mutation-only scenario loudly when the run is read-only.
    pub fn require_mutations(&self) {
        if self.mutations {
            return;
        }
        panic!(
            "mutation scenario needs HAMSTIK_ACCEPTANCE_MUTATIONS=true \
             (plus a test PAT with write scopes); the default run is read-only"
        );
    }

    /// Runs the CLI with the ephemeral token; never logs the token or any
    /// Authorization header material.
    pub fn cli(&self, args: &[&str]) -> Output {
        let argv: Vec<&OsStr> = [
            HAMSTIK_BIN,
            "--host",
            self.host.as_str(),
            "--org",
            self.org.as_str(),
            "--project",
            self.project.as_str(),
            "--json",
            "--no-retry",
            "--no-input",
        ]
        .iter()
        .chain(args.iter())
        .copied()
        .map(OsStr::new)
        .collect();
        let output = Command::new(HAMSTIK_BIN)
            .args(argv.iter().copied())
            .env("HAMSTIK_TOKEN", &self.token)
            .env("HAMSTIK_CONFIG", &self.temp_config)
            .env_remove("HAMSTIK_PROFILE")
            .env_remove("HAMSTIK_HOST")
            .env_remove("HAMSTIK_ORG")
            .env_remove("HAMSTIK_PROJECT")
            .env_remove("HAMSTIK_JSON")
            .env_remove("HAMSTIK_NO_RETRY")
            .env_remove("HAMSTIK_NO_INPUT")
            .env_remove("HAMSTIK_QUIET")
            .output()
            .expect("failed to run the hamstik CLI");
        self.log_call(&argv, &output);
        output
    }

    /// Diagnostics on stderr for every CLI invocation, never the token.
    fn log_call(&self, argv: &[&OsStr], output: &Output) {
        // Split argv entries that contain spaces into separate tokens so the
        // logged command line matches the real argv shape.
        let args: Vec<String> = argv
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .map(String::from)
            .collect();
        let summary = format!(
            "\n[hamstik] {}\n[hamstik]   exit: {}",
            args.join(" "),
            output.status.code().unwrap_or(-1)
        );
        eprintln!("{summary}");
        let clip = |text: &str| {
            text.lines()
                .take(30)
                .map(|line| line.chars().take(600).collect::<String>())
                .collect::<Vec<_>>()
                .join(" | ")
        };
        if !output.stdout.is_empty() {
            eprintln!(
                "[hamstik]   stdout: {}",
                clip(&String::from_utf8_lossy(&output.stdout))
            );
        }
        if !output.stderr.is_empty() {
            eprintln!(
                "[hamstik]   stderr: {}",
                clip(&String::from_utf8_lossy(&output.stderr))
            );
        }
    }

    /// Runs the CLI and asserts success, returning stdout as a `String`.
    pub fn ok(&self, args: &[&str]) -> String {
        let output = self.cli(args);
        assert!(
            output.status.success(),
            "command failed: `hamstik {}` (see stderr diagnostics above)",
            args.join(" ")
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    /// Runs the CLI expecting failure, returning `(status, stdout, stderr)`.
    pub fn fails(&self, args: &[&str]) -> (bool, String, String) {
        let output = self.cli(args);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        (!output.status.success(), stdout, stderr)
    }

    /// Runs the CLI, parses stdout as JSON, and extracts a dotted path
    /// (object keys and integer array indices, e.g. `value.0.title`).
    /// Empty-string when the path does not resolve.
    pub fn json(&self, args: &[&str], path: &str) -> String {
        let stdout = self.ok(args);
        let value = serde_json::from_str::<serde_json::Value>(&stdout)
            .unwrap_or_else(|err| panic!("CLI stdout was not JSON: {err}\n{stdout}"));
        let mut cursor = &value;
        for part in path.split('.') {
            if let Ok(index) = part.parse::<usize>() {
                cursor = &cursor[index];
            } else {
                cursor = cursor.get(part).unwrap_or(&serde_json::Value::Null);
            }
        }
        match cursor {
            serde_json::Value::String(text) => text.clone(),
            // A missing path resolves to Null; represent that as empty so
            // callers can use `.is_empty()` checks.
            serde_json::Value::Null => String::new(),
            other => other.to_string(),
        }
    }

    /// Writes a temp file (mode 0666) and returns its path.
    pub fn temp_file(&self, name: &str, content: &[u8]) -> PathBuf {
        let path = self.temp_config.join(name);
        fs::write(&path, content).expect("write temp file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).expect("set mode");
        path
    }

    /// Removes the throwaway config dir (and any temp files) at exit.
    #[allow(dead_code)]
    pub fn cleanup_config(&self) {
        let _ = fs::remove_dir_all(&self.temp_config);
    }
}
