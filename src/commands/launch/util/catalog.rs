use std::collections::HashMap;

use crate::api::GatewayModelCost;

/// What the console catalog knows about one model, beyond its id.
#[derive(Debug, Clone, Default)]
pub struct ModelMetadata {
    /// Context window in tokens, when a provider declares one.
    pub context: Option<u64>,
    /// Per-million-token rates, in US dollars.
    pub cost: Option<GatewayModelCost>,
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
                },
            ))
        })
        .collect()
}
