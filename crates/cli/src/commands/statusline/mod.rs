//! `edgee statusline` — render the Edgee statusline, optionally merged with a
//! wrapped command's output.
//!
//! - `edgee statusline` (no flags): renders only Edgee's segment. Used as the
//!   default `statusLine.command` in `~/.claude/settings.json`.
//! - `edgee statusline --wrap '<cmd>'`: runs `<cmd>` through the platform
//!   shell in parallel with Edgee's renderer and merges the two outputs.
//!   Used as an overlay in `.claude/settings.local.json` to coexist with a
//!   project's own statusLine.
//!
//! See `wrap.rs` for the merge contract — Edgee's segment is always emitted
//! and is never the one that gets truncated.

pub mod render;
pub mod wrap;
pub mod width;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
pub struct Options {
    /// Wrap an existing statusline command and merge its output with Edgee's.
    /// The wrapped command is executed via the platform shell.
    #[arg(long, value_name = "COMMAND")]
    pub wrap: Option<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    if let Some(cmd) = opts.wrap {
        wrap::run(cmd).await
    } else {
        render::run().await
    }
}
