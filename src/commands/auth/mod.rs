pub mod login;

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Log in to Edgee
    Login(login::Options),
}

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    command: Command,
}

pub async fn run(opts: Options) -> anyhow::Result<()> {
    match opts.command {
        Command::Login(o) => login::run(o).await,
    }
}
