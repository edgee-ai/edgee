//! The Edgee CLI as a library.
//!
//! The `edgee` binary (`src/main.rs`) is a thin shell over these modules, and
//! other workspace crates (e.g. the macOS menubar app) reuse them directly —
//! notably [`config`] for the credentials/profile store and [`api`] for the
//! console API client — so there is a single source of truth for the on-disk
//! format and the server contract.

pub mod api;
pub mod commands;
pub mod config;
pub mod crypto;
pub mod git;
#[cfg(feature = "self-update")]
pub mod version_check;
