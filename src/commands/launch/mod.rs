pub mod claude;
pub mod codex;

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Launch Claude Code routed through Edgee
    Claude(claude::Options),
    /// Launch Codex routed through Edgee
    Codex(codex::Options),
}

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    command: Command,
}

pub async fn run(opts: Options) -> anyhow::Result<()> {
    match opts.command {
        Command::Claude(o) => claude::run(o).await,
        Command::Codex(o) => codex::run(o).await,
    }
}
