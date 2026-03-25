use anyhow::Result;
use clap::Parser;

mod api;
mod commands;
mod config;

#[derive(Debug, Parser)]
#[command(name = "edgee", about = "Edgee CLI", version)]
struct Options {
    #[command(subcommand)]
    command: commands::Command,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Options::parse();
    commands::run(opts.command).await
}
