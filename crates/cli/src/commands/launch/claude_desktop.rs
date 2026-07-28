use anyhow::Result;

#[derive(Debug, clap::Parser)]
pub struct Options {}

pub async fn run(_opts: Options) -> Result<()> {
    crate::commands::relay::run_for_agent("claude-desktop").await
}
