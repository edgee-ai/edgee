//! PHPStorm's GitHub Copilot plugin through the Copilot subscription relay.

use std::path::PathBuf;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Project paths and arguments forwarded to PHPStorm
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub async fn run(opts: Options, reroute: &super::reroute::Reroute) -> Result<()> {
    binary()?;
    crate::commands::relay::run_for_agent_with_args("phpstorm", &opts.args, reroute).await
}

pub(crate) fn binary() -> Result<PathBuf> {
    super::intellij::jetbrains_binary("PHPStorm", "EDGEE_PHPSTORM_BINARY", &["PhpStorm.app", "PHPStorm.app"], "phpstorm")
}
