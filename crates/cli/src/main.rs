use anyhow::Result;
use clap::Parser;

mod api;
mod commands;
mod config;

#[derive(Debug, Parser)]
#[command(name = "edgee", about = "Edgee CLI", version)]
struct Options {
    /// Target environment: production (default), staging, or dev
    #[arg(long, global = true, default_value = "production")]
    env: config::Env,
    #[command(subcommand)]
    command: commands::Command,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Options::parse();
    config::set_env(opts.env);
    commands::run(opts.command).await
}
