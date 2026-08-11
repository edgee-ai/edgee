//! `edgee plugins` — see what the org has given you, and opt into what it offered.
//!
//! Enforced plugins are org policy and are not the member's to decline; optional
//! ones are opt-in and this is where a terminal user does that without opening
//! the console.

use anyhow::{Context, Result};
use console::style;

use crate::api::{ApiClient, Plugin, PluginInstallOutcome};
use crate::commands::auth::login;
use crate::config;

mod list;

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// List the plugins your organization has assigned to you
    List(list::Options),
    /// Install an optional plugin that has been offered to you
    Install(InstallOptions),
    /// Remove an optional plugin you previously installed
    #[command(visible_alias = "uninstall")]
    Remove(InstallOptions),
}

#[derive(Debug, clap::Parser)]
pub struct InstallOptions {
    /// Plugin name (as shown by `edgee plugins`), or its id
    name: String,
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
        Some(Command::Install(o)) => set_installed(o, true).await,
        Some(Command::Remove(o)) => set_installed(o, false).await,
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

/// Matches by name first, then by id. Names are unique per org, so there is no
/// ambiguity to resolve — this is purely so both work.
pub(crate) fn find<'a>(plugins: &'a [Plugin], needle: &str) -> Option<&'a Plugin> {
    plugins
        .iter()
        .find(|p| p.name == needle)
        .or_else(|| plugins.iter().find(|p| p.id == needle))
}

async fn set_installed(opts: InstallOptions, installed: bool) -> Result<()> {
    let (token, org_id) = org_context().await?;
    let client = ApiClient::new(&token)?;

    let plugins = client.list_plugins(&org_id).await?;
    let Some(plugin) = find(&plugins, &opts.name) else {
        // The API would 404 anyway, but resolving locally lets us say which
        // name was not found rather than echoing a bare status code.
        println!();
        println!(
            "  {} No plugin named {} is assigned to you.",
            style("✗").red().bold(),
            style(&opts.name).bold()
        );
        println!("  {}", style("Run `edgee plugins` to see what you have.").dim());
        println!();
        std::process::exit(1);
    };

    let title = plugin.title().to_string();
    let outcome = client
        .set_plugin_installed(&org_id, &plugin.id, installed)
        .await?;

    println!();
    match outcome {
        PluginInstallOutcome::Updated(updated) => {
            let verb = if installed { "Installed" } else { "Removed" };
            // Report the server's view, not the one we listed a moment ago.
            println!(
                "  {} {verb} {}",
                style("✓").green().bold(),
                style(updated.title()).bold()
            );
            // Every agent reads this config at startup, and `--plugin-dir` is
            // session-scoped, so a running session cannot pick it up.
            println!(
                "  {}",
                style("Applies the next time you launch a coding agent.").dim()
            );
        }
        // Enforced plugins are org policy. Print the server's own wording rather
        // than inventing a second explanation that could drift from it.
        PluginInstallOutcome::Refused(message) => {
            println!("  {} {message}", style("✗").red().bold());
            println!();
            std::process::exit(1);
        }
        PluginInstallOutcome::NotTargeted => {
            println!(
                "  {} {} is not assigned to you.",
                style("✗").red().bold(),
                style(&title).bold()
            );
            println!();
            std::process::exit(1);
        }
    }
    println!();
    Ok(())
}
