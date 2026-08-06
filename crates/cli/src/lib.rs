//! The Edgee CLI as a library.
//!
//! The `edgee` binary (`src/main.rs`) is a thin shell over these modules;
//! exposing them as a library keeps the internals (credential store, console
//! API client) unit-testable and available to future in-process consumers.
//!
//! The macOS menubar app (`apps/menubar/`) is a separate Swift app — it does
//! *not* link this crate. It drives the CLI as a subprocess and consumes the
//! `--json` output of `auth status`/`auth list`/`auth orgs`/`stats`, so the
//! Rust CLI stays the single source of truth for the on-disk format and the
//! server contract.

pub mod api;
pub mod commands;
pub mod config;
pub mod crypto;
pub mod git;
#[cfg(feature = "self-update")]
pub mod version_check;
