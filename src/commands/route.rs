use anyhow::{Context, Result};
use clap::ValueEnum;
use console::style;
use serde::Serialize;

use crate::api::{ApiClient, KeySettings, ModelRoute};
use crate::commands::auth::login;

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Show current Edgee routing for an agent.
    Status {
        /// Coding agent whose key to inspect.
        #[arg(long, default_value = "claude")]
        agent: String,
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// List active models available for routing.
    Models {
        /// Coding agent whose catalog to inspect.
        #[arg(long, default_value = "claude")]
        agent: String,
        /// Optional case-insensitive search across model name, id, alias, and provider.
        #[arg(long)]
        query: Option<String>,
        /// Print machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Set Edgee routing for an agent.
    Set {
        /// Coding agent whose key to configure.
        #[arg(long, default_value = "claude")]
        agent: String,
        /// Routing strategy.
        #[arg(long, value_enum)]
        strategy: Strategy,
        /// Edgee model identifier, required unless strategy is passthrough.
        #[arg(long)]
        model: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Strategy {
    Passthrough,
    Fallback,
    Reroute,
}

#[derive(Debug, Serialize)]
struct RoutingStatus {
    agent: String,
    strategy: &'static str,
    model: Option<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    match opts.command {
        Command::Status { agent, json } => status(&agent, json).await,
        Command::Models {
            agent,
            query,
            json,
        } => models(&agent, query.as_deref(), json).await,
        Command::Set {
            agent,
            strategy,
            model,
        } => set(&agent, strategy, model).await,
    }
}

async fn load(agent: &str) -> Result<(crate::config::Credentials, crate::api::ApiKeyItem)> {
    login::ensure_org_selected().await?;
    let creds = crate::config::read()?;
    let key = login::fetch_provider_key(agent).await?;
    Ok((creds, key))
}

fn routing_status(agent: &str, key: &crate::api::ApiKeyItem) -> RoutingStatus {
    if let Some(route) = key.reroutes.first() {
        return RoutingStatus {
            agent: agent.to_string(),
            strategy: "reroute",
            model: Some(route.model.clone()),
        };
    }
    if let Some(route) = key.fallbacks.first() {
        return RoutingStatus {
            agent: agent.to_string(),
            strategy: "fallback",
            model: Some(route.model.clone()),
        };
    }
    RoutingStatus {
        agent: agent.to_string(),
        strategy: "passthrough",
        model: None,
    }
}

async fn status(agent: &str, json: bool) -> Result<()> {
    let (_, key) = load(agent).await?;
    let status = routing_status(agent, &key);
    if json {
        println!("{}", serde_json::to_string(&status)?);
    } else {
        println!("agent: {}", status.agent);
        println!("strategy: {}", status.strategy);
        println!("model: {}", status.model.as_deref().unwrap_or("passthrough"));
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct RoutingModel {
    name: String,
    catalog_id: Option<String>,
    display_name: String,
    aliases: Vec<String>,
}

async fn models(agent: &str, query: Option<&str>, json: bool) -> Result<()> {
    let (creds, _) = load(agent).await?;
    let token = creds
        .user_token
        .as_deref()
        .filter(|token| !token.is_empty())
        .context("Not authenticated. Run `edgee auth login` first.")?;
    let catalog = ApiClient::new(token)?.list_models().await?;
    let query = query.map(str::to_ascii_lowercase);
    let models: Vec<RoutingModel> = catalog
        .iter()
        .filter(|model| model.active && !model.app_subscription_only())
        .filter_map(|model| {
            let name = model.route_identifier()?;
            let haystack = format!(
                "{} {} {} {}",
                name,
                model.display_name,
                model.model_id,
                model.aliases.join(" ")
            )
            .to_ascii_lowercase();
            if query.as_ref().is_some_and(|query| !haystack.contains(query)) {
                return None;
            }
            Some(RoutingModel {
                name,
                catalog_id: model.catalog_id(),
                display_name: model.display_name.clone(),
                aliases: model.aliases.clone(),
            })
        })
        .collect();

    if json {
        println!("{}", serde_json::to_string(&models)?);
    } else {
        for model in &models {
            println!("{}{}", model.name, if model.display_name.is_empty() {
                String::new()
            } else {
                format!(" — {}", model.display_name)
            });
        }
    }
    Ok(())
}

async fn set(agent: &str, strategy: Strategy, model: Option<String>) -> Result<()> {
    let (creds, key) = load(agent).await?;
    let model = match strategy {
        Strategy::Passthrough => None,
        Strategy::Fallback | Strategy::Reroute => Some(
            model
                .filter(|model| !model.trim().is_empty())
                .ok_or_else(|| anyhow::anyhow!("--model is required for this strategy"))?,
        ),
    };

    if let Some(model) = &model {
        let token = creds
            .user_token
            .as_deref()
            .filter(|token| !token.is_empty())
            .context("Not authenticated. Run `edgee auth login` first.")?;
        let org_id = creds
            .org_id
            .as_deref()
            .filter(|org_id| !org_id.is_empty())
            .context("No organization selected. Run `edgee auth login` first.")?;
        let catalog = ApiClient::new(token)?.list_models().await?;
        if !catalog.iter().any(|entry| {
            entry.active
                && (entry.route_identifier().as_deref() == Some(model.as_str())
                    || entry.catalog_id().as_deref() == Some(model.as_str()))
        }) {
            anyhow::bail!("Unknown or inactive Edgee model `{model}`");
        }
        let _ = org_id;
    }

    let settings = KeySettings {
        compression: key.compression.unwrap_or_default(),
        fallback: matches!(strategy, Strategy::Fallback),
        fallbacks: model.clone().filter(|_| matches!(strategy, Strategy::Fallback)).map(|model| {
            vec![ModelRoute { model }]
        }),
        reroutes: model.filter(|_| matches!(strategy, Strategy::Reroute)).map(|model| {
            vec![ModelRoute { model }]
        }),
    };
    let token = creds.user_token.as_deref().unwrap_or_default();
    let org_id = creds.org_id.as_deref().unwrap_or_default();
    ApiClient::new(token)?
        .update_key_settings(org_id, &key.id, &settings)
        .await
        .context("Failed to update Edgee routing")?;

    println!(
        "  {} {} routing: {}{}",
        style("✓").green(),
        agent,
        match strategy {
            Strategy::Passthrough => "passthrough".to_string(),
            Strategy::Fallback => "fallback".to_string(),
            Strategy::Reroute => "reroute".to_string(),
        },
        settings
            .fallbacks
            .as_ref()
            .or(settings.reroutes.as_ref())
            .and_then(|routes| routes.first())
            .map(|route| format!(" → {}", route.model))
            .unwrap_or_default()
    );
    Ok(())
}
