use anyhow::Result;
use serde_json::Value;

use super::util;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the crush CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(std::path::PathBuf::from)
}

/// Resolves the directory Crush reads its global `crush.json` from. Honors an
/// existing `CRUSH_GLOBAL_CONFIG` so we read whatever config the user already
/// has before we override the variable for the launched process. Falls back to
/// the platform default (`$XDG_CONFIG_HOME/crush` or `~/.config/crush`, and
/// `%LOCALAPPDATA%\crush` on Windows).
fn global_config_dir() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("CRUSH_GLOBAL_CONFIG") {
        if !dir.is_empty() {
            return Some(std::path::PathBuf::from(dir));
        }
    }
    #[cfg(windows)]
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        if !local.is_empty() {
            return Some(std::path::PathBuf::from(local).join("crush"));
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(std::path::PathBuf::from(xdg).join("crush"));
        }
    }
    home_dir().map(|h| h.join(".config").join("crush"))
}

/// Reads the user's existing global `crush.json` so launch preserves whatever
/// they already configured (LSPs, MCPs, options) and only layers the Edgee
/// provider on top. Project-level `.crush.json`/`crush.json` are loaded by
/// Crush itself and still take precedence, so we deliberately don't touch them.
fn find_global_config() -> Option<Value> {
    let path = global_config_dir()?.join("crush.json");
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    let parsed: Value = serde_json::from_str(&content).ok()?;
    parsed.is_object().then_some(parsed)
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

/// Fetches the gateway's OpenAI-style `/v1/models` listing so the Crush
/// provider config can be populated with a concrete `models` list.
///
/// The endpoint serves anonymous callers the whole catalog, but narrows the
/// listing for a *resolved* key: with BYOK-only enforced (org, squad or key
/// scope), it returns only models the key has provider credentials for. It
/// resolves the key from `x-api-key`/`Authorization: Bearer` only — Edgee's own
/// `x-edgee-api-key` header is not read there, so sending that alone would
/// silently produce the unfiltered catalog and offer models every request would
/// be rejected for. Returns an empty vec on any failure so launch falls back to
/// a provider that relies on Crush's own `/v1/models` discovery.
async fn fetch_gateway_models(gateway_url: &str, api_key: &str) -> Vec<String> {
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

fn build_edgee_provider(
    api_key: &str,
    session_id: &str,
    gateway_url: &str,
    models: &[String],
    catalog: &util::ModelCatalog,
    debug_log_headers: Option<crate::crypto::DebugLogHeaderValues>,
) -> Value {
    // The provider is an OpenAI-compatible endpoint pointed at the gateway's
    // `/v1`. The gateway `id` (e.g. `anthropic/claude-opus-4-8`) is already the
    // routing identifier the gateway accepts, so it serves as both the model id
    // and the display name. When the listing is empty we leave `models` off and
    // set `discover_models` so Crush populates the picker from `/v1/models`.
    let mut extra_headers = serde_json::json!({
        "x-edgee-api-key": api_key,
        "x-edgee-session-id": session_id,
    });
    if let (Some(headers_obj), Some(debug_headers)) =
        (extra_headers.as_object_mut(), debug_log_headers)
    {
        headers_obj.insert("x-edgee-debug-pubkey".to_string(), Value::String(debug_headers.pubkey));
        headers_obj.insert("x-edgee-debug-salt".to_string(), Value::String(debug_headers.salt));
    }

    let mut provider = serde_json::json!({
        "id": "edgee",
        "name": "Edgee",
        "type": "openai-compat",
        "base_url": format!("{}/v1", gateway_url),
        "api_key": api_key,
        "extra_headers": extra_headers,
        "discover_models": true,
    });

    if !models.is_empty() {
        let models_arr: Vec<Value> = models
            .iter()
            .map(|id| {
                let mut model = serde_json::json!({ "id": id, "name": id });
                // Crush treats a `context_window` of 0 as "unknown": it hides the
                // header's context gauge and explicitly skips auto-summarizing to
                // avoid truncating custom models. Declaring the real window is what
                // turns both back on. Omitted when the catalog has no window rather
                // than guessing one, which keeps Crush's unknown-model handling.
                //
                // `default_max_tokens` is deliberately left off: Crush omits
                // `max_tokens` from the request when it is 0, letting the upstream
                // apply its own cap. The catalog carries no output-token cap, so any
                // value we invented would either truncate replies or be rejected.
                let metadata = catalog.get(id);
                if let Some(context_window) = metadata.and_then(|m| m.context) {
                    model["context_window"] = serde_json::json!(context_window);
                }
                // Rates are dollars per million tokens; Crush divides by 1e6 when
                // costing a session. Without them every session reports as free.
                //
                // The two cache fields mean the opposite of what they sound like:
                // Crush multiplies `in_cached` by cache-*creation* tokens and
                // `out_cached` by cache-*read* tokens (`agent.go:1902-1905`), and
                // catwalk's own Anthropic data follows suit (Sonnet 4.5 ships
                // `in_cached: 3.75`, the write rate, and `out_cached: 0.3`, the read
                // rate). So write maps to `in_cached` and read to `out_cached`.
                if let Some(cost) = metadata.and_then(|m| m.cost) {
                    model["cost_per_1m_in"] = serde_json::json!(cost.input);
                    model["cost_per_1m_out"] = serde_json::json!(cost.output);
                    model["cost_per_1m_in_cached"] = serde_json::json!(cost.cache_write);
                    model["cost_per_1m_out_cached"] = serde_json::json!(cost.cache_read);
                }
                model
            })
            .collect();
        provider["models"] = Value::Array(models_arr);
    }

    provider
}

/// Inserts `provider` under `providers.edgee`, creating the `providers` object
/// when the config doesn't have one yet.
fn insert_edgee_provider(config: &mut Value, provider: Value) {
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    match obj.get_mut("providers").and_then(Value::as_object_mut) {
        Some(providers) => {
            providers.insert("edgee".to_string(), provider);
        }
        None => {
            let mut providers = serde_json::Map::new();
            providers.insert("edgee".to_string(), provider);
            obj.insert("providers".to_string(), Value::Object(providers));
        }
    }
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Step 1: ensure we are authenticated
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }

    // Step 1b: ensure an org is selected (handles partial state after aborted login)
    crate::commands::auth::login::ensure_org_selected().await?;

    // Step 2: ensure we have a live api_key for Crush. Re-provisions if the
    // cached key was deleted in the console; re-runs onboarding for a fresh key.
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("crush")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("crush").await?;
    }
    creds = crate::config::read()?;

    // Step 3: ensure we have a connection choice (default to "plan")
    if creds
        .crush
        .as_ref()
        .and_then(|c| c.connection.as_deref())
        .is_none()
    {
        let provider = creds.crush.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    // Step 4: build merged config from the user's existing global crush.json +
    // the Edgee provider.
    let crush = creds.crush.as_ref().unwrap();
    let api_key = &crush.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();
    util::spawn_cli_version_report(&creds, &session_id);

    // First-run: install the persistent user-level statusline integration
    // exactly once (Claude Code-targeted; honors the disable marker).
    util::ensure_first_run_installed().await;

    let gateway_url = super::resolve_gateway_base_url(&creds).await;

    let mut config = find_global_config().unwrap_or_else(|| {
        serde_json::json!({
            "$schema": "https://charm.land/crush.json",
        })
    });

    let (models, catalog) = tokio::join!(
        fetch_gateway_models(&gateway_url, api_key),
        util::fetch_model_catalog(&creds)
    );
    let models = util::without_app_subscription_models(models, &catalog);
    let debug_log_headers = util::resolve_debug_log_keypair()?.map(|k| k.header_values());
    let edgee_provider = build_edgee_provider(
        api_key,
        &session_id,
        &gateway_url,
        &models,
        &catalog,
        debug_log_headers,
    );
    insert_edgee_provider(&mut config, edgee_provider);

    // Crush reads `crush.json` from the directory named by CRUSH_GLOBAL_CONFIG,
    // so we write into a per-session temp directory and point the variable at it.
    let config_dir = std::env::temp_dir().join(format!("edgee-crush-config-{}", session_id));
    std::fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join("crush.json");
    let config_content = serde_json::to_string_pretty(&config)?;
    std::fs::write(&config_path, &config_content)?;

    // Step 5: launch crush with the correct env vars
    let mut cmd = std::process::Command::new(util::resolve_binary("crush"));
    cmd.env("CRUSH_GLOBAL_CONFIG", &config_dir);
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.args(&opts.args);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Crush is not installed. Install it from https://github.com/charmbracelet/crush"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    // Clean up the temporary config directory
    let _ = std::fs::remove_dir_all(&config_dir);

    super::print_session_stats(&creds, &session_id, "Crush").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models_of(models: &[&str], limits: &[(&str, u64)]) -> Vec<Value> {
        let models: Vec<String> = models.iter().map(|m| m.to_string()).collect();
        let catalog: util::ModelCatalog = limits
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    util::ModelMetadata {
                        context: Some(*v),
                        cost: None,
                            app_subscription_only: false,
                    },
                )
            })
            .collect();
        let provider =
            build_edgee_provider("key", "sess", "https://gw.test", &models, &catalog, None);
        provider["models"].as_array().cloned().unwrap_or_default()
    }

    fn priced_model(id: &str, cost: crate::api::GatewayModelCost) -> Value {
        let catalog: util::ModelCatalog = [(
            id.to_string(),
            util::ModelMetadata {
                context: None,
                cost: Some(cost),
                    app_subscription_only: false,
            },
        )]
        .into_iter()
        .collect();
        let provider = build_edgee_provider(
            "key",
            "sess",
            "https://gw.test",
            &[id.to_string()],
            &catalog,
            None,
        );
        provider["models"][0].clone()
    }

    #[test]
    fn declares_the_context_window_from_the_catalog() {
        let models = models_of(
            &["anthropic/claude-opus-5"],
            &[("anthropic/claude-opus-5", 1_000_000)],
        );
        assert_eq!(models[0]["id"], serde_json::json!("anthropic/claude-opus-5"));
        assert_eq!(models[0]["context_window"], serde_json::json!(1_000_000));
    }

    #[test]
    fn omits_the_context_window_when_the_catalog_has_none() {
        let models = models_of(&["openai/gpt-5"], &[]);
        assert_eq!(models[0]["name"], serde_json::json!("openai/gpt-5"));
        assert!(models[0].get("context_window").is_none());
    }

    /// Crush drops `max_tokens` from the request when it is 0, deferring to the
    /// upstream cap. We have no output-cap data, so the field must stay absent.
    #[test]
    fn never_declares_a_max_token_cap() {
        let models = models_of(
            &["anthropic/claude-opus-5"],
            &[("anthropic/claude-opus-5", 1_000_000)],
        );
        assert!(models[0].get("default_max_tokens").is_none());
    }

    #[test]
    fn windows_are_matched_per_model_not_applied_wholesale() {
        let models = models_of(
            &["anthropic/claude-haiku-4-5", "openai/gpt-5"],
            &[("anthropic/claude-haiku-4-5", 200_000)],
        );
        assert_eq!(models[0]["context_window"], serde_json::json!(200_000));
        assert!(models[1].get("context_window").is_none());
    }

    /// Crush's cache rate fields are named the opposite of their meaning: it costs
    /// cache-creation tokens at `in_cached` and cache-read tokens at `out_cached`.
    /// These are Sonnet 4.5's real rates, matching catwalk's own catalog entry
    /// (`cost_per_1m_in_cached: 3.75`, `cost_per_1m_out_cached: 0.3`).
    #[test]
    fn maps_cache_rates_to_crushs_inverted_field_names() {
        let model = priced_model(
            "anthropic/claude-sonnet-4-5",
            crate::api::GatewayModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
            },
        );
        assert_eq!(model["cost_per_1m_in"], serde_json::json!(3.0));
        assert_eq!(model["cost_per_1m_out"], serde_json::json!(15.0));
        // Write rate under the `in_cached` name, read rate under `out_cached`.
        assert_eq!(model["cost_per_1m_in_cached"], serde_json::json!(3.75));
        assert_eq!(model["cost_per_1m_out_cached"], serde_json::json!(0.3));
    }

    #[test]
    fn omits_rates_when_the_catalog_has_none() {
        let models = models_of(&["openai/gpt-5"], &[("openai/gpt-5", 400_000)]);
        for field in [
            "cost_per_1m_in",
            "cost_per_1m_out",
            "cost_per_1m_in_cached",
            "cost_per_1m_out_cached",
        ] {
            assert!(models[0].get(field).is_none(), "{field} should be absent");
        }
    }

    #[test]
    fn a_free_model_declares_zeroed_rates_rather_than_omitting_them() {
        let model = priced_model("zai/glm-4.5-flash", crate::api::GatewayModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        });
        assert_eq!(model["cost_per_1m_in"], serde_json::json!(0.0));
        assert_eq!(model["cost_per_1m_out"], serde_json::json!(0.0));
    }

    /// Crush's model schema sets `additionalProperties: false`, so every field we
    /// emit has to be one it declares.
    #[test]
    fn emits_only_fields_crushs_schema_declares() {
        let model = priced_model(
            "anthropic/claude-opus-5",
            crate::api::GatewayModelCost {
                input: 5.0,
                output: 25.0,
                cache_read: 0.5,
                cache_write: 6.25,
            },
        );
        let allowed = [
            "id",
            "name",
            "cost_per_1m_in",
            "cost_per_1m_out",
            "cost_per_1m_in_cached",
            "cost_per_1m_out_cached",
            "context_window",
            "default_max_tokens",
            "can_reason",
            "reasoning_levels",
            "default_reasoning_effort",
            "supports_attachments",
            "options",
        ];
        for key in model.as_object().unwrap().keys() {
            assert!(allowed.contains(&key.as_str()), "unexpected field {key}");
        }
    }
}
