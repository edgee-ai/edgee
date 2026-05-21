use anyhow::Result;
use clap::Parser;

use edgee_gateway_core::Region;

mod api;
mod commands;
mod config;
mod git;
mod local_gateway;

#[derive(Debug, Parser)]
#[command(name = "edgee", about = "Edgee CLI", version)]
struct Options {
    /// Profile to use
    #[arg(long, short = 'p', global = true)]
    profile: Option<String>,

    /// Data residency region for the hosted gateway (us, eu, apac).
    /// Routes traffic through region-specific Fastly POPs.
    /// Falls back to US if the requested region is unavailable.
    #[arg(long, global = true, value_parser = parse_region)]
    region: Option<Region>,

    #[command(subcommand)]
    command: commands::Command,
}

/// Parse a region value from the CLI, supporting common aliases.
fn parse_region(s: &str) -> Result<Region, String> {
    Region::parse(s).ok_or_else(|| {
        format!(
            "invalid region '{s}'. Supported regions: {}",
            Region::ALL
                .iter()
                .map(|r| r.short_code())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
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

    // Apply --region CLI flag to the active profile if provided.
    // This persists the choice so downstream commands (launch, etc.)
    // pick it up via config::region().
    if let Some(region) = opts.region {
        let mut creds = config::read()?;
        creds.region = Some(region);
        config::write(&creds)?;
    }

    commands::run(opts.command).await
}
