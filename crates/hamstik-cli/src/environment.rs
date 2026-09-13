// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Process-environment access behind an injectable seam.
//!
//! Keeping environment reads behind [`Environment`] makes host/profile/token
//! precedence deterministic in unit tests.

use std::collections::BTreeMap;

/// Reads environment variables and terminal capabilities.
pub trait Environment: Send + Sync {
    /// Returns the value of an environment variable, if set.
    fn var(&self, key: &str) -> Option<String>;

    /// True when standard output is attached to an interactive terminal.
    fn stdout_is_terminal(&self) -> bool;

    /// True when standard input is attached to an interactive terminal.
    fn stdin_is_terminal(&self) -> bool;
}

/// Real environment backed by `std::env`.
pub struct SystemEnvironment;

impl Environment for SystemEnvironment {
    fn var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn stdout_is_terminal(&self) -> bool {
        use std::io::IsTerminal;
        std::io::stdout().is_terminal()
    }

    fn stdin_is_terminal(&self) -> bool {
        use std::io::IsTerminal;
        std::io::stdin().is_terminal()
    }
}

/// Deterministic environment used by tests.
#[derive(Debug, Default, Clone)]
pub struct MapEnvironment {
    vars: BTreeMap<String, String>,
    /// Whether stdin and stdout are both terminals.
    pub terminals: bool,
}

impl MapEnvironment {
    #[must_use]
    /// Creates an empty environment.
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    /// Sets one variable (builder style).
    pub fn with_var(mut self, key: &str, value: &str) -> Self {
        self.vars.insert(key.to_string(), value.to_string());
        self
    }
}

impl Environment for MapEnvironment {
    fn var(&self, key: &str) -> Option<String> {
        self.vars.get(key).cloned()
    }

    fn stdout_is_terminal(&self) -> bool {
        self.terminals
    }

    fn stdin_is_terminal(&self) -> bool {
        self.terminals
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn map_environment_reads_values() {
        let env = MapEnvironment::new().with_var("HAMSTIK_HOST", "https://x.test");
        assert_eq!(env.var("HAMSTIK_HOST").as_deref(), Some("https://x.test"));
        assert_eq!(env.var("MISSING"), None);
    }
}
