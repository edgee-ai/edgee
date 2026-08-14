//! `edgee plugins` — see what the org has assigned to you.
//!
//! Read-only by design. A plugin is an admin's decision applied to a fleet: a
//! targeted member receives it on the next launch and has no opt-out, so there
//! is nothing here for them to change.

use anyhow::{Context, Result};

use crate::commands::auth::login;
use crate::config;

mod list;

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// List the plugins your organization has assigned to you
    List(list::Options),
}

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    command: Option<Command>,
}

pub async fn run(opts: Options) -> Result<()> {
    match opts.command {
        // Bare `edgee plugins` is the listing — the thing people want most.
        None => list::run(list::Options::default()).await,
        Some(Command::List(o)) => list::run(o).await,
    }
}

/// Resolves the signed-in user's org, failing with the same guidance the rest of
/// the CLI gives when there is no session.
pub(crate) async fn org_context() -> Result<(String, String)> {
    login::ensure_org_selected().await?;
    let creds = config::read()?;
    let token = creds
        .user_token
        .context("You are not signed in. Run `edgee auth login` first.")?;
    let org_id = creds
        .org_id
        .context("No organization selected. Run `edgee auth login` first.")?;
    Ok((token, org_id))
}
