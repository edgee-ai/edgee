//! `edgee statusline`: render the Edgee statusline, optionally merged with a
//! wrapped command's output, plus management subcommands for the GitHub
//! Copilot CLI integration.
//!
//! Bare invocation (`edgee statusline` with no subcommand) prints help. The
//! actual renderer used by Copilot CLI's `statusLine.command` is
//! `edgee statusline render`. Claude Code no longer uses it: its inline UI is
//! the Edgee mod (`mods/edgee/`).

pub mod copilot;
pub mod render;
pub mod settings;
pub mod width;
pub mod wrap;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
#[command(arg_required_else_help = true)]
pub struct Options {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Render the Edgee statusline segment. Used by Copilot CLI's
    /// `statusLine.command` setting.
    Render,
    /// Run a command through the platform shell and merge its output with
    /// Edgee's. Used to coexist with a statusLine of your own.
    Wrap {
        /// The shell command to run alongside Edgee's renderer.
        #[arg(required = true)]
        command: String,
    },
    /// Manage the GitHub Copilot CLI statusline integration.
    Copilot(copilot::Options),
}

pub async fn run(opts: Options) -> Result<()> {
    match opts.command {
        Command::Render => render::run().await,
        Command::Wrap { command } => wrap::run(command).await,
        Command::Copilot(o) => copilot::run(o).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn bare_invocation_errors_with_help() {
        let err = Options::try_parse_from(["edgee-statusline"]).unwrap_err();
        let rendered = err.to_string();
        assert!(
            rendered.contains("Usage:") || rendered.contains("USAGE:"),
            "expected help text in error: {rendered}"
        );
    }

    #[test]
    fn parses_render_subcommand() {
        let opts = Options::try_parse_from(["edgee-statusline", "render"]).unwrap();
        assert!(matches!(opts.command, Command::Render));
    }

    #[test]
    fn parses_wrap_subcommand() {
        let opts = Options::try_parse_from(["edgee-statusline", "wrap", "echo hi"]).unwrap();
        assert!(matches!(
            opts.command,
            Command::Wrap { ref command } if command == "echo hi"
        ));
    }

    #[test]
    fn parses_copilot_subtree() {
        let opts =
            Options::try_parse_from(["edgee-statusline", "copilot", "install", "--wrap"]).unwrap();
        assert!(matches!(
            opts.command,
            Command::Copilot(copilot::Options {
                command: copilot::Command::Install(copilot::install::Options { wrap: true, .. }),
            })
        ));
    }
}
