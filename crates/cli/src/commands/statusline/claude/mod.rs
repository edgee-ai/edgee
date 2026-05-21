//! `edgee statusline claude` — manage the Claude Code statusline integration.

pub mod doctor;
pub mod fix;
pub mod install;
pub mod toggle;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Install the user-level Claude Code integration (statusline + hook).
    Install(install::Options),
    /// Re-enable the integration after `disable` (alias for install + clears
    /// the disabled marker).
    Enable,
    /// Disable the integration: removes Edgee's statusline/hook from
    /// `~/.claude/settings.json` and prevents auto-install on future
    /// `edgee launch` calls.
    Disable,
    /// Diagnose project-level statusLine conflicts.
    Doctor(doctor::Options),
    /// Overlay Edgee on top of a conflicting project statusLine by writing
    /// `.claude/settings.local.json`.
    Fix(fix::Options),
}

pub async fn run(opts: Options) -> Result<()> {
    match opts.command {
        Command::Install(o) => install::run(o).await,
        Command::Enable => toggle::enable().await,
        Command::Disable => toggle::disable().await,
        Command::Doctor(o) => doctor::run(o).await,
        Command::Fix(o) => fix::run(o).await,
    }
}
