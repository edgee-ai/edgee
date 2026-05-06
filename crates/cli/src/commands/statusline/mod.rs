//! `edgee statusline` — render the Edgee statusline, optionally merged with a
//! wrapped command's output, plus management subcommands for per-agent
//! integrations (currently `claude`).
//!
//! Default action when invoked with no subcommand and no flags is to render
//! Edgee's statusline segment. This is the form used in Claude Code's
//! `statusLine.command` setting.

pub mod claude;
pub mod render;
pub mod wrap;
pub mod width;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
#[command(args_conflicts_with_subcommands = true)]
pub struct Options {
    /// **Deprecated** — use `edgee statusline wrap <COMMAND>` instead. Kept
    /// as a hidden flag so already-deployed `.claude/settings.local.json`
    /// overlays written by `edgee fix` continue to work.
    #[arg(long, value_name = "COMMAND", hide = true)]
    pub wrap: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Render the Edgee statusline segment (default action).
    Render,
    /// Run a command through the platform shell and merge its output with
    /// Edgee's. Used as an overlay in `.claude/settings.local.json` to
    /// coexist with a project's own statusLine.
    Wrap {
        /// The shell command to run alongside Edgee's renderer.
        #[arg(required = true)]
        command: String,
    },
    /// Manage the Claude Code statusline integration.
    Claude(claude::Options),
}

pub async fn run(opts: Options) -> Result<()> {
    if let Some(cmd) = opts.wrap {
        return wrap::run(cmd).await;
    }
    match opts.command {
        None | Some(Command::Render) => render::run().await,
        Some(Command::Wrap { command }) => wrap::run(command).await,
        Some(Command::Claude(o)) => claude::run(o).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_bare_invocation_as_render() {
        let opts = Options::try_parse_from(["edgee-statusline"]).unwrap();
        assert!(opts.wrap.is_none());
        assert!(opts.command.is_none());
    }

    #[test]
    fn parses_legacy_dash_dash_wrap_flag() {
        let opts =
            Options::try_parse_from(["edgee-statusline", "--wrap", "echo hi"]).unwrap();
        assert_eq!(opts.wrap.as_deref(), Some("echo hi"));
    }

    #[test]
    fn parses_new_wrap_subcommand() {
        let opts = Options::try_parse_from(["edgee-statusline", "wrap", "echo hi"]).unwrap();
        assert!(opts.wrap.is_none());
        assert!(matches!(
            opts.command,
            Some(Command::Wrap { ref command }) if command == "echo hi"
        ));
    }

    #[test]
    fn parses_claude_subtree() {
        let opts =
            Options::try_parse_from(["edgee-statusline", "claude", "doctor"]).unwrap();
        assert!(matches!(
            opts.command,
            Some(Command::Claude(claude::Options {
                command: claude::Command::Doctor(_),
            }))
        ));
    }
}
