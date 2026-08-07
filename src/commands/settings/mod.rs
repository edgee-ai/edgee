use anyhow::Result;
use clap::builder::PossibleValuesParser;
use dialoguer::{theme::ColorfulTheme, Select};

use crate::commands::auth::login;

pub mod agent;
pub mod profile;

/// Coding agents whose keys can be configured. Single source of truth for
/// the interactive picker and the `--agent` value parser below.
///
/// Deliberately distinct from `edgee launch`'s target list: launch uses
/// `copilot-vscode` for today's GitHub Copilot (VS Code) target, reserving
/// bare `copilot` for a future Copilot CLI, see `commands/launch/README.md`.
/// Both map to the same provider key here via `copilot`, which is why this
/// list uses the bare form.
const PROVIDERS: &[&str] = &[
    "claude",
    "claude_desktop",
    "codebuddy",
    "codex",
    "codex_desktop",
    "opencode",
    "crush",
    "cursor",
    "copilot",
];

/// Pseudo-agent value selecting profile-wide (non-agent-specific) settings.
const PROFILE_TARGET: &str = "profile";

#[derive(Debug, clap::Parser)]
pub struct Options {
    /// Coding agent whose key to configure, or `profile` for profile-wide settings.
    /// Prompts to pick one if omitted.
    #[arg(value_parser = PossibleValuesParser::new(
        std::iter::once(PROFILE_TARGET).chain(PROVIDERS.iter().copied())
    ))]
    agent: Option<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    let target = match opts.agent {
        Some(a) => a,
        None => prompt_for_target()?,
    };

    if target == PROFILE_TARGET {
        return profile::run().await;
    }

    // Reuse the auth flow's org gate so an unauthenticated user gets a clear hint.
    login::ensure_org_selected().await?;
    agent::configure(&target, false).await
}

fn prompt_for_target() -> Result<String> {
    let items = std::iter::once("Profile settings".to_string())
        .chain(PROVIDERS.iter().map(|p| login::agent_label(p).to_string()))
        .collect::<Vec<_>>();

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("What do you want to configure?")
        .items(&items)
        .default(0)
        .interact()?;

    if selection == 0 {
        Ok(PROFILE_TARGET.to_string())
    } else {
        Ok(PROVIDERS[selection - 1].to_string())
    }
}
