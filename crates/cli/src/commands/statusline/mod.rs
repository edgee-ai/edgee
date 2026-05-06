//! `edgee statusline` — render the Edgee statusline, optionally merged with a
//! wrapped command's output, plus management subcommands for per-agent
//! integrations (currently `claude`).
//!
//! Bare invocation (`edgee statusline` with no flags or subcommand) prints
//! help. The actual renderer used by Claude Code's `statusLine.command` is
//! `edgee statusline render`.

pub mod claude;
pub mod render;
pub mod wrap;
pub mod width;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
#[command(args_conflicts_with_subcommands = true, arg_required_else_help = true)]
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
    /// Render the Edgee statusline segment. Used by Claude Code's
    /// `statusLine.command` setting.
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
        Some(Command::Render) => render::run().await,
        Some(Command::Wrap { command }) => wrap::run(command).await,
        Some(Command::Claude(o)) => claude::run(o).await,
        None => {
            // Unreachable: `arg_required_else_help` makes clap exit with help
            // before we get here.
            unreachable!("clap should have printed help when no args/subcommand were given")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn bare_invocation_errors_with_help() {
        let err = Options::try_parse_from(["edgee-statusline"]).unwrap_err();
        // clap returns a "DisplayHelpOnMissingArgumentOrSubcommand" kind for
        // `arg_required_else_help`. We don't need to inspect the kind — the
        // important behaviour is that bare invocation does NOT yield a parsed
        // `Options` struct, so we can't accidentally render anything.
        let rendered = err.to_string();
        assert!(
            rendered.contains("Usage:") || rendered.contains("USAGE:"),
            "expected help text in error: {rendered}"
        );
    }

    #[test]
    fn parses_legacy_dash_dash_wrap_flag() {
        let opts =
            Options::try_parse_from(["edgee-statusline", "--wrap", "echo hi"]).unwrap();
        assert_eq!(opts.wrap.as_deref(), Some("echo hi"));
    }

    #[test]
    fn parses_render_subcommand() {
        let opts = Options::try_parse_from(["edgee-statusline", "render"]).unwrap();
        assert!(opts.wrap.is_none());
        assert!(matches!(opts.command, Some(Command::Render)));
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
