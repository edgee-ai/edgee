use anyhow::Result;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the copilot CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    crate::commands::relay::run_for_agent_with_args("copilot-cli", &opts.args).await
}
