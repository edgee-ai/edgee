pub mod login;
pub mod status;

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Log in to Edgee
    Login(login::Options),
    /// Show authentication status
    Status(status::Options),
}

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    command: Command,
}

pub async fn run(opts: Options) -> anyhow::Result<()> {
    match opts.command {
        Command::Login(o) => login::run(o).await,
        Command::Status(o) => status::run(o).await,
    }
}
