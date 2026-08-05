use std::collections::HashSet;

use anyhow::{Context, Result};
use console::style;
use dialoguer::{theme::ColorfulTheme, MultiSelect, Select};

use crate::api::{
    is_app_provider, ApiClient, Compression, GatewayModel, KeySettings, ModelRoute, ProviderKey,
};
use crate::commands::auth::login;

/// Runs the full settings wizard (compression + routing) for a single provider and
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
    let current = CurrentSettings::from(&key);

    // Routing (fallback/reroute) is only offered for Claude Code and Codex. OpenCode
    // is configured for compression only here, so we skip the routing picker and the
    // catalog/billing fetches it needs.
    let routing_enabled = matches!(provider, "claude" | "codex");

    let (is_paying, choices) = if routing_enabled {
        // Routing only takes effect with a paid plan. We still run the picker without
        // one — the user gets to choose, then sees the upsell. Unknown (request
        // failed) → assume access so we never nag a paying user; the server enforces
        // the seat requirement on save regardless.
        let is_paying = client.org_is_paying(org_id).await.unwrap_or(true);

        // BYOK keys let us tag routing targets reachable via the user's own provider
        // keys. Best-effort: an empty set just drops the tag rather than failing.
        let byok_providers = match client.list_provider_keys(org_id).await {
            Ok(keys) => byok_provider_set(&keys),
            Err(_) => HashSet::new(),
        };

        // The catalog feeds the routing picker. If it's unavailable, routing is
        // simply skipped this run rather than blocking the whole command.
        let mut catalog_loaded = true;
        let choices = match client.list_models().await {
            Ok(models) => route_choices(&models, &byok_providers, key.byok_only),
            Err(e) => {
                catalog_loaded = false;
                eprintln!(
                    "  {} {}",
                    style("Could not load the model catalog; skipping routing setup.").yellow(),
                    style(e).dim()
                );
                Vec::new()
            }
        };

        // byok_only rejects every model with no matching BYOK credentials, so a key
        // with the restriction has nothing to offer once none of its configured
        // provider keys (if any) match a routable model. Gated on the catalog
        // having actually loaded, so this doesn't pile on top of the error above.
        if catalog_loaded && key.byok_only && choices.is_empty() {
            eprintln!(
                "  {}",
                style(
                    "This key is BYOK-only but none of your provider keys match a routable model; skipping routing setup."
                )
                .yellow()
            );
        }
        (is_paying, choices)
    } else {
        (true, Vec::new())
    };

    let outcome =
        match run_settings_wizard(label, &current, &choices, is_paying, first_run, routing_enabled)?
        {
        Some(o) => o,
        None => {
            println!();
            println!("  {}", style("No changes made.").dim());
            return Ok(());
        }
    };

    client
        .update_key_settings(org_id, &key.id, &outcome.settings)
        .await
        .context("Failed to update key settings")?;

    print_summary(label, &outcome.settings);

    // A model was chosen but the org can't use routing yet — surface the upsell.
    if let Some(pending) = &outcome.upsell {
        print_upsell(pending, creds.org_slug.as_deref());
    }
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

/// A selectable routing target. `featured` models form the short list shown
/// first; the rest of the active catalog is reachable behind "+ More models…".
/// Featured normally means plan-covered, but under `byok_only` every surviving
/// choice is featured — there's nothing else to propose, so it shouldn't hide
/// behind "+ More models…".
struct RouteChoice {
    /// Display label, e.g. `Kimi K2.7 Turbo (kimi-k2.7-turbo)`.
    label: String,
    /// Identifier sent to the API.
    identifier: String,
    /// Shown in the short list rather than requiring "+ More models…".
    featured: bool,
    /// Turbo variant — surfaced first within the short list.
    turbo: bool,
    /// Included in the org's Edgee plan — billing tag "· plan". Independent of
    /// `featured`: under byok_only a model can be featured without being
    /// plan-covered.
    plan_covered: bool,
    /// Reachable via one of the user's BYOK keys — billing tag "· your key"
    /// when not plan-covered.
    byok: bool,
}

impl RouteChoice {
    /// Menu label annotated with how the model bills: included in the plan, billed
    /// to the user's own BYOK key, or charged against Edgee credits.
    fn menu_label(&self) -> String {
        let tag = if self.plan_covered {
            style("· plan").green()
        } else if self.byok {
            style("· your key").magenta()
        } else {
            style("· credits").yellow()
        };
        format!("{} {tag}", self.label)
    }
}

/// Builds routing choices from the active catalog, ordered featured-first (plan
/// models, turbos at the top), then the remaining models alphabetically. Mirrors the
/// console's route picker, which offers every active model and splits by
/// `plan_fallback` rather than filtering by premium status.
///
/// Models served only through a coding-app subscription (Cursor, GitHub Copilot)
/// are dropped: routing can't reach them, so offering `cursor/composer-2` or
/// `claude-opus-4-8-fast` as a fallback/reroute target would only produce a route
/// that fails at request time.
///
/// `byok_only` mirrors the key's server-side restriction (org/squad/key-resolved):
/// when set, the gateway rejects any request for a model with no matching BYOK
/// credentials, so such models are dropped here too rather than offered as a
/// route that would fail. Plan coverage no longer applies once byok_only forces
/// every request through the user's own keys, so every surviving choice is
/// tagged BYOK instead of plan — and, since there's nothing else to propose,
/// every surviving choice is also featured (short-listed) rather than buried
/// behind "+ More models…".
fn route_choices(
    models: &[GatewayModel],
    byok_providers: &HashSet<String>,
    byok_only: bool,
) -> Vec<RouteChoice> {
    let mut out: Vec<RouteChoice> = models
        .iter()
        .filter(|m| m.active)
        .filter(|m| !m.app_subscription_only())
        .filter(|m| !byok_only || model_uses_byok(m, byok_providers))
        .filter_map(|m| {
            m.route_identifier().map(|identifier| {
                let turbo = is_turbo(&identifier);
                let label = if m.display_name.is_empty() {
                    identifier.clone()
                } else {
                    format!("{} ({})", m.display_name, identifier)
                };
                let plan_covered = m.plan_fallback && !byok_only;
                RouteChoice {
                    label,
                    identifier,
                    // Plan-covered models are short-listed normally; under
                    // byok_only everything left has already been filtered down
                    // to BYOK-reachable models, so short-list all of them.
                    featured: byok_only || plan_covered,
                    turbo,
                    plan_covered,
                    // BYOK is shown only for non-plan models; plan coverage wins.
                    byok: !plan_covered && model_uses_byok(m, byok_providers),
                }
            })
        })
        .collect();
    // Featured first, turbos ahead of the rest within each group, then alphabetical.
    out.sort_by(|a, b| {
        b.featured
            .cmp(&a.featured)
            .then_with(|| b.turbo.cmp(&a.turbo))
            .then_with(|| a.label.cmp(&b.label))
    });
    out
}

/// Turbo variants (identifier ends with "turbo") are surfaced first, mirroring the
/// console's route picker.
fn is_turbo(identifier: &str) -> bool {
    identifier.to_lowercase().ends_with("turbo")
}

/// Set of provider bases covered by the org's active BYOK keys. Regional provider
/// names are folded to their base (`bedrock_us-east-1` → `bedrock`, `azure_*` →
/// `azure`) so they match the model catalog's provider keys.
fn byok_provider_set(keys: &[ProviderKey]) -> HashSet<String> {
    keys.iter()
        .filter(|k| k.active)
        .map(|k| resolve_provider_base(&k.provider))
        .collect()
}

/// Folds a (possibly regional) provider key to its BYOK base provider.
/// `bedrock-mantle` is a distinct provider from `bedrock` (separate endpoint),
/// so it folds to its own base rather than into `bedrock` — checked first since
/// `bedrock-mantle_<region>` would not otherwise match the `bedrock_` prefix.
fn resolve_provider_base(provider: &str) -> String {
    if provider.starts_with("bedrock-mantle_") {
        return "bedrock-mantle".to_string();
    }
    if provider.starts_with("bedrock_") {
        return "bedrock".to_string();
    }
    if provider.starts_with("azure_") {
        return "azure".to_string();
    }
    provider.to_string()
}

/// True when the model is reachable through at least one of the user's BYOK keys.
/// App-subscription providers don't count: a route never goes through Cursor or
/// Copilot, so such a key would never be the one billed.
fn model_uses_byok(model: &GatewayModel, byok_providers: &HashSet<String>) -> bool {
    if byok_providers.is_empty() {
        return false;
    }
    model
        .providers
        .keys()
        .filter(|p| !is_app_provider(p))
        .any(|p| byok_providers.contains(&resolve_provider_base(p)))
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

/// How Edgee should use a chosen routing model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteMode {
    /// Only when the usual model errors or is rate-limited (`fallbacks`).
    Fallback,
    /// Every request, instead of the usual model (`reroutes`).
    Reroute,
}

/// Result of the wizard: the settings to save, plus an optional upsell to show when
/// the user picked a routing model but the org has no plan to back it.
struct WizardOutcome {
    settings: KeySettings,
    upsell: Option<PendingRoute>,
}

/// A routing model the user chose but couldn't activate without a plan.
struct PendingRoute {
    label: String,
    mode: RouteMode,
}

/// Renders the settings editor pre-filled from the key's current state.
/// Returns the chosen settings, or `None` if the user aborts.
fn run_settings_wizard(
    agent: &str,
    current: &CurrentSettings,
    choices: &[RouteChoice],
    is_paying: bool,
    first_run: bool,
    routing_enabled: bool,
) -> Result<Option<WizardOutcome>> {
    let theme = brackets_theme();

    // The routing hint is only relevant for agents where we offer routing.
    let routing_hint = if routing_enabled {
        "\n  Routing sends traffic to another model — as a fallback (errors only) or a reroute (every request)."
    } else {
        ""
    };

    println!();
    if first_run {
        println!("  {}", style(format!("Set up Edgee for {agent} 🎉")).bold());
        println!(
            "{}",
            style(format!(
                "  Edgee compresses the token-heavy traffic between your coding agent\n  \
                 and the LLM provider, on the fly.\n\n  \
                 Space toggles a compression technique; enter confirms.{routing_hint}\n"
            ))
            .dim()
        );
    } else {
        println!(
            "  {}",
            style(format!("Configure Edgee settings for {agent}")).bold()
        );
        println!(
            "{}",
            style(format!(
                "  Space toggles a compression technique; enter confirms.{routing_hint}\n"
            ))
            .dim()
        );
    }

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
            // Default ON for a fresh setup; preserve the saved choice when reconfiguring.
            first_run || current.compression.output_brevity,
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

    // --- Model routing ---
    // Pre-select the model currently routed to (if any), and the current strategy.
    let current_model = current
        .reroutes
        .first()
        .or_else(|| current.fallbacks.first())
        .cloned();
    let current_idx = current_model
        .as_ref()
        .and_then(|id| choices.iter().position(|c| &c.identifier == id));
    let current_strategy = if !current.reroutes.is_empty() {
        RouteStrategy::Reroute
    } else if !current.fallbacks.is_empty() {
        RouteStrategy::Fallback
    } else {
        RouteStrategy::Passthrough
    };

    // Skip routing entirely when it isn't offered for this agent (OpenCode) or the
    // catalog couldn't be loaded — leave any existing routing untouched rather than
    // clearing it on a compression-only edit.
    if !routing_enabled || choices.is_empty() {
        return Ok(Some(WizardOutcome {
            settings: KeySettings {
                compression,
                fallback: !current.fallbacks.is_empty(),
                fallbacks: to_routes(&current.fallbacks),
                reroutes: to_routes(&current.reroutes),
            },
            upsell: None,
        }));
    }

    // Routes are rebuilt from scratch each run; passthrough (or no model chosen)
    // leaves them cleared.
    let mut settings = KeySettings {
        compression,
        fallback: false,
        fallbacks: None,
        reroutes: None,
    };
    let mut upsell = None;

    // Step 1 — routing strategy. Fallback keeps passthrough as the primary and only
    // uses the Edgee model on errors; reroute replaces the primary entirely.
    let strategy = match select_route_strategy(&theme, agent, current_strategy)? {
        Some(s) => s,
        None => return Ok(None),
    };

    if let Some(mode) = strategy.mode() {
        // Step 2 — pick the Edgee model the fallback/reroute points at.
        let picked = match select_route_model(&theme, choices, current_idx, is_paying)? {
            RouteSelection::Aborted => return Ok(None),
            RouteSelection::Skip => None,
            RouteSelection::Model(i) => Some(i),
        };

        if let Some(i) = picked {
            let identifier = choices[i].identifier.clone();
            let label = choices[i].label.clone();

            if is_paying {
                match mode {
                    RouteMode::Fallback => {
                        settings.fallback = true;
                        settings.fallbacks = Some(vec![ModelRoute { model: identifier }]);
                    }
                    RouteMode::Reroute => {
                        settings.reroutes = Some(vec![ModelRoute { model: identifier }]);
                    }
                }
            } else {
                // No plan: keep routes cleared (the server would reject them) and tell
                // the user what unlocking would buy them.
                upsell = Some(PendingRoute { label, mode });
            }
        }
    }

    Ok(Some(WizardOutcome { settings, upsell }))
}

/// How Edgee selects the model for a request. Mutually exclusive — note that
/// `Fallback` *coexists* with passthrough (primary stays your own model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteStrategy {
    /// Always use the agent's own model/billing — no Edgee model involved.
    Passthrough,
    /// Keep passthrough as the primary; use an Edgee model only on errors/limits.
    Fallback,
    /// Replace the primary entirely — every request goes to an Edgee model.
    Reroute,
}

impl RouteStrategy {
    /// The route mode this strategy maps to, or `None` for passthrough.
    fn mode(self) -> Option<RouteMode> {
        match self {
            RouteStrategy::Passthrough => None,
            RouteStrategy::Fallback => Some(RouteMode::Fallback),
            RouteStrategy::Reroute => Some(RouteMode::Reroute),
        }
    }
}

/// Asks which routing strategy to use, defaulting to the key's current state.
/// `None` on abort.
fn select_route_strategy(
    theme: &ColorfulTheme,
    agent: &str,
    current: RouteStrategy,
) -> Result<Option<RouteStrategy>> {
    // Index order must match the `from_index` mapping below.
    let strategies = [
        RouteStrategy::Passthrough,
        RouteStrategy::Fallback,
        RouteStrategy::Reroute,
    ];
    let items = [
        format!("Passthrough — always use your {agent} plan or API billing"),
        format!("Fallback — use your {agent} model, switch to an Edgee model on errors or rate-limits"),
        "Reroute — send every request to an Edgee model instead".to_string(),
    ];
    let default = strategies.iter().position(|&s| s == current).unwrap_or(0);

    let sel = match Select::with_theme(theme)
        .with_prompt("How should Edgee route your requests?")
        .items(&items)
        .default(default)
        .interact_opt()?
    {
        Some(s) => s,
        None => return Ok(None),
    };

    Ok(Some(strategies[sel]))
}

/// Outcome of the model picker.
enum RouteSelection {
    /// User aborted the whole wizard (Esc).
    Aborted,
    /// User chose not to route to another model.
    Skip,
    /// Index into `choices` of the selected model.
    Model(usize),
}

/// Prompts for a single routing model: a short list of plan-covered models (turbos
/// first) plus "+ More models…" to browse the full catalog. The
/// current route, if any, is pre-highlighted. Returns the choice or an abort.
fn select_route_model(
    theme: &ColorfulTheme,
    choices: &[RouteChoice],
    current_idx: Option<usize>,
    is_paying: bool,
) -> Result<RouteSelection> {
    let featured_len = choices.iter().filter(|c| c.featured).count();

    if is_paying {
        // Legend for the billing tags shown on each model.
        println!(
            "  {}",
            style(format!(
                "{} included in your plan   {} billed to your BYOK key   {} uses Edgee credits",
                style("· plan").green(),
                style("· your key").magenta(),
                style("· credits").yellow(),
            ))
            .dim()
        );
    } else {
        // No plan yet: the picker is a preview, so don't claim anything is included.
        println!(
            "  {}",
            style("Routing needs an Edgee plan — pick a model to preview what you'd unlock.").dim()
        );
    }

    loop {
        // Short list = featured models, plus the current route if it lives outside
        // the featured set (so enter keeps it instead of clearing it).
        let mut short: Vec<usize> = (0..featured_len).collect();
        if let Some(i) = current_idx {
            if i >= featured_len && !short.contains(&i) {
                short.push(i);
            }
        }

        let mut labels = vec!["None — stay on passthrough".to_string()];
        labels.extend(short.iter().map(|&i| choices[i].menu_label()));
        let more_idx = if choices.len() > short.len() {
            labels.push("+ More models…".to_string());
            Some(labels.len() - 1)
        } else {
            None
        };

        let default = current_idx
            .and_then(|ci| short.iter().position(|&i| i == ci))
            .map(|p| p + 1)
            .unwrap_or(0);

        let sel = match Select::with_theme(theme)
            .with_prompt("Which Edgee model should it use?")
            .items(&labels)
            .default(default)
            .interact_opt()?
        {
            Some(s) => s,
            None => return Ok(RouteSelection::Aborted),
        };

        if sel == 0 {
            return Ok(RouteSelection::Skip);
        }
        if Some(sel) == more_idx {
            match select_from_full_catalog(theme, choices, current_idx)? {
                FullSelection::Aborted => return Ok(RouteSelection::Aborted),
                FullSelection::Back => continue,
                FullSelection::Model(i) => return Ok(RouteSelection::Model(i)),
            }
        }
        return Ok(RouteSelection::Model(short[sel - 1]));
    }
}

/// Outcome of the full-catalog picker (reached via "+ More…").
enum FullSelection {
    Aborted,
    Back,
    Model(usize),
}

/// Shows the complete catalog (with a "← Back" escape hatch) so the user
/// can route to a model that isn't in the short list.
fn select_from_full_catalog(
    theme: &ColorfulTheme,
    choices: &[RouteChoice],
    current_idx: Option<usize>,
) -> Result<FullSelection> {
    let mut labels = vec!["← Back".to_string()];
    labels.extend(choices.iter().map(|c| c.menu_label()));
    let default = current_idx.map(|i| i + 1).unwrap_or(0);

    let sel = match Select::with_theme(theme)
        .with_prompt("All models")
        .items(&labels)
        .default(default)
        .max_length(12)
        .interact_opt()?
    {
        Some(s) => s,
        None => return Ok(FullSelection::Aborted),
    };

    if sel == 0 {
        Ok(FullSelection::Back)
    } else {
        Ok(FullSelection::Model(sel - 1))
    }
}

/// Prints the "you need a plan" upsell after the user selected a routing model.
fn print_upsell(pending: &PendingRoute, org_slug: Option<&str>) {
    let url = match org_slug {
        Some(slug) => format!(
            "{}/~/{}/settings/plans",
            crate::config::console_base_url(),
            slug
        ),
        None => format!("{}/pricing", crate::config::console_base_url()),
    };
    let role = match pending.mode {
        RouteMode::Fallback => "fallback",
        RouteMode::Reroute => "reroute target",
    };
    println!();
    println!(
        "  {} {}",
        style("⚠").yellow().bold(),
        style("Model routing requires an Edgee plan").bold()
    );
    println!(
        "      {} won't be used as your {role} until your org has an AI Gateway seat.",
        style(&pending.label).cyan()
    );
    println!("      Upgrade: {}", style(url).cyan().underlined());
    println!();
}

/// Wraps model identifiers into the API's route list, mapping empty to `None`
/// (→ JSON null, which clears the field server-side).
fn to_routes(models: &[String]) -> Option<Vec<ModelRoute>> {
    if models.is_empty() {
        None
    } else {
        Some(
            models
                .iter()
                .map(|model| ModelRoute {
                    model: model.clone(),
                })
                .collect(),
        )
    }
}

/// One-line description of the saved routing state, e.g. `fallback → kimi-...`.
fn routing_summary(settings: &KeySettings) -> String {
    let join = |v: &[ModelRoute]| {
        v.iter()
            .map(|m| m.model.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let fb = settings.fallbacks.as_deref().filter(|v| !v.is_empty());
    let rr = settings.reroutes.as_deref().filter(|v| !v.is_empty());
    match (fb, rr) {
        // Same model in both lists → show it once with a combined label.
        (Some(f), Some(_)) => style(format!("fallback & reroute → {}", join(f))).cyan().to_string(),
        (Some(f), None) => style(format!("fallback → {}", join(f))).cyan().to_string(),
        (None, Some(r)) => style(format!("reroute → {}", join(r))).cyan().to_string(),
        (None, None) => style("off").dim().to_string(),
    }
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
    println!("      routing                    {}", routing_summary(settings));
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(active: bool, display: &str, aliases: &[&str], providers: &[&str]) -> GatewayModel {
        mk(active, display, aliases, providers, false)
    }

    fn mk(
        active: bool,
        display: &str,
        aliases: &[&str],
        providers: &[&str],
        plan_fallback: bool,
    ) -> GatewayModel {
        GatewayModel {
            model_id: "m1".to_string(),
            author_id: String::new(),
            display_name: display.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            providers: providers
                .iter()
                .map(|p| (p.to_string(), Default::default()))
                .collect(),
            active,
            plan_fallback,
        }
    }

    fn no_byok() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn route_choices_skip_inactive_and_keep_all_active() {
        // All active models are offered (premium status is not a filter — the API
        // marks every catalog model premium); only inactive ones are dropped.
        let models = vec![
            model(true, "Kept A", &["keep-1"], &["anthropic"]),
            model(false, "Hidden", &["hidden-1"], &["anthropic"]),
            model(true, "Kept B", &["keep-2"], &["openai"]),
        ];
        let choices = route_choices(&models, &no_byok(), false);
        let mut ids: Vec<&str> = choices.iter().map(|c| c.identifier.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["keep-1", "keep-2"]);
    }

    #[test]
    fn route_choices_hide_cursor_and_copilot_only_models() {
        // Cursor/Copilot-exclusive models can't be routed to — they're reachable
        // only through those apps' own subscriptions.
        let models = vec![
            model(true, "Composer 2", &["composer-2"], &["cursor"]),
            model(true, "Raptor Mini", &["raptor-mini"], &["github_copilot"]),
            model(true, "Opus Fast", &["claude-opus-4-8-fast"], &["cursor", "github_copilot"]),
            // Also served by a real API provider → still routable, so kept.
            model(true, "GPT-5.4", &["gpt-5.4"], &["cursor", "github_copilot", "openai"]),
            model(true, "Sonnet 5", &["claude-sonnet-5"], &["anthropic", "cursor"]),
            // Alias-only entry with no providers at all stays offered.
            model(true, "Aliased", &["some-alias"], &[]),
        ];
        let choices = route_choices(&models, &no_byok(), false);
        let mut ids: Vec<&str> = choices.iter().map(|c| c.identifier.as_str()).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec!["claude-sonnet-5", "gpt-5.4", "some-alias"],
            "only app-subscription models should be hidden"
        );
    }

    #[test]
    fn byok_tag_ignores_app_providers() {
        // A Cursor key doesn't bill the route: the request goes to OpenAI.
        let byok: HashSet<String> = ["cursor".to_string()].into_iter().collect();
        let models = vec![model(true, "GPT", &["gpt-5.4"], &["cursor", "openai"])];
        let choices = route_choices(&models, &byok, false);
        assert!(!choices[0].byok);
        assert!(choices[0].menu_label().contains("· credits"));
    }

    #[test]
    fn route_choices_order_featured_and_turbo_first() {
        let byok: HashSet<String> = ["openai".to_string()].into_iter().collect();
        let models = vec![
            // Plan, non-turbo.
            mk(true, "Plan Std", &["plan-std"], &["anthropic"], true),
            // Plan, turbo → top.
            mk(true, "Plan Turbo", &["x-turbo"], &["anthropic"], true),
            // Non-plan, reachable via the openai BYOK key.
            mk(true, "OSS Byok", &["oss-byok"], &["openai"], false),
            // Non-plan, no key.
            mk(true, "OSS Plain", &["oss-plain"], &["anthropic"], false),
        ];
        let choices = route_choices(&models, &byok, false);
        let ids: Vec<&str> = choices.iter().map(|c| c.identifier.as_str()).collect();
        assert_eq!(ids, vec!["x-turbo", "plan-std", "oss-byok", "oss-plain"]);

        assert!(choices[0].featured && choices[0].turbo);
        assert!(choices[1].featured && !choices[1].turbo);
        assert!(!choices[2].featured);

        // BYOK reachability is tagged.
        let byok_choice = choices.iter().find(|c| c.identifier == "oss-byok").unwrap();
        assert!(byok_choice.byok);
        assert!(byok_choice.menu_label().contains("· your key"));
    }

    #[test]
    fn menu_label_tags_each_billing_category() {
        let byok: HashSet<String> = ["openai".to_string()].into_iter().collect();
        let models = vec![
            mk(true, "Plan", &["plan-1"], &["anthropic"], true),
            mk(true, "Byok", &["byok-1"], &["openai"], false),
            mk(true, "Credits", &["cred-1"], &["anthropic"], false),
            // A plan model also reachable via BYOK is still tagged as plan.
            mk(true, "PlanAndKey", &["pk-1"], &["openai"], true),
        ];
        let choices = route_choices(&models, &byok, false);
        let label = |id: &str| {
            choices
                .iter()
                .find(|c| c.identifier == id)
                .unwrap()
                .menu_label()
        };
        assert!(label("plan-1").contains("· plan"));
        assert!(label("byok-1").contains("· your key"));
        assert!(label("cred-1").contains("· credits"));
        // Plan coverage wins over BYOK.
        assert!(label("pk-1").contains("· plan"));
        let pk = choices.iter().find(|c| c.identifier == "pk-1").unwrap();
        assert!(!pk.byok);
    }

    #[test]
    fn byok_only_drops_models_with_no_matching_byok_key() {
        // The gateway rejects requests for models with no matching BYOK credentials
        // under byok_only, so the picker must not offer them at all.
        let byok: HashSet<String> = ["openai".to_string()].into_iter().collect();
        let models = vec![
            mk(true, "Reachable", &["oss-byok"], &["openai"], false),
            mk(true, "Unreachable", &["oss-plain"], &["anthropic"], false),
            // Plan coverage doesn't rescue a model byok_only would otherwise reject.
            mk(true, "PlanOnly", &["plan-only"], &["anthropic"], true),
        ];
        let choices = route_choices(&models, &byok, true);
        let ids: Vec<&str> = choices.iter().map(|c| c.identifier.as_str()).collect();
        assert_eq!(ids, vec!["oss-byok"]);
    }

    #[test]
    fn byok_only_with_no_provider_keys_drops_everything() {
        // No BYOK keys configured at all → nothing can match, so the picker ends
        // up empty rather than falling back to plan/credits models.
        let models = vec![
            mk(true, "PlanOnly", &["plan-only"], &["anthropic"], true),
            mk(true, "Plain", &["oss-plain"], &["openai"], false),
        ];
        let choices = route_choices(&models, &no_byok(), true);
        assert!(choices.is_empty());
    }

    #[test]
    fn byok_only_tags_every_surviving_choice_as_byok_not_plan() {
        // Once byok_only forces every request through the user's own key, plan
        // coverage is no longer the billing story — the menu should say so.
        let byok: HashSet<String> = ["openai".to_string()].into_iter().collect();
        let models = vec![mk(true, "PlanAndKey", &["pk-1"], &["openai"], true)];
        let choices = route_choices(&models, &byok, true);
        assert!(!choices[0].plan_covered);
        assert!(choices[0].byok);
        assert!(choices[0].menu_label().contains("· your key"));
    }

    #[test]
    fn byok_only_short_lists_every_surviving_choice() {
        // Nothing else is offered under byok_only, so surviving models should be
        // proposed directly instead of buried behind "+ More models…".
        let byok: HashSet<String> = ["openai".to_string()].into_iter().collect();
        let models = vec![
            mk(true, "One", &["oss-1"], &["openai"], false),
            mk(true, "Two", &["oss-2"], &["openai"], false),
        ];
        let choices = route_choices(&models, &byok, true);
        assert_eq!(choices.len(), 2);
        assert!(choices.iter().all(|c| c.featured));
    }

    #[test]
    fn byok_provider_set_keeps_active_and_folds_regions() {
        let keys = vec![
            ProviderKey {
                provider: "bedrock_us-east-1".to_string(),
                active: true,
            },
            ProviderKey {
                provider: "bedrock-mantle_us-east-1".to_string(),
                active: true,
            },
            ProviderKey {
                provider: "azure_westus".to_string(),
                active: true,
            },
            ProviderKey {
                provider: "openai".to_string(),
                active: false,
            },
        ];
        let set = byok_provider_set(&keys);
        assert!(set.contains("bedrock"));
        // bedrock-mantle is a distinct provider from bedrock, not a region of it.
        assert!(set.contains("bedrock-mantle"));
        assert!(!set.contains("bedrock-mantle_us-east-1"));
        assert!(set.contains("azure"));
        assert!(!set.contains("openai")); // inactive key dropped
    }

    #[test]
    fn is_turbo_matches_turbo_suffix() {
        assert!(is_turbo("kimi-k2.7-turbo"));
        assert!(is_turbo("GLM-5.1-TURBO"));
        assert!(!is_turbo("kimi-k2.7-code"));
    }

    #[test]
    fn routing_summary_reflects_mode() {
        let base = KeySettings {
            compression: Compression::default(),
            fallback: false,
            fallbacks: None,
            reroutes: None,
        };
        // Off when no routes.
        assert!(routing_summary(&base).contains("off"));

        let fb = KeySettings {
            fallback: true,
            fallbacks: Some(vec![ModelRoute {
                model: "kimi-turbo".to_string(),
            }]),
            ..clone_settings(&base)
        };
        assert!(routing_summary(&fb).contains("fallback → kimi-turbo"));

        let rr = KeySettings {
            reroutes: Some(vec![ModelRoute {
                model: "glm".to_string(),
            }]),
            ..clone_settings(&base)
        };
        assert!(routing_summary(&rr).contains("reroute → glm"));

        // A model in both lists collapses to a single combined label.
        let both = KeySettings {
            fallback: true,
            fallbacks: Some(vec![ModelRoute {
                model: "glm".to_string(),
            }]),
            reroutes: Some(vec![ModelRoute {
                model: "glm".to_string(),
            }]),
            ..clone_settings(&base)
        };
        assert!(routing_summary(&both).contains("fallback & reroute → glm"));
    }

    #[test]
    fn strategy_maps_to_route_mode() {
        assert_eq!(RouteStrategy::Passthrough.mode(), None);
        assert_eq!(RouteStrategy::Fallback.mode(), Some(RouteMode::Fallback));
        assert_eq!(RouteStrategy::Reroute.mode(), Some(RouteMode::Reroute));
    }

    fn clone_settings(s: &KeySettings) -> KeySettings {
        KeySettings {
            compression: s.compression,
            fallback: s.fallback,
            fallbacks: s.fallbacks.clone(),
            reroutes: s.reroutes.clone(),
        }
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
        // App providers lose to any real API provider, despite sorting first.
        assert_eq!(
            model(true, "X", &[], &["cursor", "openai"]).route_identifier(),
            Some("openai/m1".to_string())
        );
        // Neither → none.
        assert_eq!(model(true, "X", &[], &[]).route_identifier(), None);
    }
}
