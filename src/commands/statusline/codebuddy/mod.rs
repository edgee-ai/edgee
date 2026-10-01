//! `edgee statusline codebuddy` — manage the CodeBuddy statusline
//! integration.

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
    /// Install the user-level CodeBuddy statusline.
    Install(install::Options),
    /// Re-enable the integration after `disable`.
    Enable,
    /// Disable the integration and prevent auto-install on future
    /// `edgee launch codebuddy` calls.
    Disable,
}

pub async fn run(opts: Options) -> Result<()> {
    match opts.command {
        Command::Install(o) => install::run(o).await,
        Command::Enable => toggle::enable().await,
        Command::Disable => toggle::disable().await,
    }
}
