use anyhow::Result;
use serde_json::Value;

use super::util;
use crate::commands::util::plugins;

/// OpenCode clamps every request's `max_tokens` to its own `OUTPUT_TOKEN_MAX` of
/// 32k (`maxOutputTokens()` in its provider transform), and falls back to that
/// same value when a model's output limit is `0`. Its config schema requires
/// `output` whenever `limit` is present, so declaring 32k satisfies the schema
/// while leaving the request identical to what OpenCode would send on its own.
const OPENCODE_OUTPUT_TOKEN_MAX: u64 = 32_000;

/// OpenCode 2.x config shape is incompatible with v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigVersion {
    V1,
    V2,
}

/// Unparseable output falls back to v1.
fn parse_config_version(output: &str) -> ConfigVersion {
    let major = output
        .split_whitespace()
        .find_map(|token| {
            token
                .trim_start_matches('v')
                .split('.')
                .next()
                .and_then(|m| m.parse::<u64>().ok())
        })
        .unwrap_or(1);
    if major >= 2 {
        ConfigVersion::V2
    } else {
        ConfigVersion::V1
    }
}

fn detect_config_version(binary: &std::ffi::OsStr) -> ConfigVersion {
    std::process::Command::new(binary)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| parse_config_version(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or(ConfigVersion::V1)
}

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the opencode CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

fn strip_jsonc(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escape_next = false;

    while let Some(c) = chars.next() {
        if escape_next {
            result.push(c);
            escape_next = false;
            continue;
        }

        if in_string {
            match c {
                '\\' => {
                    result.push(c);
                    escape_next = true;
                    continue;
                }
                '"' => {
                    result.push(c);
                    in_string = false;
                    continue;
                }
                _ => {
                    result.push(c);
                    continue;
                }
            }
        }

        match c {
            '"' => {
                result.push(c);
                in_string = true;
            }
            '/' => match chars.peek() {
                Some('/') => {
                    chars.next();
                    for ch in chars.by_ref() {
                        if ch == '\n' {
                            result.push(ch);
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    loop {
                        match chars.next() {
                            Some('*') if matches!(chars.peek(), Some('/')) => {
                                chars.next();
                                break;
                            }
                            None => break,
                            _ => {}
                        }
                    }
                }
                _ => result.push(c),
            },
            _ => result.push(c),
        }
    }

    result
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(std::path::PathBuf::from)
}

fn find_user_config() -> Option<Value> {
    let candidates: Vec<std::path::PathBuf> = {
        let mut paths = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            paths.push(cwd.join("opencode.json"));
            paths.push(cwd.join("opencode.jsonc"));
        }
        if let Some(home) = home_dir() {
            let config_dir = home.join(".config").join("opencode");
            paths.push(config_dir.join("opencode.json"));
            paths.push(config_dir.join("opencode.jsonc"));
        }
        #[cfg(windows)]
        if let Ok(appdata) = std::env::var("APPDATA") {
            let config_dir = std::path::PathBuf::from(appdata).join("opencode");
            paths.push(config_dir.join("opencode.json"));
            paths.push(config_dir.join("opencode.jsonc"));
        }
        paths
    };

    for path in candidates {
        if !path.exists() {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let parsed: Value = if path.extension().is_some_and(|ext| ext == "jsonc") {
            serde_json::from_str(&strip_jsonc(&content)).ok()?
        } else {
            serde_json::from_str(&content).ok()?
        };

        if parsed.is_object() {
            return Some(parsed);
        }
    }

    None
}

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
    // OpenCode's picker is populated with the live catalog. The gateway `id`
    // (e.g. `anthropic/claude-opus-4-8`) is already the routing identifier the
    // gateway accepts, so it serves as both the map key and the display name.
    let mut headers = serde_json::json!({
        "x-edgee-api-key": api_key,
        "x-edgee-session-id": session_id,
    });
    if let (Some(headers_obj), Some(debug_headers)) = (headers.as_object_mut(), debug_log_headers) {
        headers_obj.insert("x-edgee-debug-pubkey".to_string(), Value::String(debug_headers.pubkey));
        headers_obj.insert("x-edgee-debug-salt".to_string(), Value::String(debug_headers.salt));
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
            // a fabricated one is worse than letting OpenCode fall back to 0.
            if let Some(context) = metadata.and_then(|m| m.context) {
                entry["limit"] = serde_json::json!({
                    "context": context,
                    "output": OPENCODE_OUTPUT_TOKEN_MAX,
                });
            }
            // Rates are dollars per million tokens, the same unit OpenCode's own
            // catalog uses. Without them every session reports as costing $0.
            //
            // Tiered pricing is deliberately not emitted: the catalog has a
            // long-context premium for a few models and OpenCode's config schema
            // has a `cost.context_over_200k` slot, but its config merge drops that
            // field (it survives only on the models.dev path), so declaring it
            // would imply a tier that never takes effect.
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

/// v2 equivalent of [`build_edgee_provider`].
fn build_edgee_provider_v2(
    api_key: &str,
    session_id: &str,
    gateway_url: &str,
    models: &[String],
    catalog: &util::ModelCatalog,
    debug_log_headers: Option<crate::crypto::DebugLogHeaderValues>,
) -> Value {
    let mut headers = serde_json::json!({
        "x-edgee-api-key": api_key,
        "x-edgee-session-id": session_id,
    });
    if let (Some(headers_obj), Some(debug_headers)) = (headers.as_object_mut(), debug_log_headers) {
        headers_obj.insert("x-edgee-debug-pubkey".to_string(), Value::String(debug_headers.pubkey));
        headers_obj.insert("x-edgee-debug-salt".to_string(), Value::String(debug_headers.salt));
    }

    let mut provider = serde_json::json!({
        "name": "Edgee",
        "package": "@opencode/ai/providers/openai-compatible",
        "settings": {
            "baseURL": format!("{}/v1", gateway_url),
            "apiKey": api_key,
        },
        "headers": headers,
    });

    if !models.is_empty() {
        let mut models_map = serde_json::Map::new();
        for id in models {
            let mut entry = serde_json::json!({ "modelID": id, "name": id });
            let metadata = catalog.get(id);
            if let Some(input) = metadata
                .map(|m| m.input_modalities.as_slice())
                .filter(|input| !input.is_empty())
            {
                entry["capabilities"] = serde_json::json!({
                    "tools": true,
                    "input": input,
                    "output": ["text"],
                });
            }
            if let Some(context) = metadata.and_then(|m| m.context) {
                entry["limit"] = serde_json::json!({
                    "context": context,
                    "output": OPENCODE_OUTPUT_TOKEN_MAX,
                });
            }
            if let Some(cost) = metadata.and_then(|m| m.cost) {
                entry["cost"] = serde_json::json!({
                    "input": cost.input,
                    "output": cost.output,
                    "cache": {
                        "read": cost.cache_read,
                        "write": cost.cache_write,
                    },
                });
            }
            if let Some(efforts) = metadata
                .map(|m| m.reasoning_efforts.as_slice())
                .filter(|efforts| !efforts.is_empty())
            {
                entry["variants"] = efforts
                    .iter()
                    .map(|effort| {
                        serde_json::json!({
                            "id": effort,
                            "settings": { "reasoningEffort": effort },
                        })
                    })
                    .collect();
            }
            models_map.insert(id.clone(), entry);
        }
        provider["models"] = Value::Object(models_map);
    }

    provider
}

fn merge_nested_object(config: &mut Value, parent: &str, key: &str, value: Value) {
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    let parent_obj = obj
        .entry(parent.to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    plugins::config::merge_object(parent_obj, key, value);
}

/// The `mcp.edgee` entry registering Edgee's own session-tracking MCP
/// server, same `opencode_mcp` shape `plugins/config.rs` already uses for
/// third-party plugin servers (`"remote"` discriminant, `enabled: true`).
fn build_edgee_mcp(token: &str) -> Value {
    serde_json::json!({
        "edgee": {
            "type": "remote",
            "url": crate::config::mcp_base_url(),
            "enabled": true,
            "headers": {
                "Authorization": format!("Bearer {token}")
            }
        }
    })
}

/// Appends `path` to a top-level string array, without duplicates.
fn push_top_level_path(config: &mut Value, key: &str, path: &str) {
    let Some(obj) = config.as_object_mut() else {
        return;
    };
    let list = obj
        .entry(key)
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(array) = list.as_array_mut() else {
        return;
    };
    let entry = Value::String(path.to_string());
    if !array.contains(&entry) {
        array.push(entry);
    }
}

fn rename_entry_field(map: &mut Value, from: &str, to: &str, convert: fn(Value) -> Value) {
    let Some(entries) = map.as_object_mut() else {
        return;
    };
    for entry in entries.values_mut().filter_map(Value::as_object_mut) {
        if let Some(value) = entry.remove(from) {
            entry.insert(to.to_string(), convert(value));
        }
    }
}

/// v2 nests servers under `mcp.servers` and uses `disabled` instead of `enabled`.
fn merge_mcp(config: &mut Value, version: ConfigVersion, mut servers: Value) {
    match version {
        ConfigVersion::V1 => plugins::config::merge_object(config, "mcp", servers),
        ConfigVersion::V2 => {
            rename_entry_field(&mut servers, "enabled", "disabled", |enabled| {
                Value::Bool(!enabled.as_bool().unwrap_or(true))
            });
            merge_nested_object(config, "mcp", "servers", servers);
        }
    }
}

/// v2 uses `agents` and reads the prompt from `system`.
fn merge_agents(config: &mut Value, version: ConfigVersion, mut agents: Value) {
    match version {
        ConfigVersion::V1 => plugins::config::merge_object(config, "agent", agents),
        ConfigVersion::V2 => {
            rename_entry_field(&mut agents, "prompt", "system", |prompt| prompt);
            plugins::config::merge_object(config, "agents", agents);
        }
    }
}

/// v2 clients otherwise attach to a shared background service that ignores
/// `OPENCODE_CONFIG` and outlives the launch.
fn standalone_args(args: &[String]) -> Vec<String> {
    const V2_SUBCOMMANDS: &[&str] = &[
        "upgrade", "update", "uninstall", "acp", "api", "debug", "auth", "mcp", "plugin",
        "models", "stats", "mini", "run", "session", "service", "reload", "pair", "serve",
    ];
    const STANDALONE_SUBCOMMANDS: &[&str] = &["run", "mini"];

    if args
        .iter()
        .any(|a| a == "--standalone" || a == "--server" || a.starts_with("--server="))
    {
        return args.to_vec();
    }
    let mut out = args.to_vec();
    match args.first().map(String::as_str) {
        Some(sub) if STANDALONE_SUBCOMMANDS.contains(&sub) => {
            out.insert(1, "--standalone".to_string())
        }
        Some(sub) if V2_SUBCOMMANDS.contains(&sub) => {}
        _ => out.insert(0, "--standalone".to_string()),
    }
    out
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Step 1: ensure we are authenticated
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }

    // Step 1b: ensure an org is selected (handles partial state after aborted login)
    crate::commands::auth::login::ensure_org_selected().await?;

    // Step 2: ensure we have a live api_key for OpenCode. Re-provisions if the
    // cached key was deleted in the console; re-runs onboarding for a fresh key.
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("opencode")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("opencode").await?;
    }
    creds = crate::config::read()?;

    // Step 3: ensure we have a connection choice (default to "plan")
    if creds
        .opencode
        .as_ref()
        .and_then(|c| c.connection.as_deref())
        .is_none()
    {
        let provider = creds.opencode.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    // Step 3b: fetch the org once and derive both the gateway URL and the MCP
    // gate from it, rather than issue a second request later. Done before
    // borrowing `creds.opencode` below since a positive gate re-reads `creds`.
    let org = super::fetch_active_org(&creds).await;
    let gateway_url = super::gateway_base_url_with_org(org.as_ref());
    let mcp_disabled = super::mcp_injection_disabled_with_org(org.as_ref());
    if !mcp_disabled {
        crate::commands::auth::login::ensure_mcp_preference().await?;
        creds = crate::config::read()?;
    }

    // Step 4: build merged config from user's existing opencode.json + edgee provider
    let opencode = creds.opencode.as_ref().unwrap();
    let api_key = &opencode.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();
    util::spawn_cli_version_report(&creds, &session_id);

    // First-run: install the persistent user-level statusline integration
    // exactly once (Claude Code-targeted; honors the disable marker).
    util::ensure_first_run_installed().await;

    let mut config = find_user_config().unwrap_or_else(|| {
        serde_json::json!({
            "$schema": "https://opencode.ai/config.json",
        })
    });

    let (models, catalog) = tokio::join!(
        util::fetch_gateway_models(&gateway_url, api_key),
        util::fetch_model_catalog(&creds)
    );
    let models = util::without_app_subscription_models(models, &catalog);
    let debug_log_headers = util::resolve_debug_log_keypair()?.map(|k| k.header_values());
    let binary = util::resolve_binary("opencode");
    let version = detect_config_version(&binary);
    let (provider_key, edgee_provider) = match version {
        ConfigVersion::V1 => (
            "provider",
            build_edgee_provider(
                api_key,
                &session_id,
                &gateway_url,
                &models,
                &catalog,
                debug_log_headers,
            ),
        ),
        ConfigVersion::V2 => (
            "providers",
            build_edgee_provider_v2(
                api_key,
                &session_id,
                &gateway_url,
                &models,
                &catalog,
                debug_log_headers,
            ),
        ),
    };
    let mut edgee_entry = serde_json::Map::new();
    edgee_entry.insert("edgee".to_string(), edgee_provider);
    plugins::config::merge_object(&mut config, provider_key, Value::Object(edgee_entry));

    // Org plugins. OpenCode's schema exposes `skills.paths`, an `agent` map and
    // an `mcp` map, so all three go into the config the CLI already generates —
    // the user's own opencode.json is read but never written.
    let plugin_report = plugins::sync_for_target(&creds, plugins::Target::Opencode).await;
    if let Some(mcp) = plugins::config::opencode_mcp(&plugin_report.plugins) {
        merge_mcp(&mut config, version, mcp);
    }
    if let Some(agents) = plugins::config::opencode_agents(&plugin_report.plugins) {
        merge_agents(&mut config, version, agents);
    }
    if let Some(skills) = plugin_report.skills_root.as_ref() {
        let skills = skills.to_string_lossy();
        match version {
            ConfigVersion::V1 => plugins::config::push_path(&mut config, "skills", "paths", &skills),
            ConfigVersion::V2 => push_top_level_path(&mut config, "skills", &skills),
        }
    }
    plugins::report_launch(&plugin_report);

    let use_mcp = creds.enable_mcp.unwrap_or(false) && !mcp_disabled;
    let mut instructions_path: Option<std::path::PathBuf> = None;
    if use_mcp {
        let token = creds.user_token.as_deref().unwrap_or("");
        merge_mcp(&mut config, version, build_edgee_mcp(token));

        // v2 never loads `instructions`.
        if version == ConfigVersion::V1 {
            let repo_origin = crate::git::detect_origin();
            let session_url = match creds.org_slug.as_deref() {
                Some(slug) if !slug.is_empty() => {
                    format!(
                        "{}/sessions/{slug}/{session_id}",
                        crate::config::console_base_url()
                    )
                }
                _ => format!(
                    "{}/sessions/{session_id}",
                    crate::config::console_base_url()
                ),
            };
            let text =
                super::mcp::session_instructions(&session_id, repo_origin.as_deref(), &session_url);
            let path =
                std::env::temp_dir().join(format!("edgee-opencode-instructions-{session_id}.md"));
            std::fs::write(&path, &text)?;
            push_top_level_path(&mut config, "instructions", &path.to_string_lossy());
            instructions_path = Some(path);
        }
    }

    let config_content = serde_json::to_string_pretty(&config)?;
    let config_path =
        std::env::temp_dir().join(format!("edgee-opencode-config-{}.json", session_id));
    std::fs::write(&config_path, &config_content)?;

    // Step 5: launch opencode with the correct env vars
    let mut cmd = std::process::Command::new(&binary);
    cmd.env("OPENCODE_CONFIG", &config_path);
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env("EDGEE_ORG_SLUG", creds.org_slug.as_deref().unwrap_or_default());
    match version {
        ConfigVersion::V1 => cmd.args(&opts.args),
        ConfigVersion::V2 => cmd.args(standalone_args(&opts.args)),
    };

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "OpenCode is not installed. Install it from https://opencode.ai"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    // Clean up the temporary config file
    let _ = std::fs::remove_file(&config_path);
    if let Some(path) = instructions_path {
        let _ = std::fs::remove_file(path);
    }

    super::print_session_stats(&creds, &session_id, "OpenCode").await;

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
            serde_json::json!(OPENCODE_OUTPUT_TOKEN_MAX)
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
        // Sonnet 4.5's real rates, in the dollars-per-million unit OpenCode uses.
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

    /// OpenCode's config merge drops `cost.context_over_200k`, so emitting it
    /// would imply a long-context tier that never takes effect.
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
    fn a_free_model_declares_zeroed_rates_rather_than_omitting_them() {
        let provider = provider_priced("zai/glm-4.5-flash", crate::api::GatewayModelCost {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        });
        let cost = &provider["models"]["zai/glm-4.5-flash"]["cost"];
        assert_eq!(cost["input"], serde_json::json!(0.0));
        assert_eq!(cost["output"], serde_json::json!(0.0));
    }

    #[test]
    fn edgee_mcp_uses_the_remote_discriminant() {
        let mcp = build_edgee_mcp("tok");
        assert_eq!(mcp["edgee"]["type"], serde_json::json!("remote"));
        assert_eq!(mcp["edgee"]["enabled"], serde_json::json!(true));
        assert_eq!(
            mcp["edgee"]["headers"]["Authorization"],
            serde_json::json!("Bearer tok")
        );
    }

    #[test]
    fn merging_edgee_mcp_preserves_plugin_mcp_entries() {
        let mut config = serde_json::json!({
            "mcp": { "house__remote": { "type": "remote", "url": "https://example.com/mcp" } }
        });
        plugins::config::merge_object(&mut config, "mcp", build_edgee_mcp("tok"));

        assert_eq!(
            config["mcp"]["house__remote"]["url"],
            serde_json::json!("https://example.com/mcp")
        );
        assert_eq!(config["mcp"]["edgee"]["type"], serde_json::json!("remote"));
    }

    #[test]
    fn push_instructions_path_creates_and_dedupes() {
        let mut config = serde_json::json!({});
        push_top_level_path(&mut config, "instructions", "/tmp/edgee-instructions.md");
        push_top_level_path(&mut config, "instructions", "/tmp/edgee-instructions.md");

        assert_eq!(
            config["instructions"],
            serde_json::json!(["/tmp/edgee-instructions.md"])
        );
    }

    #[test]
    fn push_instructions_path_preserves_existing_entries() {
        let mut config = serde_json::json!({ "instructions": ["CONTRIBUTING.md"] });
        push_top_level_path(&mut config, "instructions", "/tmp/edgee-instructions.md");

        assert_eq!(
            config["instructions"],
            serde_json::json!(["CONTRIBUTING.md", "/tmp/edgee-instructions.md"])
        );
    }

    #[test]
    fn parses_config_version_from_version_output() {
        assert_eq!(parse_config_version("1.18.31\n"), ConfigVersion::V1);
        assert_eq!(parse_config_version("2.0.0"), ConfigVersion::V2);
        assert_eq!(parse_config_version("v2.3.1"), ConfigVersion::V2);
        assert_eq!(parse_config_version("opencode 2.1.0"), ConfigVersion::V2);
        assert_eq!(parse_config_version(""), ConfigVersion::V1);
        assert_eq!(parse_config_version("garbage"), ConfigVersion::V1);
    }

    #[test]
    fn v2_provider_uses_package_settings_and_headers() {
        let provider = build_edgee_provider_v2(
            "key",
            "sess",
            "https://gw.test",
            &[],
            &util::ModelCatalog::default(),
            None,
        );
        assert_eq!(
            provider["package"],
            serde_json::json!("@opencode/ai/providers/openai-compatible")
        );
        assert_eq!(
            provider["settings"]["baseURL"],
            serde_json::json!("https://gw.test/v1")
        );
        assert_eq!(provider["settings"]["apiKey"], serde_json::json!("key"));
        assert_eq!(provider["headers"]["x-edgee-api-key"], serde_json::json!("key"));
        assert_eq!(
            provider["headers"]["x-edgee-session-id"],
            serde_json::json!("sess")
        );
        assert!(provider.get("npm").is_none());
        assert!(provider.get("options").is_none());
    }

    #[test]
    fn v2_models_declare_model_id_capabilities_limit_and_variant_array() {
        let id = "anthropic/claude-opus-5";
        let catalog: util::ModelCatalog = [(
            id.to_string(),
            util::ModelMetadata {
                context: Some(1_000_000),
                input_modalities: vec!["text".to_string(), "image".to_string()],
                reasoning_efforts: vec!["low".to_string(), "high".to_string()],
                cost: Some(crate::api::GatewayModelCost {
                    input: 5.0,
                    output: 25.0,
                    cache_read: 0.5,
                    cache_write: 6.25,
                }),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect();
        let provider = build_edgee_provider_v2(
            "key",
            "sess",
            "https://gw.test",
            &[id.to_string()],
            &catalog,
            None,
        );
        let model = &provider["models"][id];

        assert_eq!(model["modelID"], serde_json::json!(id));
        assert_eq!(
            model["capabilities"],
            serde_json::json!({ "tools": true, "input": ["text", "image"], "output": ["text"] })
        );
        assert_eq!(model["limit"]["context"], serde_json::json!(1_000_000));
        assert_eq!(
            model["variants"],
            serde_json::json!([
                { "id": "low", "settings": { "reasoningEffort": "low" } },
                { "id": "high", "settings": { "reasoningEffort": "high" } },
            ])
        );
        assert_eq!(
            model["cost"],
            serde_json::json!({ "input": 5.0, "output": 25.0, "cache": { "read": 0.5, "write": 6.25 } })
        );
        assert!(model.get("modalities").is_none());
        assert!(model.get("reasoning").is_none());
    }

    #[test]
    fn v2_mcp_merges_under_servers_and_keeps_existing_entries() {
        let mut config = serde_json::json!({
            "mcp": { "servers": { "house__remote": { "type": "remote" } } }
        });
        merge_mcp(&mut config, ConfigVersion::V2, build_edgee_mcp("tok"));

        assert_eq!(
            config["mcp"]["servers"]["house__remote"]["type"],
            serde_json::json!("remote")
        );
        assert_eq!(
            config["mcp"]["servers"]["edgee"]["headers"]["Authorization"],
            serde_json::json!("Bearer tok")
        );
        assert_eq!(
            config["mcp"]["servers"]["edgee"]["disabled"],
            serde_json::json!(false)
        );
        assert!(config["mcp"]["servers"]["edgee"].get("enabled").is_none());
        assert!(config["mcp"].get("edgee").is_none());
    }

    #[test]
    fn v2_agents_move_prompt_to_system() {
        let mut config = serde_json::json!({});
        let agents = serde_json::json!({
            "house__rev": { "description": "d", "prompt": "p", "mode": "subagent" }
        });
        merge_agents(&mut config, ConfigVersion::V2, agents.clone());
        assert_eq!(config["agents"]["house__rev"]["system"], serde_json::json!("p"));
        assert!(config["agents"]["house__rev"].get("prompt").is_none());

        let mut v1 = serde_json::json!({});
        merge_agents(&mut v1, ConfigVersion::V1, agents);
        assert_eq!(v1["agent"]["house__rev"]["prompt"], serde_json::json!("p"));
    }

    #[test]
    fn standalone_is_injected_for_tui_and_model_subcommands_only() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        assert_eq!(standalone_args(&[]), args(&["--standalone"]));
        assert_eq!(
            standalone_args(&args(&["./repo", "-c"])),
            args(&["--standalone", "./repo", "-c"])
        );
        assert_eq!(
            standalone_args(&args(&["run", "-m", "edgee/x", "hi"])),
            args(&["run", "--standalone", "-m", "edgee/x", "hi"])
        );
        assert_eq!(standalone_args(&args(&["debug", "config"])), args(&["debug", "config"]));
        assert_eq!(
            standalone_args(&args(&["--server", "http://x"])),
            args(&["--server", "http://x"])
        );
    }

    #[test]
    fn v1_mcp_merges_at_top_level() {
        let mut config = serde_json::json!({});
        merge_mcp(&mut config, ConfigVersion::V1, build_edgee_mcp("tok"));
        assert_eq!(config["mcp"]["edgee"]["type"], serde_json::json!("remote"));
    }
}
