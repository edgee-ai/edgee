use anyhow::Result;
use serde_json::Value;

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

fn build_edgee_provider(api_key: &str, session_id: &str, gateway_url: &str) -> Value {
    serde_json::json!({
        "npm": "@ai-sdk/openai-compatible",
        "name": "Edgee",
        "options": {
            "baseURL": format!("{}/v1", gateway_url),
            "apiKey": api_key,
            "headers": {
                "x-edgee-api-key": api_key,
                "x-edgee-session-id": session_id,
            }
        },
        "models": {
            "anthropic/claude-haiku-4-5": {
                "name": "Claude Haiku 4.5 (via Edgee)"
            },
            "anthropic/claude-opus-4-6": {
                "name": "Claude Opus 4.6 (via Edgee)"
            },
            "anthropic/claude-sonnet-4-6": {
                "name": "Claude Sonnet 4.6 (via Edgee)"
            },
            "openai/gpt-5.4": {
                "name": "GPT-5.4 (via Edgee)"
            },
            "openai/gpt-5.3-codex": {
                "name": "GPT-5.3 Codex (via Edgee)"
            },
        }
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
    creds = crate::config::read()?;

    // Step 2: ensure we have an api_key for OpenCode
    if creds
        .opencode
        .as_ref()
        .map(|c| c.api_key.is_empty())
        .unwrap_or(true)
    {
        crate::commands::auth::login::ensure_provider_key("opencode").await?;
        creds = crate::config::read()?;
    }

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
    let gateway_url = crate::config::gateway_base_url();

    let mut config = find_user_config().unwrap_or_else(|| {
        serde_json::json!({
            "$schema": "https://opencode.ai/config.json",
        })
    });

    let edgee_provider = build_edgee_provider(api_key, &session_id, &gateway_url);

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
    let mut cmd = std::process::Command::new(crate::commands::launch::resolve_binary("opencode"));
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

    crate::commands::launch::print_session_stats(&creds, &session_id, "OpenCode").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}
