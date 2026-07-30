use std::collections::HashMap;

/// Context windows, in tokens, keyed by the model id used in agent configs
/// (`<author>/<model>`, e.g. `anthropic/claude-opus-5`).
pub type ContextLimits = HashMap<String, u64>;

/// Fetches the console catalog so each model's context window can be declared in
/// a generated agent config.
///
/// The gateway's `/v1/models` listing — the source of the model ids themselves —
/// carries no limits, and agents default a config-defined model's context window
/// to `0`, which reads as "unknown" and disables the features that depend on it
/// (context gauges, auto-compaction). The console catalog is the only source that
/// has the real windows, and its `<author_id>/<model_id>` keys match the gateway
/// listing's ids exactly.
///
/// Best-effort: any failure yields an empty map, so launch falls back to models
/// with no declared window rather than failing.
pub async fn fetch_model_context_limits(creds: &crate::config::Credentials) -> ContextLimits {
    let Some(token) = creds.user_token.as_deref().filter(|t| !t.is_empty()) else {
        return ContextLimits::new();
    };
    let Ok(client) = crate::api::ApiClient::new(token) else {
        return ContextLimits::new();
    };
    let Ok(models) = client.list_models().await else {
        return ContextLimits::new();
    };
    models
        .iter()
        .filter_map(|m| Some((m.catalog_id()?, m.context_limit()?)))
        .collect()
}
