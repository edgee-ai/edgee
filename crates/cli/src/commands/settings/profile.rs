//! Profile-wide (non-agent-specific) settings, reached via `edgee settings
//! profile`. Currently holds only the E2EE debug-log passphrase; more
//! profile-global settings can be added as further menu items.

use anyhow::{Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Password, Select};

use crate::commands::launch::util;
use crate::crypto::DebugLogKeypair;

pub async fn run() -> Result<()> {
    let items = ["Debug-log encryption passphrase"];
    match Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Profile settings")
        .items(items)
        .default(0)
        .interact_opt()?
    {
        Some(0) => debug_log_flow(),
        _ => Ok(()),
    }
}

fn debug_log_flow() -> Result<()> {
    let creds = crate::config::read()?;
    let configured = creds
        .debug_log_e2ee_passphrase
        .as_deref()
        .is_some_and(|s| !s.is_empty());
    let env_active = util::env_passphrase_set();

    println!();
    if env_active {
        println!(
            "  {} set via {} (takes precedence over any profile setting).",
            style("Active:").green().bold(),
            util::ENV_VAR
        );
    } else if configured {
        println!("  {} configured for this profile.", style("Active:").green().bold());
    } else {
        println!(
            "  {} not configured — debug logs upload as plaintext.",
            style("Inactive:").yellow().bold()
        );
    }
    println!();

    let action = match Select::with_theme(&ColorfulTheme::default())
        .with_prompt("What do you want to do?")
        .items(["Set new passphrase", "Clear passphrase", "Cancel"])
        .default(0)
        .interact_opt()?
    {
        Some(a) => a,
        None => return Ok(()),
    };

    match action {
        0 => set_passphrase(configured),
        1 => clear_passphrase(configured),
        _ => Ok(()),
    }
}

fn set_passphrase(already_configured: bool) -> Result<()> {
    if already_configured {
        let proceed = Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(
                "A passphrase is already set. Replacing it makes any previously uploaded \
                 encrypted debug logs permanently unreadable. Continue?",
            )
            .default(false)
            .interact()?;
        if !proceed {
            println!("  Aborted. Existing passphrase left unchanged.");
            return Ok(());
        }
    }

    let passphrase = Password::with_theme(&ColorfulTheme::default())
        .with_prompt("Enter debug-log encryption passphrase")
        .with_confirmation("Confirm passphrase", "Passphrases didn't match")
        .interact()?;

    if passphrase.is_empty() {
        anyhow::bail!("Passphrase cannot be empty.");
    }

    // Fail fast: don't persist a passphrase whose derivation would later hard-error
    // at launch time (resolve_debug_log_keypair's contract: never silently fall
    // back to plaintext).
    DebugLogKeypair::derive(&passphrase)
        .context("failed to derive debug-log encryption key from passphrase")?;

    let mut creds = crate::config::read()?;
    creds.debug_log_e2ee_passphrase = Some(passphrase);
    crate::config::write(&creds)?;

    println!();
    println!("  {} Debug-log encryption passphrase set.", style("✓").green().bold());
    if util::env_passphrase_set() {
        println!(
            "  {} {} is set in your shell and takes precedence — this profile \
             setting is ignored until you unset it.",
            style("Warning:").yellow().bold(),
            util::ENV_VAR
        );
    }
    println!();
    Ok(())
}

fn clear_passphrase(already_configured: bool) -> Result<()> {
    if !already_configured {
        println!("  No debug-log passphrase is configured.");
        return Ok(());
    }

    let proceed = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt("Clear the debug-log passphrase? Future debug logs upload as plaintext.")
        .default(false)
        .interact()?;
    if !proceed {
        println!("  Aborted.");
        return Ok(());
    }

    let mut creds = crate::config::read()?;
    creds.debug_log_e2ee_passphrase = None;
    crate::config::write(&creds)?;
    println!("  {} Debug-log passphrase cleared.", style("✓").green().bold());
    Ok(())
}
