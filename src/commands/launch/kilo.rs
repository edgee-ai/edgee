//! `edgee launch kilo` — Kilo Code CLI (`@kilocode/cli`, binary `kilo`).
//!
//! ## Why `KILO_CONFIG_CONTENT` (Transport A) and not the row above it
//!
//! Kilo's CLI is an OpenCode fork — its own startup log still prints `opencode`,
//! and its config schema (`https://app.kilo.ai/config.json`) matches OpenCode's
//! `ProviderConfig` field for field. So the natural move is to copy
//! [`opencode.rs`](opencode.rs): build a merged config in `$TMPDIR` and point
//! the agent at it with `KILO_CONFIG` (the exact analogue of `OPENCODE_CONFIG`).
//!
//! Kilo exposes something strictly better. `KILO_CONFIG_CONTENT` takes the
//! config as an **inline JSON string**, and sits near the top of Kilo's
//! precedence chain:
//!
//! ```text
//! remote well-known → ~/.config/kilo/kilo.json → KILO_CONFIG → ./kilo.json
//!   → .kilo/kilo.json → KILO_CONFIG_CONTENT → managed     (deep-merged, later wins)
//! ```
//!
//! Two consequences, both of which OpenCode's temp-file path cannot offer:
//!
//! - **The API key never touches disk.** `opencode.rs` and `crush.rs` write a
//!   config containing the key into `$TMPDIR` and delete it afterwards; a crash
//!   between those two points leaves it there. Here the key lives only in the
//!   child process's environment.
//! - **Kilo does the merge, so we never read the user's files.** `opencode.rs`
//!   has to locate and parse the user's `opencode.json`/`.jsonc` itself (hence
//!   its JSONC stripper) to avoid clobbering it. `KILO_CONFIG_CONTENT` is
//!   deep-merged *over* whatever the user already has, so this module ships one
//!   `provider.edgee` key and nothing else. Sitting above the project layer also
//!   means a repo-local `kilo.json` cannot shadow the Edgee provider.
//!
//! `KILO_CONFIG_DIR` is **not** the lever it appears to be. The docs embedded in
//! the binary describe it as "appended to the search list", but the binary
//! resolves `config: KILO_CONFIG_DIR ?? Hc.config` — it *replaces* the global
//! config root, hiding the user's own commands, agents and skills. The binary
//! wins over the doc string; do not switch to it.
//!
//! ## Credentials
//!
//! This target runs **entirely on Edgee-supplied credentials** — like OpenCode
//! and Crush, and unlike Claude Code or Codex, it does not redirect an agent the
//! user already authenticated. Kilo's own `kilo auth` login is untouched and
//! unused on this path; the Edgee provider authenticates with the Edgee key
//! alone.
//!
//! ## Wire shape
//!
//! OpenAI Chat Completions (`POST /v1/chat/completions`), so `baseURL` **keeps**
//! the `/v1` — unlike the Anthropic-shaped targets, where the SDK appends it.
//! Tool names arrive lowercase (`bash`, `read`, `grep`, `glob`, …), identical to
//! OpenCode's, which is why the gateway needs no new trimming strategy and the
//! `kilo` key is provisioned with the OpenCode compression flavor.

use anyhow::Result;
use serde_json::Value;

use super::util;

/// Kilo clamps every request's `max_tokens` with
/// `maxOutputTokens(model, max) = Math.min(model.limit.output, max) || max`,
/// where `max` defaults to its `OUTPUT_TOKEN_MAX` of 32k — the same constant,
/// and the same clamp, as OpenCode. Its config schema requires `output`
/// whenever `limit` is present, so declaring 32k satisfies the schema while
/// leaving the request identical to what Kilo would send on its own.
const KILO_OUTPUT_TOKEN_MAX: u64 = 32_000;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the kilo CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// The single `provider.edgee` object that is deep-merged into the user's Kilo
/// config. Nothing else is emitted: everything the user already configured is
/// preserved by Kilo's own merge.
fn build_edgee_provider(
    api_key: &str,
    session_id: &str,
    gateway_url: &str,
    models: &[String],
    catalog: &util::ModelCatalog,
    debug_log_headers: Option<crate::crypto::DebugLogHeaderValues>,
) -> Value {
    // The provider is `@ai-sdk/openai-compatible` pointed at the gateway's
    // `/v1`. We fill the `models` map from the gateway's `/v1/models` listing so
    // Kilo's picker is populated with the live catalog. The gateway `id`
    // (e.g. `anthropic/claude-opus-4-8`) is already the routing identifier the
    // gateway accepts, so it serves as both the map key and the display name.
    let mut headers = serde_json::json!({
        "x-edgee-api-key": api_key,
        "x-edgee-session-id": session_id,
    });
    if let (Some(headers_obj), Some(debug_headers)) = (headers.as_object_mut(), debug_log_headers) {
        headers_obj.insert(
            "x-edgee-debug-pubkey".to_string(),
            Value::String(debug_headers.pubkey),
        );
        headers_obj.insert(
            "x-edgee-debug-salt".to_string(),
            Value::String(debug_headers.salt),
        );
    }

    let mut provider = serde_json::json!({
        "npm": "@ai-sdk/openai-compatible",
        "name": "Edgee",
        "options": {
            "baseURL": format!("{}/v1", gateway_url),
            "apiKey": api_key,
            "headers": headers,
        }
    });

    if !models.is_empty() {
        let mut models_map = serde_json::Map::new();
        for id in models {
            let mut entry = serde_json::json!({ "name": id });
            let metadata = catalog.get(id);
            if let Some(input) = metadata
                .map(|m| m.input_modalities.as_slice())
                .filter(|input| !input.is_empty())
            {
                entry["modalities"] = serde_json::json!({ "input": input });
            }
            // Only declare `limit` when the catalog gave us a real context window;
            // a fabricated one is worse than letting Kilo fall back to 0.
            if let Some(context) = metadata.and_then(|m| m.context) {
                entry["limit"] = serde_json::json!({
                    "context": context,
                    "output": KILO_OUTPUT_TOKEN_MAX,
                });
            }
            // Rates are dollars per million tokens, the same unit Kilo's own
            // catalog uses. Without them every session reports as costing $0.
            //
            // Tiered pricing is deliberately not emitted: Kilo's schema has a
            // `cost.context_over_200k` slot, inherited from OpenCode along with
            // the config merge that drops the field, so declaring it would imply
            // a tier that never takes effect.
            if let Some(cost) = metadata.and_then(|m| m.cost) {
                entry["cost"] = serde_json::json!({
                    "input": cost.input,
                    "output": cost.output,
                    "cache_read": cost.cache_read,
                    "cache_write": cost.cache_write,
                });
            }
            if let Some(efforts) = metadata
                .map(|m| m.reasoning_efforts.as_slice())
                .filter(|efforts| !efforts.is_empty())
            {
                entry["reasoning"] = Value::Bool(true);
                entry["variants"] = Value::Object(
                    efforts
                        .iter()
                        .map(|effort| {
                            (
                                effort.clone(),
                                serde_json::json!({ "reasoningEffort": effort }),
                            )
                        })
                        .collect(),
                );
            }
            models_map.insert(id.clone(), entry);
        }
        provider["models"] = Value::Object(models_map);
    }

    provider
}

/// The full `KILO_CONFIG_CONTENT` payload. Kilo deep-merges this over the
/// user's own config, so it carries the Edgee provider and nothing else.
fn build_config_content(edgee_provider: Value) -> Value {
    serde_json::json!({
        "$schema": "https://app.kilo.ai/config.json",
        "provider": { "edgee": edgee_provider },
    })
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Step 1: ensure we are authenticated
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }

    // Step 1b: ensure an org is selected (handles partial state after aborted login)
    crate::commands::auth::login::ensure_org_selected().await?;

    // Step 2: ensure we have a live api_key for Kilo. Re-provisions if the
    // cached key was deleted in the console; re-runs onboarding for a fresh key.
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("kilo")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("kilo").await?;
    }
    creds = crate::config::read()?;

    // Step 3: ensure we have a connection choice (default to "plan")
    if creds
        .kilo
        .as_ref()
        .and_then(|c| c.connection.as_deref())
        .is_none()
    {
        let provider = creds.kilo.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    // Step 4: build the inline config carrying the Edgee provider
    let kilo = creds.kilo.as_ref().unwrap();
    let api_key = &kilo.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();
    util::spawn_cli_version_report(&creds, &session_id);

    // First-run: install the persistent user-level statusline integration
    // exactly once (Claude Code-targeted; honors the disable marker).
    util::ensure_first_run_installed().await;

    let gateway_url = super::resolve_gateway_base_url(&creds).await;

    let (models, catalog) = tokio::join!(
        util::fetch_gateway_models(&gateway_url, api_key),
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
    let config_content = serde_json::to_string(&build_config_content(edgee_provider))?;

    // Step 5: launch kilo with the correct env vars
    let mut cmd = std::process::Command::new(util::resolve_binary("kilo"));
    cmd.env("KILO_CONFIG_CONTENT", &config_content);
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env("EDGEE_ORG_SLUG", creds.org_slug.as_deref().unwrap_or_default());
    cmd.args(&opts.args);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Kilo Code is not installed. Install it with `npm install -g @kilocode/cli`"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    super::print_session_stats(&creds, &session_id, "Kilo Code").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_with(models: &[&str], limits: &[(&str, u64)]) -> Value {
        let models: Vec<String> = models.iter().map(|m| m.to_string()).collect();
        let catalog: util::ModelCatalog = limits
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    util::ModelMetadata {
                        context: Some(*v),
                        ..Default::default()
                    },
                )
            })
            .collect();
        build_edgee_provider("key", "sess", "https://gw.test", &models, &catalog, None)
    }

    fn provider_priced(id: &str, cost: crate::api::GatewayModelCost) -> Value {
        let catalog: util::ModelCatalog = [(
            id.to_string(),
            util::ModelMetadata {
                cost: Some(cost),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect();
        build_edgee_provider(
            "key",
            "sess",
            "https://gw.test",
            &[id.to_string()],
            &catalog,
            None,
        )
    }

    fn provider_with_reasoning(id: &str, efforts: &[&str]) -> Value {
        let catalog: util::ModelCatalog = [(
            id.to_string(),
            util::ModelMetadata {
                reasoning_efforts: efforts.iter().map(|effort| effort.to_string()).collect(),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect();
        build_edgee_provider(
            "key",
            "sess",
            "https://gw.test",
            &[id.to_string()],
            &catalog,
            None,
        )
    }

    #[test]
    fn declares_input_modalities_from_the_catalog() {
        let catalog: util::ModelCatalog = [(
            "anthropic/claude-opus-5".to_string(),
            util::ModelMetadata {
                input_modalities: vec!["text".to_string(), "image".to_string()],
                ..Default::default()
            },
        )]
        .into_iter()
        .collect();
        let provider = build_edgee_provider(
            "key",
            "sess",
            "https://gw.test",
            &["anthropic/claude-opus-5".to_string()],
            &catalog,
            None,
        );

        assert_eq!(
            provider["models"]["anthropic/claude-opus-5"]["modalities"]["input"],
            serde_json::json!(["text", "image"])
        );
    }

    /// Kilo speaks OpenAI Chat Completions and does not append the version
    /// segment itself, so the `/v1` has to be in `baseURL`.
    #[test]
    fn base_url_keeps_the_v1_segment() {
        let provider = provider_with(&["openai/gpt-5"], &[]);
        assert_eq!(
            provider["options"]["baseURL"],
            serde_json::json!("https://gw.test/v1")
        );
    }

    #[test]
    fn carries_the_edgee_headers() {
        let provider = provider_with(&["openai/gpt-5"], &[]);
        let headers = &provider["options"]["headers"];
        assert_eq!(headers["x-edgee-api-key"], serde_json::json!("key"));
        assert_eq!(headers["x-edgee-session-id"], serde_json::json!("sess"));
        // Absent unless a debug-log keypair was resolved.
        assert!(headers.get("x-edgee-debug-pubkey").is_none());
        assert!(headers.get("x-edgee-debug-salt").is_none());
    }

    #[test]
    fn debug_log_headers_are_added_when_a_keypair_exists() {
        let catalog = util::ModelCatalog::new();
        let provider = build_edgee_provider(
            "key",
            "sess",
            "https://gw.test",
            &[],
            &catalog,
            Some(crate::crypto::DebugLogHeaderValues {
                pubkey: "PUB".to_string(),
                salt: "SALT".to_string(),
            }),
        );
        let headers = &provider["options"]["headers"];
        assert_eq!(headers["x-edgee-debug-pubkey"], serde_json::json!("PUB"));
        assert_eq!(headers["x-edgee-debug-salt"], serde_json::json!("SALT"));
    }

    #[test]
    fn declares_context_and_output_limits_from_the_catalog() {
        let provider = provider_with(
            &["anthropic/claude-opus-5"],
            &[("anthropic/claude-opus-5", 1_000_000)],
        );
        let model = &provider["models"]["anthropic/claude-opus-5"];
        assert_eq!(model["limit"]["context"], serde_json::json!(1_000_000));
        assert_eq!(
            model["limit"]["output"],
            serde_json::json!(KILO_OUTPUT_TOKEN_MAX)
        );
    }

    #[test]
    fn declares_reasoning_variants_from_the_catalog() {
        let provider = provider_with_reasoning(
            "anthropic/claude-opus-5",
            &["none", "low", "medium", "high", "xhigh", "max"],
        );
        let model = &provider["models"]["anthropic/claude-opus-5"];

        assert_eq!(model["reasoning"], serde_json::json!(true));
        for effort in ["none", "low", "medium", "high", "xhigh", "max"] {
            assert_eq!(
                model["variants"][effort]["reasoningEffort"],
                serde_json::json!(effort)
            );
        }
        assert_eq!(model["variants"].as_object().map(|v| v.len()), Some(6));
    }

    #[test]
    fn omits_reasoning_and_variants_without_catalog_efforts() {
        let provider = provider_with(&["openai/gpt-4.1"], &[]);
        let model = &provider["models"]["openai/gpt-4.1"];
        assert!(model.get("reasoning").is_none());
        assert!(model.get("variants").is_none());
    }

    #[test]
    fn omits_limit_when_the_catalog_has_no_context_size() {
        let provider = provider_with(&["openai/gpt-5"], &[]);
        let model = &provider["models"]["openai/gpt-5"];
        assert_eq!(model["name"], serde_json::json!("openai/gpt-5"));
        assert!(model.get("limit").is_none());
    }

    #[test]
    fn limits_are_matched_per_model_not_applied_wholesale() {
        let provider = provider_with(
            &["anthropic/claude-haiku-4-5", "openai/gpt-5"],
            &[("anthropic/claude-haiku-4-5", 200_000)],
        );
        assert_eq!(
            provider["models"]["anthropic/claude-haiku-4-5"]["limit"]["context"],
            serde_json::json!(200_000)
        );
        assert!(provider["models"]["openai/gpt-5"].get("limit").is_none());
    }

    #[test]
    fn declares_per_million_token_rates() {
        let provider = provider_priced(
            "anthropic/claude-sonnet-4-5",
            crate::api::GatewayModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
            },
        );
        let cost = &provider["models"]["anthropic/claude-sonnet-4-5"]["cost"];
        assert_eq!(cost["input"], serde_json::json!(3.0));
        assert_eq!(cost["output"], serde_json::json!(15.0));
        assert_eq!(cost["cache_read"], serde_json::json!(0.3));
        assert_eq!(cost["cache_write"], serde_json::json!(3.75));
    }

    /// Inherited from OpenCode: the config merge drops `cost.context_over_200k`,
    /// so emitting it would imply a long-context tier that never takes effect.
    #[test]
    fn never_declares_tiered_pricing() {
        let provider = provider_priced(
            "anthropic/claude-sonnet-4-5",
            crate::api::GatewayModelCost {
                input: 3.0,
                output: 15.0,
                cache_read: 0.3,
                cache_write: 3.75,
            },
        );
        let cost = &provider["models"]["anthropic/claude-sonnet-4-5"]["cost"];
        assert!(cost.get("context_over_200k").is_none());
    }

    #[test]
    fn omits_cost_when_the_catalog_has_no_rates() {
        let provider = provider_with(&["openai/gpt-5"], &[("openai/gpt-5", 400_000)]);
        assert!(provider["models"]["openai/gpt-5"].get("cost").is_none());
    }

    #[test]
    fn omits_the_models_map_entirely_when_the_gateway_listed_none() {
        let provider = provider_with(&[], &[]);
        assert!(provider.get("models").is_none());
    }

    /// The payload is deep-merged over the user's own config, so it must carry
    /// the Edgee provider and nothing that could clobber their settings.
    #[test]
    fn config_content_carries_only_the_edgee_provider() {
        let config = build_config_content(provider_with(&["openai/gpt-5"], &[]));
        let obj = config.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        keys.sort();
        assert_eq!(keys, vec!["$schema", "provider"]);
        let providers = config["provider"].as_object().unwrap();
        assert_eq!(providers.keys().collect::<Vec<_>>(), vec!["edgee"]);
    }
}
