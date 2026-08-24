use std::collections::HashMap;

use crate::api::GatewayModelCost;

/// What the console catalog knows about one model, beyond its id.
#[derive(Debug, Clone, Default)]
pub struct ModelMetadata {
    /// Context window in tokens, when a provider declares one.
    pub context: Option<u64>,
    /// Per-million-token rates, in US dollars.
    pub cost: Option<GatewayModelCost>,
    /// Gateway-normalized reasoning effort values accepted by the model.
    pub reasoning_efforts: Vec<String>,
    /// Served only through a coding-app subscription (Cursor, GitHub Copilot) —
    /// see [`without_app_subscription_models`].
    pub app_subscription_only: bool,
}

/// Model metadata keyed by the model id used in agent configs
/// (`<author>/<model>`, e.g. `anthropic/claude-opus-5`).
pub type ModelCatalog = HashMap<String, ModelMetadata>;

/// Fetches the console catalog so generated agent configs can declare each
/// model's context window and pricing.
///
/// The gateway's `/v1/models` listing — the source of the model ids themselves —
/// carries neither. Agents default both to zero for a config-defined model, which
/// disables the features that depend on them (context gauges, auto-compaction)
/// and reports every session as costing nothing. The console catalog is the only
/// source that has them, and its `<author_id>/<model_id>` keys match the gateway
/// listing's ids exactly.
///
/// Best-effort: any failure yields an empty map, so launch falls back to models
/// with no declared metadata rather than failing.
pub async fn fetch_model_catalog(creds: &crate::config::Credentials) -> ModelCatalog {
    let Some(token) = creds.user_token.as_deref().filter(|t| !t.is_empty()) else {
        return ModelCatalog::new();
    };
    let Ok(client) = crate::api::ApiClient::new(token) else {
        return ModelCatalog::new();
    };
    let Ok(models) = client.list_models().await else {
        return ModelCatalog::new();
    };
    models
        .iter()
        .filter_map(|m| {
            Some((
                m.catalog_id()?,
                ModelMetadata {
                    context: m.context_limit(),
                    cost: m.cost(),
                    reasoning_efforts: m.reasoning_efforts.clone(),
                    app_subscription_only: m.app_subscription_only(),
                },
            ))
        })
        .collect()
}

#[derive(serde::Deserialize)]
struct GatewayModelList {
    #[serde(default)]
    data: Vec<GatewayModelEntry>,
}

#[derive(serde::Deserialize)]
struct GatewayModelEntry {
    id: String,
}

/// Fetches the gateway's OpenAI-style `/v1/models` listing so a generated agent
/// config can be populated with a concrete model list.
///
/// The endpoint serves anonymous callers the whole catalog, but narrows the
/// listing for a *resolved* key: with BYOK-only enforced (org, squad or key
/// scope), it returns only models the key has provider credentials for. It
/// resolves the key from `x-api-key`/`Authorization: Bearer` only — Edgee's own
/// `x-edgee-api-key` header is not read there, so sending that alone would
/// silently produce the unfiltered catalog and offer models every request would
/// be rejected for. Returns an empty vec on any failure so launch falls back to
/// a provider with no explicit model list.
pub async fn fetch_gateway_models(gateway_url: &str, api_key: &str) -> Vec<String> {
    let url = format!("{}/v1/models", gateway_url);
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let resp = match client.get(&url).header("x-api-key", api_key).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return Vec::new(),
    };
    match resp.json::<GatewayModelList>().await {
        Ok(list) => list.data.into_iter().map(|m| m.id).collect(),
        Err(_) => Vec::new(),
    }
}

/// Drops models reachable only through a coding-app subscription (Cursor, GitHub
/// Copilot) from a gateway `/v1/models` listing.
///
/// The gateway serves the whole catalog, including entries like `cursor/composer-2`
/// whose only upstreams are those apps' own subscriptions. A CLI agent talks to
/// the gateway with an Edgee key, so such a model is unreachable and listing it
/// only puts a model in the picker that fails at request time.
///
/// Models the catalog doesn't know are kept: an unknown id means we can't judge
/// reachability, and dropping it would hide a usable model. An empty catalog
/// (fetch failed) therefore leaves the listing untouched.
pub fn without_app_subscription_models(models: Vec<String>, catalog: &ModelCatalog) -> Vec<String> {
    models
        .into_iter()
        .filter(|id| !catalog.get(id).is_some_and(|m| m.app_subscription_only))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog(entries: &[(&str, bool)]) -> ModelCatalog {
        entries
            .iter()
            .map(|(id, app_only)| {
                (
                    id.to_string(),
                    ModelMetadata {
                        app_subscription_only: *app_only,
                        ..Default::default()
                    },
                )
            })
            .collect()
    }

    fn ids(models: &[&str]) -> Vec<String> {
        models.iter().map(|m| m.to_string()).collect()
    }

    #[test]
    fn drops_app_subscription_only_models() {
        let catalog = catalog(&[
            ("cursor/composer-2", true),
            ("github/raptor-mini", true),
            // Also served by a real API provider → reachable, so kept.
            ("openai/gpt-5", false),
            ("anthropic/claude-sonnet-5", false),
        ]);
        let kept = without_app_subscription_models(
            ids(&[
                "cursor/composer-2",
                "openai/gpt-5",
                "github/raptor-mini",
                "anthropic/claude-sonnet-5",
            ]),
            &catalog,
        );
        assert_eq!(kept, ids(&["openai/gpt-5", "anthropic/claude-sonnet-5"]));
    }

    #[test]
    fn keeps_models_the_catalog_does_not_know() {
        // An unknown id can't be judged; hiding it would drop a usable model.
        let kept = without_app_subscription_models(
            ids(&["openai/gpt-5", "cursor/composer-2"]),
            &catalog(&[("openai/gpt-5", false)]),
        );
        assert_eq!(kept, ids(&["openai/gpt-5", "cursor/composer-2"]));
    }

    #[test]
    fn an_empty_catalog_leaves_the_listing_untouched() {
        // Catalog fetch failed → filter nothing rather than emptying the picker.
        let listing = ids(&["cursor/composer-2", "openai/gpt-5"]);
        let kept = without_app_subscription_models(listing.clone(), &ModelCatalog::new());
        assert_eq!(kept, listing);
    }
}
