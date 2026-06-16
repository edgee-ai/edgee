use anyhow::{Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, Input, MultiSelect, Select};

use crate::api::{ApiClient, Compression, GatewayModel, KeySettings, ModelRoute};
use crate::commands::auth::login;

/// Coding agents whose keys can be configured. Order is reused for the interactive picker.
const PROVIDERS: [&str; 3] = ["claude", "codex", "opencode"];

#[derive(Debug, clap::Parser)]
pub struct Options {
    /// Coding agent whose key to configure. Prompts to pick one if omitted.
    #[arg(value_parser = PROVIDERS)]
    agent: Option<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    // Reuse the auth flow's org gate so an unauthenticated user gets a clear hint.
    login::ensure_org_selected().await?;

    let provider = match opts.agent {
        Some(a) => a,
        None => prompt_for_provider()?,
    };
    let label = login::agent_label(&provider);

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
    let key = login::fetch_provider_key(&provider).await?;
    let current = CurrentSettings::from(&key);

    // The catalog feeds the fallback/reroute pickers. If it's unavailable we fall
    // back to free-text entry rather than blocking the whole command.
    let choices = match client.list_models().await {
        Ok(models) => model_choices(&models),
        Err(e) => {
            eprintln!(
                "  {} {}",
                style("Could not load the model catalog; entering models by hand.").yellow(),
                style(e).dim()
            );
            Vec::new()
        }
    };

    let settings = match run_settings_wizard(label, &current, &choices)? {
        Some(s) => s,
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

/// Current settings read back from the key, used as wizard defaults.
struct CurrentSettings {
    compression: Compression,
    fallbacks: Vec<String>,
    reroutes: Vec<String>,
}

impl From<&crate::api::ApiKeyItem> for CurrentSettings {
    fn from(key: &crate::api::ApiKeyItem) -> Self {
        CurrentSettings {
            compression: key.compression.unwrap_or_default(),
            fallbacks: key.fallbacks.iter().map(|r| r.model.clone()).collect(),
            reroutes: key.reroutes.iter().map(|r| r.model.clone()).collect(),
        }
    }
}

/// A selectable routing target: a friendly label plus the identifier sent to the API.
struct ModelChoice {
    label: String,
    identifier: String,
}

/// Builds selectable choices from the active catalog models, sorted by label.
fn model_choices(models: &[GatewayModel]) -> Vec<ModelChoice> {
    let mut out: Vec<ModelChoice> = models
        .iter()
        .filter(|m| m.active)
        .filter_map(|m| {
            m.route_identifier().map(|identifier| {
                let label = if m.display_name.is_empty() {
                    identifier.clone()
                } else {
                    format!("{} ({})", m.display_name, identifier)
                };
                ModelChoice { label, identifier }
            })
        })
        .collect();
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

fn prompt_for_provider() -> Result<String> {
    let items: Vec<&str> = PROVIDERS.iter().map(|p| login::agent_label(p)).collect();
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Which coding agent's key do you want to configure?")
        .items(&items)
        .default(0)
        .interact()?;
    Ok(PROVIDERS[selection].to_string())
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
fn run_settings_wizard(
    agent: &str,
    current: &CurrentSettings,
    choices: &[ModelChoice],
) -> Result<Option<KeySettings>> {
    let theme = brackets_theme();

    println!();
    println!(
        "  {}",
        style(format!("Configure Edgee settings for {agent}")).bold()
    );
    println!(
        "{}",
        style(
            r#"  Toggle with space, confirm with enter.
  Fallback and reroute require an assigned AI Gateway seat.
"#
        )
        .dim()
    );

    // --- Compression techniques ---
    let comp_options: [(&str, bool); 3] = [
        (
            "Tool results compression — trims verbose tool outputs",
            current.compression.tool_result_trimming,
        ),
        (
            "Tool surface reduction — shrinks tool/MCP definitions sent to the model",
            current.compression.tool_surface_reduction,
        ),
        (
            "Output brevity — nudges the model toward more concise responses",
            current.compression.output_brevity,
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

    // --- Fallback target models (failover) ---
    let fallbacks = match select_models(
        &theme,
        "Fallback models — fail over to these on errors (none = disabled)",
        choices,
        &current.fallbacks,
    )? {
        Some(v) => v,
        None => return Ok(None),
    };

    // --- Reroute target models ---
    let reroutes = match select_models(
        &theme,
        "Reroute models — send requests to these instead (none = disabled)",
        choices,
        &current.reroutes,
    )? {
        Some(v) => v,
        None => return Ok(None),
    };

    Ok(Some(KeySettings {
        compression: Compression {
            tool_result_trimming: comp_selected.contains(&0),
            tool_surface_reduction: comp_selected.contains(&1),
            output_brevity: comp_selected.contains(&2),
        },
        fallback: !fallbacks.is_empty(),
        fallbacks: to_routes(fallbacks),
        reroutes: to_routes(reroutes),
    }))
}

/// Prompts for a set of model identifiers, pre-selecting `current`.
///
/// With a catalog, this is a multiselect; any currently-configured identifier not
/// in the catalog is appended as an extra (pre-checked) choice so it's never
/// silently dropped. Without a catalog, it degrades to comma-separated text input.
/// Returns the chosen identifiers, or `None` if the user aborts.
fn select_models(
    theme: &ColorfulTheme,
    prompt: &str,
    choices: &[ModelChoice],
    current: &[String],
) -> Result<Option<Vec<String>>> {
    if choices.is_empty() {
        let initial = current.join(", ");
        let raw: String = Input::with_theme(theme)
            .with_prompt(format!("{prompt} (comma-separated)"))
            .with_initial_text(initial)
            .allow_empty(true)
            .interact_text()?;
        let models = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        return Ok(Some(models));
    }

    // Catalog identifiers, plus any current value the catalog doesn't know about.
    let mut identifiers: Vec<String> = choices.iter().map(|c| c.identifier.clone()).collect();
    let mut labels: Vec<String> = choices.iter().map(|c| c.label.clone()).collect();
    for cur in current {
        if !identifiers.iter().any(|i| i == cur) {
            identifiers.push(cur.clone());
            labels.push(format!("{cur} (not in catalog)"));
        }
    }
    let defaults: Vec<bool> = identifiers.iter().map(|i| current.contains(i)).collect();

    let selected = match MultiSelect::with_theme(theme)
        .with_prompt(prompt)
        .items(&labels)
        .defaults(&defaults)
        .interact_opt()?
    {
        Some(s) => s,
        None => return Ok(None),
    };

    Ok(Some(selected.into_iter().map(|i| identifiers[i].clone()).collect()))
}

/// Maps selected identifiers to the API's route list, using `None` (→ JSON null,
/// which clears the field) for an empty selection.
fn to_routes(models: Vec<String>) -> Option<Vec<ModelRoute>> {
    if models.is_empty() {
        None
    } else {
        Some(models.into_iter().map(|model| ModelRoute { model }).collect())
    }
}

fn print_summary(agent: &str, settings: &KeySettings) {
    let on = |b: bool| if b { style("on").green() } else { style("off").dim() };
    let routes = |r: &Option<Vec<ModelRoute>>| match r {
        Some(v) if !v.is_empty() => style(
            v.iter()
                .map(|m| m.model.clone())
                .collect::<Vec<_>>()
                .join(", "),
        )
        .cyan()
        .to_string(),
        _ => style("off").dim().to_string(),
    };
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
    println!("      fallback                    {}", routes(&settings.fallbacks));
    println!("      reroute                     {}", routes(&settings.reroutes));
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(active: bool, display: &str, aliases: &[&str], providers: &[&str]) -> GatewayModel {
        GatewayModel {
            model_id: "m1".to_string(),
            display_name: display.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            providers: providers
                .iter()
                .map(|p| (p.to_string(), serde_json::Value::Null))
                .collect(),
            active,
        }
    }

    #[test]
    fn model_choices_skip_inactive_and_sort_by_label() {
        let models = vec![
            model(true, "Zed", &["zed-1"], &["anthropic"]),
            model(false, "Hidden", &["hidden-1"], &["anthropic"]),
            model(true, "Ada", &["ada-1"], &["openai"]),
        ];
        let choices = model_choices(&models);
        let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, vec!["Ada (ada-1)", "Zed (zed-1)"]);
    }

    #[test]
    fn route_identifier_prefers_alias_then_provider_slash_model() {
        // Alias wins.
        assert_eq!(
            model(true, "X", &["fast"], &["openai"]).route_identifier(),
            Some("fast".to_string())
        );
        // No alias → lowest-sorted provider/model_id.
        assert_eq!(
            model(true, "X", &[], &["openai", "anthropic"]).route_identifier(),
            Some("anthropic/m1".to_string())
        );
        // Neither → none.
        assert_eq!(model(true, "X", &[], &[]).route_identifier(), None);
    }

    #[test]
    fn to_routes_maps_empty_to_none() {
        assert!(to_routes(vec![]).is_none());
        let routes = to_routes(vec!["a".to_string(), "b".to_string()]).unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].model, "a");
    }
}
