//! `edgee statusline kimi` — manage the Kimi Code statusline integration.
//!
//! Kimi Code's config is TOML, not JSON (`$KIMI_CODE_HOME/tui.toml`, default
//! `~/.kimi-code/tui.toml`, per
//! <https://moonshotai.github.io/kimi-code/en/configuration/config-files.html>).
//! `[status_line]` has two keys: `items` (built-in footer slots) and
//! `command` (an external command whose first stdout line replaces the
//! footer, fed a JSON snapshot on stdin — closely mirrors Claude Code's
//! `statusLine.command`). Only `command` is ever touched here; `items` and
//! everything else in the file survive untouched.

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
    /// Install the user-level Kimi Code statusline.
    Install(install::Options),
    /// Re-enable the integration after `disable`.
    Enable,
    /// Disable the integration and prevent auto-install on future
    /// `edgee launch kimi` calls.
    Disable,
}

pub async fn run(opts: Options) -> Result<()> {
    match opts.command {
        Command::Install(o) => install::run(o).await,
        Command::Enable => toggle::enable().await,
        Command::Disable => toggle::disable().await,
    }
}

pub(crate) fn tui_toml_path() -> Option<std::path::PathBuf> {
    crate::commands::launch::kimi::kimi_code_home().map(|h| h.join("tui.toml"))
}
