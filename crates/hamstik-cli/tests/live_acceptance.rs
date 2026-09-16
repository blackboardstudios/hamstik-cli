// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

// The live acceptance suite is opt-in (`#[ignore]`d tests gated behind
// required environment variables); it compiles everywhere and never runs in
// offline `cargo test`.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![allow(missing_docs)]

#[path = "live_acceptance/mod.rs"]
mod live_acceptance;
