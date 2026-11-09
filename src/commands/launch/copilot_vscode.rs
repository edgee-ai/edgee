use anyhow::Result;

#[derive(Debug, clap::Parser)]
pub struct Options {}

pub async fn run(_opts: Options, reroute: &super::pause::Controls) -> Result<()> {
    crate::commands::relay::run_for_agent("copilot-vscode", reroute).await
}
