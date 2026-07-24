use anyhow::Result;
use serde_json::Value;

use super::util;

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

#[derive(serde::Deserialize)]
struct GatewayModelList {
    #[serde(default)]
    data: Vec<GatewayModelEntry>,
}

#[derive(serde::Deserialize)]
struct GatewayModelEntry {
    id: String,
}

/// Fetches the gateway's OpenAI-style `/v1/models` listing so the OpenCode
/// provider config can be populated with a concrete `models` map. The endpoint
/// is public today; the api key is sent anyway to stay correct if it ever
/// starts requiring auth. Returns an empty vec on any failure so launch falls
/// back to a provider with no explicit `models` map.
async fn fetch_gateway_models(gateway_url: &str, api_key: &str) -> Vec<String> {
    let url = format!("{}/v1/models", gateway_url);
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let resp = match client.get(&url).header("x-edgee-api-key", api_key).send().await {
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
            models_map.insert(id.clone(), serde_json::json!({ "name": id }));
        }
        provider["models"] = Value::Object(models_map);
    }

    provider
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

    // Step 4: build merged config from user's existing opencode.json + edgee provider
    let opencode = creds.opencode.as_ref().unwrap();
    let api_key = &opencode.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();
    util::spawn_cli_version_report(&creds, &session_id);

    // First-run: install the persistent user-level statusline integration
    // exactly once (Claude Code-targeted; honors the disable marker).
    util::ensure_first_run_installed().await;

    let gateway_url = super::resolve_gateway_base_url(&creds).await;

    let mut config = find_user_config().unwrap_or_else(|| {
        serde_json::json!({
            "$schema": "https://opencode.ai/config.json",
        })
    });

    let models = fetch_gateway_models(&gateway_url, api_key).await;
    let debug_log_headers = util::resolve_debug_log_keypair()?.map(|k| k.header_values());
    let edgee_provider =
        build_edgee_provider(api_key, &session_id, &gateway_url, &models, debug_log_headers);

    if let Some(obj) = config.as_object_mut() {
        if let Some(providers) = obj.get_mut("provider") {
            if let Some(providers_obj) = providers.as_object_mut() {
                providers_obj.insert("edgee".to_string(), edgee_provider);
            } else {
                let mut providers_map = serde_json::Map::new();
                providers_map.insert("edgee".to_string(), edgee_provider);
                obj.insert("provider".to_string(), Value::Object(providers_map));
            }
        } else {
            let mut providers_map = serde_json::Map::new();
            providers_map.insert("edgee".to_string(), edgee_provider);
            obj.insert("provider".to_string(), Value::Object(providers_map));
        }
    }

    let config_content = serde_json::to_string_pretty(&config)?;
    let config_path =
        std::env::temp_dir().join(format!("edgee-opencode-config-{}.json", session_id));
    std::fs::write(&config_path, &config_content)?;

    // Step 5: launch opencode with the correct env vars
    let mut cmd = std::process::Command::new(util::resolve_binary("opencode"));
    cmd.env("OPENCODE_CONFIG", &config_path);
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.args(&opts.args);

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

    super::print_session_stats(&creds, &session_id, "OpenCode").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}
