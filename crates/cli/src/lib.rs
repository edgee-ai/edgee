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

use anyhow::Result;
use clap::Parser;

/// The top-level `edgee` CLI parser. Lives in the library so the full
/// argv-parsing chain is unit-testable (see `commands::launch`'s tests).
#[derive(Debug, Parser)]
#[command(name = "edgee", about = "Edgee CLI", version)]
pub struct Options {
    /// Profile to use
    #[arg(long, short = 'p')]
    pub profile: Option<String>,

    #[command(subcommand)]
    pub command: commands::Command,
}

/// Parse args and run the CLI. The `edgee` binary is a thin shell over this.
pub async fn run() -> Result<()> {
    let opts = Options::parse();

    // Resolve active profile in precedence order:
    // 1. --profile flag
    // 2. active_profile stored in the effective credentials file
    //    (local .edgee/credentials.toml if present, global otherwise)
    // 3. hardcoded fallback: "default"
    let profile = opts
        .profile
        .or_else(|| config::read_file().ok().and_then(|f| f.active_profile))
        .unwrap_or_else(|| "default".to_string());

    config::set_active_profile(profile);

    // Nudge about newer releases, except when the user is already updating.
    #[cfg(feature = "self-update")]
    if !matches!(opts.command, commands::Command::Update(_)) {
        version_check::maybe_notify().await;
    }

    commands::run(opts.command).await
}
