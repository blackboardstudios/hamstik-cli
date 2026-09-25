// Copyright 2026 Blackboard Studios
// SPDX-License-Identifier: Apache-2.0

//! Hamstik CLI implementation crate.
//!
//! `main.rs` is a thin entrypoint; all logic lives here behind [`app::run`] so
//! it can be unit- and integration-tested without spawning the process.

pub mod app;
pub mod args;
pub mod audit;
pub mod banner;
pub mod build_info;
pub mod config;
pub mod context;
pub mod credentials;
pub mod disambiguate;
pub mod editor;
pub mod environment;
pub mod error;
pub mod exit;
pub mod fsutil;
pub mod input;
pub mod output;
pub mod palette;
pub mod terminal;
pub(crate) mod terminal_image;

pub mod commands;
pub mod time_arg;

pub use error::{CliError, ErrorKind};
pub use exit::SUCCESS;
