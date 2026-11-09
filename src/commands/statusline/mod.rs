//! `edgee statusline`: the Edgee statusline of GitHub Copilot CLI.
//!
//! Bare, it renders the line (what Copilot's `statusLine.command` runs). With
//! `--wrap` it renders next to a statusLine of the user's own. `copilot`
//! installs or removes it from Copilot's settings. Claude Code does not use
//! this: its inline UI is the Edgee mod (`mods/edgee/`).

pub mod copilot;
pub mod render;
pub mod wrap;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Options {
    /// Also run this shell command, and show its output next to Edgee's
    #[arg(long, value_name = "COMMAND", allow_hyphen_values = true)]
    wrap: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Install or remove the statusline in GitHub Copilot CLI's settings
    Copilot(copilot::Options),
}

pub async fn run(opts: Options) -> Result<()> {
    match (opts.command, opts.wrap) {
        (Some(Command::Copilot(o)), _) => copilot::run(o)?,
        (None, Some(command)) => wrap::run(&command).await,
        (None, None) => render::run().await,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    fn parse(args: &[&str]) -> Result<Options, clap::Error> {
        Options::try_parse_from(std::iter::once("edgee-statusline").chain(args.iter().copied()))
    }

    #[test]
    fn bare_invocation_renders() {
        let opts = parse(&[]).unwrap();
        assert!(opts.command.is_none() && opts.wrap.is_none());
    }

    #[test]
    fn wrap_takes_a_command_even_one_starting_with_a_dash() {
        assert_eq!(parse(&["--wrap", "echo hi"]).unwrap().wrap.as_deref(), Some("echo hi"));
        assert_eq!(parse(&["--wrap", "-x"]).unwrap().wrap.as_deref(), Some("-x"));
    }

    #[test]
    fn copilot_subtree_parses() {
        let opts = parse(&["copilot", "install", "--wrap"]).unwrap();
        assert!(matches!(
            opts.command,
            Some(Command::Copilot(copilot::Options {
                command: copilot::Command::Install { wrap: true },
            }))
        ));
        assert!(parse(&["copilot", "uninstall"]).is_ok());
    }

    #[test]
    fn wrap_and_subcommand_conflict() {
        assert!(parse(&["--wrap", "x", "copilot", "uninstall"]).is_err());
    }
}
