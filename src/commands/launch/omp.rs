//! `edgee launch omp`: Oh My Pi CLI (<https://github.com/can1357/oh-my-pi>).
//!
//! OMP is a Pi fork with the same custom-provider schema. It resolves models
//! through `~/.omp/agent/models.yml`, where custom providers merge into the
//! built-in catalog by `provider + id`, so registering providers named `edgee`
//! and `edgee-anthropic` is purely additive: every provider, model and login
//! the user already had keeps working untouched.
//!
//! `edgee launch pi` does not share this path anymore; Pi gets the `pi-edgee`
//! extension instead (see `pi.rs`). OMP still uses the provider-file approach.
//!
//! ## Why this writes the user's real config instead of a temp one
//!
//! `opencode.rs` and `crush.rs` build a merged config in `$TMPDIR` and point the
//! agent at it (`OPENCODE_CONFIG`, `CRUSH_GLOBAL_CONFIG`), leaving the user's
//! files untouched. OMP has no equivalent lever, and its agent directory is not
//! overridable, so the two provider blocks go into the real `models.yml` under
//! namespaced keys: one uses Anthropic Messages for Claude models, the other
//! uses Chat Completions for everything else. This needs no patch-and-revert
//! dance (unlike `codex_desktop.rs`) precisely because it is additive rather
//! than a hijack of a provider the user already relies on.
//!
//! ## No credential at rest
//!
//! OMP looks up the complete `apiKey` or header value as an environment
//! variable name when it is set, so the generated blocks store the bare names
//! `EDGEE_API_KEY` / `EDGEE_SESSION_ID` and `run` supplies their values at
//! spawn time. The Edgee key is never written to disk, unlike the OpenCode and
//! Crush temp configs, which embed it.
//!
//! The flip side: a bare `omp` run outside `edgee launch omp` can see the Edgee
//! models but cannot authenticate them. That is the deliberate trade, the
//! credential stays out of the config file.

use anyhow::{Context, Result};
use serde_json::Value;

use super::util;
use crate::commands::util::plugins;

/// Provider keys under `providers` in `models.yml`. Everything this command
/// writes lives under them; nothing else in the file is touched.
const PROVIDER_KEY: &str = "edgee";
const ANTHROPIC_PROVIDER_KEY: &str = "edgee-anthropic";

/// Env vars whose values `run` supplies at spawn time. The config embeds their
/// names, never their values.
const API_KEY_ENV: &str = "EDGEE_API_KEY";
const SESSION_ID_ENV: &str = "EDGEE_SESSION_ID";

/// OMP's picker uses fixed slot names. The catalog's `none` effort maps to the
/// disabled `off` slot.
const THINKING_LEVELS: [(&str, &str); 7] = [
    ("off", "none"),
    ("minimal", "minimal"),
    ("low", "low"),
    ("medium", "medium"),
    ("high", "high"),
    ("xhigh", "xhigh"),
    ("max", "max"),
];

fn thinking_level_map(efforts: &[String]) -> Value {
    Value::Object(
        THINKING_LEVELS
            .into_iter()
            .map(|(level, catalog_effort)| {
                let value = if efforts.iter().any(|effort| effort == catalog_effort) {
                    Value::String(catalog_effort.to_string())
                } else {
                    Value::Null
                };
                (level.to_string(), value)
            })
            .collect(),
    )
}

/// Output cap declared for every model. The gateway catalog carries a context
/// window but no per-model output limit, and there is no "unset" for
/// `maxTokens` short of omitting it, which makes the agent fall back to a
/// conservative built-in default. This is high enough not to truncate coding
/// turns.
const OUTPUT_TOKEN_MAX: u64 = 64_000;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the agent CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(std::path::PathBuf::from)
}

/// OMP has no env override for its agent directory.
fn models_path() -> Option<std::path::PathBuf> {
    Some(home_dir()?.join(".omp").join("agent").join("models.yml"))
}

fn plugin_args(report: &plugins::sync::SyncReport) -> Vec<String> {
    report
        .plugin_dirs
        .iter()
        .map(|path| format!("--plugin-dir={}", path.to_string_lossy()))
        .collect()
}

/// Reads the agent's model config, or an empty document when absent. A file we
/// cannot parse is *not* overwritten, see [`write_providers`].
fn read_models_config(path: &std::path::Path) -> Result<Option<Value>> {
    if !path.exists() {
        return Ok(Some(serde_json::json!({ "providers": {} })));
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    // An empty file is equivalent to no file; OMP treats both as "no overrides".
    if content.trim().is_empty() {
        return Ok(Some(serde_json::json!({ "providers": {} })));
    }
    Ok(serde_yaml::from_str(&content).ok())
}

/// Replaces Edgee's providers in `models.yml`, preserving every other key.
///
/// Bails rather than clobbering when the existing file does not parse: a
/// user's annotated config is not ours to silently rewrite.
fn write_providers(path: &std::path::Path, replacements: &[(&str, Value)]) -> Result<()> {
    let Some(mut config) = read_models_config(path)? else {
        anyhow::bail!(
            "{} exists but is not valid YAML.\nFix or remove it, then launch the agent again.",
            path.display(),
        )
    };

    if !config.is_object() {
        anyhow::bail!(
            "{} does not contain a YAML mapping.\nFix or remove it, then launch the agent again.",
            path.display()
        )
    }

    let obj = config.as_object_mut().expect("checked above");
    let providers = obj
        .entry("providers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !providers.is_object() {
        *providers = Value::Object(serde_json::Map::new());
    }
    let providers = providers.as_object_mut().expect("just ensured object");
    // Remove both managed keys first so a catalog family that disappears does
    // not leave stale models in the picker.
    providers.remove(PROVIDER_KEY);
    providers.remove(ANTHROPIC_PROVIDER_KEY);
    for (key, provider) in replacements {
        providers.insert((*key).to_string(), provider.clone());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let rendered = serde_yaml::to_string(&config)?;
    std::fs::write(path, format!("{rendered}\n"))
        .with_context(|| format!("Failed to write {}", path.display()))
}

#[derive(Clone, Copy)]
enum Transport {
    ChatCompletions,
    AnthropicMessages,
}

/// Builds one of the two Edgee provider blocks. Anthropic models use the native
/// Messages transport so its normal prompt-cache behavior is retained; all
/// other models use Chat Completions.
fn build_provider(
    gateway_url: &str,
    models: &[String],
    catalog: &util::ModelCatalog,
    debug_log_headers: Option<crate::crypto::DebugLogHeaderValues>,
    transport: Transport,
) -> Value {
    let mut headers = serde_json::json!({
        "x-edgee-api-key": API_KEY_ENV,
        "x-edgee-session-id": SESSION_ID_ENV,
    });
    if matches!(transport, Transport::AnthropicMessages) {
        headers["User-Agent"] = Value::String("omp".to_string());
    }
    // Unlike the key and session id, these are embedded literally: they derive
    // from the profile passphrase and so are stable across launches, and a
    // public key plus salt is not a secret. Env-var indirection would only add
    // a way for them to resolve to nothing on a bare `omp` run.
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

    let gateway_url = gateway_url.trim_end_matches('/');
    let (name, base_url, api) = match transport {
        Transport::ChatCompletions => (
            "Edgee",
            format!("{gateway_url}/v1"),
            "openai-completions",
        ),
        // The agent appends `/v1/messages` for this transport, matching its
        // built-in Anthropic provider. Including `/v1` here would duplicate it.
        Transport::AnthropicMessages => (
            "Edgee (Anthropic)",
            gateway_url.to_string(),
            "anthropic-messages",
        ),
    };
    let mut provider = serde_json::json!({
        "name": name,
        "baseUrl": base_url,
        "api": api,
        "apiKey": API_KEY_ENV,
        "headers": headers,
    });

    if !models.is_empty() {
        let entries: Vec<Value> = models
            .iter()
            .map(|id| {
                // The gateway id (`anthropic/claude-sonnet-5`) is the routing
                // identifier, so it doubles as the agent's model id. `--model`
                // matches on id as well as name, so `--model
                // anthropic/claude-sonnet-5` works without a provider prefix.
                let mut entry = serde_json::json!({ "id": id, "name": id });
                let metadata = catalog.get(id);
                if let Some(input) = metadata.map(|m| {
                    m.input_modalities
                        .iter()
                        .filter(|modality| matches!(modality.as_str(), "text" | "image"))
                        .collect::<Vec<_>>()
                }) {
                    if !input.is_empty() {
                        entry["input"] = serde_json::json!(input);
                    }
                }
                if let Some(context) = metadata.and_then(|m| m.context) {
                    entry["contextWindow"] = serde_json::json!(context);
                    entry["maxTokens"] = serde_json::json!(OUTPUT_TOKEN_MAX);
                }
                // Dollars per million tokens, the same unit the built-in
                // catalog uses. Without this every session reports as $0.
                if let Some(cost) = metadata.and_then(|m| m.cost) {
                    entry["cost"] = serde_json::json!({
                        "input": cost.input,
                        "output": cost.output,
                        "cacheRead": cost.cache_read,
                        "cacheWrite": cost.cache_write,
                    });
                }
                if let Some(efforts) = metadata
                    .map(|m| m.reasoning_efforts.as_slice())
                    .filter(|efforts| !efforts.is_empty())
                {
                    entry["reasoning"] = Value::Bool(true);
                    entry["thinkingLevelMap"] = thinking_level_map(efforts);
                    if matches!(transport, Transport::AnthropicMessages) {
                        // The gateway accepts the adaptive control and maps it
                        // to the ultimately selected Claude provider.
                        entry["compat"] = serde_json::json!({
                            "forceAdaptiveThinking": true,
                        });
                    }
                }
                if matches!(transport, Transport::AnthropicMessages) {
                    // OMP's Anthropic transport deliberately speaks Claude Code's
                    // OAuth wire protocol. Keep its working bearer auth, but retain
                    // OMP's identity so the gateway uses the normal Messages path
                    // and the prompt has no volatile Claude Code `cch` block.
                    entry["compat"]["allowAnthropicHeaderOverrides"] = Value::Bool(true);
                    entry["compat"]["disableStrictTools"] = Value::Bool(true);
                    entry["compat"]["injectClaudeCodeInstruction"] = Value::Bool(false);
                }
                entry
            })
            .collect();
        provider["models"] = Value::Array(entries);
    }

    provider
}

fn build_edgee_provider(
    gateway_url: &str,
    models: &[String],
    catalog: &util::ModelCatalog,
    debug_log_headers: Option<crate::crypto::DebugLogHeaderValues>,
) -> Value {
    build_provider(
        gateway_url,
        models,
        catalog,
        debug_log_headers,
        Transport::ChatCompletions,
    )
}

fn build_anthropic_provider(
    gateway_url: &str,
    models: &[String],
    catalog: &util::ModelCatalog,
    debug_log_headers: Option<crate::crypto::DebugLogHeaderValues>,
) -> Value {
    build_provider(
        gateway_url,
        models,
        catalog,
        debug_log_headers,
        Transport::AnthropicMessages,
    )
}

pub async fn run(opts: Options, reroute: &super::reroute::Reroute) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Step 1: ensure we are authenticated
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }

    // Step 1b: ensure an org is selected (handles partial state after aborted login)
    crate::commands::auth::login::ensure_org_selected().await?;

    // Step 2: ensure a valid key. OMP is Pi-compatible and reuses the `pi`
    // coding-agent key, so sessions share its backend attribution and settings
    // rather than provisioning another key.
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("pi")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("pi").await?;
    }
    creds = crate::config::read()?;

    // Default the connection to the plan flavor on first use.
    if creds
        .pi
        .as_ref()
        .and_then(|c| c.connection.as_deref())
        .is_none()
    {
        let provider = creds.pi.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    let pi = creds.pi.as_ref().unwrap();
    let api_key = &pi.api_key;
    let session_id = reroute.create_session(&creds, "pi").await?;
    util::spawn_cli_version_report(&creds, &session_id);

    let gateway_url = super::resolve_gateway_base_url(&creds).await;

    let (models, catalog) = tokio::join!(
        util::fetch_gateway_models(&gateway_url, api_key),
        util::fetch_model_catalog(&creds)
    );
    let models = util::without_app_subscription_models(models, &catalog);

    // A custom provider is defined by its models, so registering one with none
    // opens the session on "No models available". `fetch_gateway_models` is
    // best-effort and returns empty on any failure, so bail before writing
    // rather than leaving a dead provider in the user's config.
    if models.is_empty() {
        anyhow::bail!(
            "The gateway at {gateway_url} returned no models, and OMP needs an explicit model list.\n\
             Check that the gateway is reachable and that its model catalog is populated \
             (`curl {gateway_url}/v1/models`), then run `edgee launch omp` again."
        );
    }
    let (anthropic_models, other_models): (Vec<_>, Vec<_>) = models
        .into_iter()
        .partition(|id| id.starts_with("anthropic/"));
    let debug_log_headers = util::resolve_debug_log_keypair()?.map(|k| k.header_values());
    let mut providers = Vec::with_capacity(2);
    if !other_models.is_empty() {
        providers.push((
            PROVIDER_KEY,
            build_edgee_provider(
                &gateway_url,
                &other_models,
                &catalog,
                debug_log_headers.clone(),
            ),
        ));
    }
    if !anthropic_models.is_empty() {
        providers.push((
            ANTHROPIC_PROVIDER_KEY,
            build_anthropic_provider(&gateway_url, &anthropic_models, &catalog, debug_log_headers),
        ));
    }

    let models_path = models_path().context("Could not determine your home directory")?;
    write_providers(&models_path, &providers)?;

    // Org-managed plugins/skills are additive and never block the launch.
    let plugin_report = plugins::sync_for_target(&creds, plugins::Target::Omp).await;
    let mut cmd = std::process::Command::new(util::resolve_binary("omp"));
    cmd.env(API_KEY_ENV, api_key);
    cmd.env(SESSION_ID_ENV, &session_id);
    cmd.env("EDGEE_ORG_SLUG", creds.org_slug.as_deref().unwrap_or_default());
    cmd.args(plugin_args(&plugin_report));
    cmd.args(&opts.args);
    plugins::report_launch(&plugin_report);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "OMP is not installed. Install it from https://github.com/can1357/oh-my-pi"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    super::print_session_stats(&creds, &session_id, "OMP").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_yaml(path: &std::path::Path) -> Value {
        serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn plugin_flags_match_the_agent_parser() {
        let report = plugins::sync::SyncReport {
            skills_root: Some(std::path::PathBuf::from("/tmp/edgee/omp/skills")),
            plugin_dirs: vec![
                std::path::PathBuf::from("/tmp/edgee/omp/alpha"),
                std::path::PathBuf::from("/tmp/edgee/omp/beta"),
            ],
            ..Default::default()
        };

        assert_eq!(
            plugin_args(&report),
            [
                "--plugin-dir=/tmp/edgee/omp/alpha",
                "--plugin-dir=/tmp/edgee/omp/beta"
            ]
        );
    }

    fn catalog_with_efforts(
        id: &str,
        context: Option<u64>,
        efforts: &[&str],
    ) -> util::ModelCatalog {
        let mut catalog = util::ModelCatalog::new();
        catalog.insert(
            id.to_string(),
            util::ModelMetadata {
                context,
                cost: None,
                reasoning_efforts: efforts.iter().map(|effort| effort.to_string()).collect(),
                input_modalities: Vec::new(),
                app_subscription_only: false,
            },
        );
        catalog
    }

    fn catalog_with(id: &str, context: Option<u64>) -> util::ModelCatalog {
        catalog_with_efforts(id, context, &[])
    }

    #[test]
    fn declares_input_modalities_from_the_catalog() {
        let models = vec!["anthropic/claude-opus-5".to_string()];
        let catalog: util::ModelCatalog = [(
            "anthropic/claude-opus-5".to_string(),
            util::ModelMetadata {
                input_modalities: ["text", "image", "audio", "video", "pdf"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect();

        let provider = build_edgee_provider("https://api.edgee.ai", &models, &catalog, None);
        assert_eq!(
            provider["models"][0]["input"],
            serde_json::json!(["text", "image"])
        );
    }

    #[test]
    fn uses_bare_env_names_as_references() {
        let provider =
            build_edgee_provider("https://api.edgee.ai", &[], &util::ModelCatalog::new(), None);

        // OMP treats the complete value as the variable name; the credential
        // itself is never stored.
        assert_eq!(provider["apiKey"], "EDGEE_API_KEY");
        assert_eq!(provider["headers"]["x-edgee-api-key"], "EDGEE_API_KEY");
        assert_eq!(
            provider["headers"]["x-edgee-session-id"],
            "EDGEE_SESSION_ID"
        );
    }

    #[test]
    fn embeds_debug_log_headers_literally() {
        // These are not env references: an unset reference resolves to nothing,
        // and a pubkey/salt pair is derived from the passphrase, not secret.
        let provider = build_edgee_provider(
            "https://api.edgee.ai",
            &[],
            &util::ModelCatalog::new(),
            Some(crate::crypto::DebugLogHeaderValues {
                pubkey: "pubkey-b64".to_string(),
                salt: "salt-b64".to_string(),
            }),
        );
        assert_eq!(provider["headers"]["x-edgee-debug-pubkey"], "pubkey-b64");
        assert_eq!(provider["headers"]["x-edgee-debug-salt"], "salt-b64");
    }

    #[test]
    fn uses_openai_chat_completions_endpoint() {
        // The agent appends `/chat/completions` to an OpenAI-compatible base URL.
        let provider =
            build_edgee_provider("https://api.edgee.ai", &[], &util::ModelCatalog::new(), None);
        assert_eq!(provider["baseUrl"], "https://api.edgee.ai/v1");
        assert_eq!(provider["api"], "openai-completions");

        let provider = build_edgee_provider(
            "https://api.edgee.ai/",
            &[],
            &util::ModelCatalog::new(),
            None,
        );
        assert_eq!(provider["baseUrl"], "https://api.edgee.ai/v1");
    }

    #[test]
    fn anthropic_models_use_the_native_messages_endpoint() {
        let provider = build_anthropic_provider(
            "https://api.edgee.ai/",
            &["anthropic/claude-sonnet-5".to_string()],
            &util::ModelCatalog::new(),
            None,
        );

        assert_eq!(provider["baseUrl"], "https://api.edgee.ai");
        assert_eq!(provider["api"], "anthropic-messages");
        assert_eq!(provider["models"][0]["id"], "anthropic/claude-sonnet-5");
    }

    #[test]
    fn declares_context_window_only_when_the_catalog_has_one() {
        let models = vec!["anthropic/claude-sonnet-5".to_string(), "zai/glm-5.2".to_string()];
        let catalog = catalog_with("anthropic/claude-sonnet-5", Some(1_000_000));

        let provider = build_edgee_provider("https://api.edgee.ai", &models, &catalog, None);
        let entries = provider["models"].as_array().unwrap();

        assert_eq!(entries[0]["id"], "anthropic/claude-sonnet-5");
        assert_eq!(entries[0]["contextWindow"], 1_000_000);
        assert_eq!(entries[0]["maxTokens"], OUTPUT_TOKEN_MAX);
        // Unknown to the catalog: better to let the agent apply its own defaults
        // than to declare a fabricated window.
        assert_eq!(entries[1]["id"], "zai/glm-5.2");
        assert!(entries[1].get("contextWindow").is_none());
    }

    #[test]
    fn declares_reasoning_levels_from_the_catalog() {
        let models = vec!["anthropic/claude-sonnet-5".to_string()];
        let catalog = catalog_with_efforts(
            "anthropic/claude-sonnet-5",
            Some(1_000_000),
            &["none", "low", "medium", "high", "xhigh", "max"],
        );

        let provider = build_edgee_provider("https://api.edgee.ai", &models, &catalog, None);
        let model = &provider["models"][0];

        assert_eq!(model["reasoning"], serde_json::json!(true));
        assert_eq!(model["thinkingLevelMap"]["off"], "none");
        assert_eq!(model["thinkingLevelMap"]["minimal"], Value::Null);
        for effort in ["low", "medium", "high", "xhigh", "max"] {
            assert_eq!(model["thinkingLevelMap"][effort], effort);
        }
        assert!(model.get("compat").is_none());
    }

    #[test]
    fn anthropic_reasoning_uses_adaptive_thinking() {
        let models = vec!["anthropic/claude-sonnet-5".to_string()];
        let catalog = catalog_with_efforts(
            "anthropic/claude-sonnet-5",
            Some(1_000_000),
            &["none", "low", "medium", "high", "xhigh", "max"],
        );
        let provider = build_anthropic_provider("https://api.edgee.ai", &models, &catalog, None);

        assert_eq!(
            provider["models"][0]["compat"]["forceAdaptiveThinking"],
            true
        );
    }

    #[test]
    fn anthropic_models_keep_the_omp_identity() {
        let provider = build_anthropic_provider(
            "https://api.edgee.ai",
            &["anthropic/claude-sonnet-5".to_string()],
            &util::ModelCatalog::new(),
            None,
        );

        assert_eq!(provider["headers"]["User-Agent"], "omp");
        assert_eq!(
            provider["models"][0]["compat"]["allowAnthropicHeaderOverrides"],
            true
        );
        assert_eq!(
            provider["models"][0]["compat"]["disableStrictTools"],
            true
        );
        assert_eq!(
            provider["models"][0]["compat"]["injectClaudeCodeInstruction"],
            false
        );
    }

    #[test]
    fn hides_levels_the_catalog_does_not_support() {
        let models = vec!["deepseek/deepseek-v4-pro".to_string()];
        let catalog = catalog_with_efforts(
            "deepseek/deepseek-v4-pro",
            None,
            &["low", "high", "max"],
        );

        let provider = build_edgee_provider("https://api.edgee.ai", &models, &catalog, None);
        let levels = &provider["models"][0]["thinkingLevelMap"];

        assert_eq!(levels["off"], Value::Null);
        assert_eq!(levels["minimal"], Value::Null);
        assert_eq!(levels["low"], "low");
        assert_eq!(levels["medium"], Value::Null);
        assert_eq!(levels["high"], "high");
        assert_eq!(levels["xhigh"], Value::Null);
        assert_eq!(levels["max"], "max");
    }

    #[test]
    fn omits_reasoning_fields_without_catalog_efforts() {
        let models = vec!["openai/gpt-4.1".to_string()];
        let catalog = catalog_with("openai/gpt-4.1", Some(1_000_000));

        let provider = build_edgee_provider("https://api.edgee.ai", &models, &catalog, None);
        let model = &provider["models"][0];
        assert!(model.get("reasoning").is_none());
        assert!(model.get("thinkingLevelMap").is_none());
        assert!(model.get("compat").is_none());
    }

    #[test]
    fn models_path_is_the_omp_yaml_file() {
        assert_eq!(
            models_path().map(|path| path.file_name().unwrap().to_owned()),
            Some(std::ffi::OsString::from("models.yml"))
        );
    }

    #[test]
    fn preserves_unrelated_providers_when_registering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.yml");
        std::fs::write(
            &path,
            "providers:\n  local:\n    api: openai-completions\n    baseUrl: http://localhost:8080/v1\n    apiKey: ollama\n",
        )
        .unwrap();

        let provider =
            build_edgee_provider("https://stg.edgee.io", &[], &util::ModelCatalog::new(), None);
        write_providers(&path, &[(PROVIDER_KEY, provider)]).unwrap();

        let written = read_yaml(&path);
        // The user's own provider survives untouched...
        assert_eq!(
            written["providers"]["local"]["baseUrl"],
            "http://localhost:8080/v1"
        );
        assert_eq!(written["providers"]["local"]["apiKey"], "ollama");
        // ...alongside ours.
        assert_eq!(written["providers"]["edgee"]["name"], "Edgee");
        assert_eq!(
            written["providers"]["edgee"]["baseUrl"],
            "https://stg.edgee.io/v1"
        );
    }

    #[test]
    fn replaces_only_its_own_providers_on_relaunch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.yml");

        let first = build_edgee_provider(
            "https://api.edgee.ai",
            &["anthropic/claude-sonnet-5".to_string()],
            &catalog_with("anthropic/claude-sonnet-5", Some(1_000_000)),
            None,
        );
        let anthropic = build_anthropic_provider(
            "https://api.edgee.ai",
            &["anthropic/claude-sonnet-5".to_string()],
            &catalog_with("anthropic/claude-sonnet-5", Some(1_000_000)),
            None,
        );
        write_providers(
            &path,
            &[
                (PROVIDER_KEY, first),
                (ANTHROPIC_PROVIDER_KEY, anthropic),
            ],
        )
        .unwrap();
        let second = build_edgee_provider(
            "https://gateway.example.com",
            &[],
            &util::ModelCatalog::new(),
            None,
        );
        write_providers(&path, &[(PROVIDER_KEY, second)]).unwrap();

        let written = read_yaml(&path);
        // Rewritten wholesale rather than merged, so a model dropped from the
        // gateway catalog does not linger in the user's picker forever.
        assert_eq!(
            written["providers"]["edgee"]["baseUrl"],
            "https://gateway.example.com/v1"
        );
        assert!(written["providers"]["edgee"].get("models").is_none());
        assert!(written["providers"].get(ANTHROPIC_PROVIDER_KEY).is_none());
    }

    #[test]
    fn refuses_to_clobber_an_unparseable_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.yml");
        let original = "providers: [unterminated";
        std::fs::write(&path, original).unwrap();

        let provider =
            build_edgee_provider("https://api.edgee.ai", &[], &util::ModelCatalog::new(), None);
        assert!(write_providers(&path, &[(PROVIDER_KEY, provider)]).is_err());
        // The user's file is left exactly as it was.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn creates_the_agent_dir_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("agent").join("models.yml");

        let provider =
            build_edgee_provider("https://api.edgee.ai", &[], &util::ModelCatalog::new(), None);
        write_providers(&path, &[(PROVIDER_KEY, provider)]).unwrap();

        assert_eq!(read_yaml(&path)["providers"]["edgee"]["name"], "Edgee");
    }
}
