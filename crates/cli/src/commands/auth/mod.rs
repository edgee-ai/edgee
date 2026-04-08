pub mod list;
pub mod login;
pub mod status;
pub mod switch;

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Log in to Edgee
    Login(login::Options),
    /// Show authentication status
    Status(status::Options),
    /// List all configured profiles
    List(list::Options),
    /// Switch the active profile globally
    Switch(switch::Options),
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
        Command::List(o) => list::run(o).await,
        Command::Switch(o) => switch::run(o).await,
    }
}
