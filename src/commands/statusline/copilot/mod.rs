//! `edgee statusline copilot`: manage the GitHub Copilot CLI statusline
//! integration.
//!
//! Copilot CLI reads a Claude-style `statusLine` command from its user
//! `settings.json` and pipes session JSON to it. The renderer ignores that
//! payload and keys off the `EDGEE_*` env `edgee launch copilot-cli` sets, so
//! plain `copilot` sessions render nothing.
//!
//! There is no `doctor`/`fix` pair as for Claude: Copilot keeps
//! `statusLine.command` in its protected settings (with `copilotUrl` and
//! `storeTokenPlaintext`), so there is no per-project shadowing to diagnose.

pub mod install;
pub mod toggle;

use std::path::PathBuf;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Install the statusline in Copilot CLI's user settings.
    Install(install::Options),
    /// Re-enable the integration after `disable`.
    Enable,
    /// Remove Edgee's statusline from Copilot CLI's settings and stop
    /// `edgee launch copilot-cli` from installing it again.
    Disable,
}

pub async fn run(opts: Options) -> Result<()> {
    match opts.command {
        Command::Install(o) => install::run(o).await,
        Command::Enable => toggle::enable().await,
        Command::Disable => toggle::disable().await,
    }
}

/// `$COPILOT_HOME/settings.json`, defaulting to `~/.copilot/settings.json`.
pub fn settings_path() -> PathBuf {
    let root = match std::env::var_os("COPILOT_HOME").filter(|v| !v.is_empty()) {
        Some(home) => PathBuf::from(home),
        None => {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .unwrap_or_default();
            PathBuf::from(home).join(".copilot")
        }
    };
    root.join("settings.json")
}
