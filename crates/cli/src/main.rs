use anyhow::Result;
use clap::Parser;

mod api;
mod commands;
mod config;
mod git;

#[derive(Debug, Parser)]
#[command(name = "edgee", about = "Edgee CLI", version)]
struct Options {
    /// Profile to use
    #[arg(long, short = 'p', global = true)]
    profile: Option<String>,

    #[command(subcommand)]
    command: commands::Command,
}

#[tokio::main]
async fn main() -> Result<()> {
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

    commands::run(opts.command).await
}
