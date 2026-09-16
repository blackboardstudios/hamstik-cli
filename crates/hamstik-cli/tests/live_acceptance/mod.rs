// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Opt-in live acceptance suite against a real Hamstik deployment's
//! documented Public API v1 (`/api/v1`).
//!
//! Entry points (each `#[test]` is `#[ignore]`d and only runs when the
//! required environment variables are present):
//!
//! - [`smoke::live_smoke_read_only`] — no writes at all.
//! - [`mutations::live_acceptance_full`] — the full lifecycle.
//!
//! See `README.md` in this directory for the runbook.

pub mod harness;
pub mod mutations;
pub mod smoke;
