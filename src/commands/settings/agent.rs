use anyhow::{Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, MultiSelect};

use crate::api::{ApiClient, Compression, KeySettings};
use crate::commands::auth::login;

/// Runs the compression settings wizard for a single provider and
/// persists the result. Shared by `edgee settings` and first-run onboarding;
/// `first_run` switches the intro to a welcome banner.
pub async fn configure(provider: &str, first_run: bool) -> Result<()> {
    let label = login::agent_label(provider);

    let creds = crate::config::read()?;
    let user_token = creds
        .user_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Not authenticated. Run `edgee auth login` first."))?;
    let org_id = creds
        .org_id
        .as_deref()
        .filter(|o| !o.is_empty())
        .ok_or_else(|| anyhow::anyhow!("No organization selected. Run `edgee auth login` first."))?;
    let client = ApiClient::new(user_token)?;

    // get-or-create both guarantees the key exists (so we have a key_id) and
    // returns its current server-side settings, which we use to pre-fill the wizard.
    let key = login::fetch_provider_key(provider).await?;
    let current = key.compression.unwrap_or_default();

    // Pre-check tool surface reduction only for newly created Claude Code / Codex keys.
    let tsr_default_on = first_run && matches!(provider, "claude" | "codex");
    let settings = match run_settings_wizard(label, &current, first_run, tsr_default_on)? {
        Some(settings) => settings,
        None => {
            println!();
            println!("  {}", style("No changes made.").dim());
            return Ok(());
        }
    };

    client
        .update_key_settings(org_id, &key.id, &settings)
        .await
        .context("Failed to update key settings")?;

    print_summary(label, &settings);
    Ok(())
}

/// The default ColorfulTheme uses a `⬚` glyph for unchecked items that renders
/// as tofu/boxes in many terminals — use plain ASCII brackets instead.
fn brackets_theme() -> ColorfulTheme {
    ColorfulTheme {
        checked_item_prefix: style("[x]".to_string()).for_stderr().green(),
        unchecked_item_prefix: style("[ ]".to_string()).for_stderr().dim(),
        ..ColorfulTheme::default()
    }
}

/// Renders the settings editor pre-filled from the key's current state.
/// Returns the chosen settings, or `None` if the user aborts.
///
/// `tsr_default_on` pre-checks tool surface reduction for a freshly created
/// Claude Code / Codex key.
fn run_settings_wizard(
    agent: &str,
    current: &Compression,
    first_run: bool,
    tsr_default_on: bool,
) -> Result<Option<KeySettings>> {
    let theme = brackets_theme();

    println!();
    if first_run {
        println!("  {}", style(format!("Set up Edgee for {agent} 🎉")).bold());
        println!(
            "{}",
            style(
                "  Edgee compresses the token-heavy traffic between your coding agent\n  \
                 and the LLM provider, on the fly.\n\n  \
                 Space toggles a compression technique; enter confirms.\n"
            )
            .dim()
        );
    } else {
        println!(
            "  {}",
            style(format!("Configure Edgee settings for {agent}")).bold()
        );
        println!(
            "{}",
            style(
                "  Space toggles a compression technique; enter confirms.\n"
            )
            .dim()
        );
    }

    // --- Compression techniques ---
    let comp_options: [(&str, bool); 3] = [
        (
            "Tool results compression — trims verbose tool outputs",
            current.tool_result_trimming,
        ),
        (
            "Tool surface reduction — shrinks tool/MCP definitions sent to the model",
            // Pre-checked for a fresh Claude Code / Codex key; preserve the saved
            // choice when reconfiguring.
            tsr_default_on || current.tool_surface_reduction,
        ),
        (
            "Output brevity — nudges the model toward more concise responses",
            // Default ON for a fresh setup; preserve the saved choice when reconfiguring.
            first_run || current.output_brevity,
        ),
    ];
    let comp_items: Vec<&str> = comp_options.iter().map(|(l, _)| *l).collect();
    let comp_defaults: Vec<bool> = comp_options.iter().map(|(_, on)| *on).collect();

    let comp_selected = match MultiSelect::with_theme(&theme)
        .with_prompt("Compression techniques")
        .items(&comp_items)
        .defaults(&comp_defaults)
        .interact_opt()?
    {
        Some(s) => s,
        None => return Ok(None),
    };

    let compression = Compression {
        tool_result_trimming: comp_selected.contains(&0),
        tool_surface_reduction: comp_selected.contains(&1),
        output_brevity: comp_selected.contains(&2),
    };

    Ok(Some(KeySettings { compression }))
}

fn print_summary(agent: &str, settings: &KeySettings) {
    let on = |b: bool| if b { style("on").green() } else { style("off").dim() };
    println!();
    println!(
        "  {} {}",
        style("✓").green().bold(),
        style(format!("Settings updated for {agent}")).bold()
    );
    println!(
        "      tool results compression   {}",
        on(settings.compression.tool_result_trimming)
    );
    println!(
        "      tool surface reduction      {}",
        on(settings.compression.tool_surface_reduction)
    );
    println!(
        "      output brevity              {}",
        on(settings.compression.output_brevity)
    );
    println!();
}
