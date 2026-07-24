use anyhow::Result;
use dialoguer::{theme::ColorfulTheme, Select};

use crate::commands::auth::login;

pub mod agent;
pub mod profile;

/// Coding agents whose keys can be configured. Order is reused for the interactive picker.
const PROVIDERS: &[&str] = &["claude", "codebuddy", "codex", "opencode", "crush"];

/// Pseudo-agent value selecting profile-wide (non-agent-specific) settings.
const PROFILE_TARGET: &str = "profile";

#[derive(Debug, clap::Parser)]
pub struct Options {
    /// Coding agent whose key to configure, or `profile` for profile-wide settings.
    /// Prompts to pick one if omitted.
    #[arg(value_parser = [
        "profile",
        "claude",
        "codebuddy",
        "codex",
        "opencode",
        "crush",
    ])]
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
