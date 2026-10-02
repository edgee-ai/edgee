use std::io::Write;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::util;

const DEFAULT_MODEL: &str = "deepseek/deepseek-v4-pro";

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

fn pick_model(value: Option<&str>) -> &str {
    value.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_MODEL)
}

fn model_entry(id: &str, catalog: &util::ModelCatalog) -> Value {
    let mut entry = json!({"id": id, "name": id});
    if let Some(metadata) = catalog.get(id) {
        if let Some(context) = metadata.context {
            entry["contextWindow"] = json!(context);
        }
        let input: Vec<_> = metadata.input_modalities.iter()
            .filter(|modality| matches!(modality.as_str(), "text" | "image"))
            .collect();
        if !input.is_empty() {
            entry["input"] = json!(input);
        }
        let mut efforts = serde_json::Map::new();
        for effort in &metadata.reasoning_efforts {
            let level = match effort.as_str() {
                "none" | "off" => "off",
                "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => effort,
                _ => continue,
            };
            efforts.insert(level.to_string(), json!(effort));
        }
        if !efforts.is_empty() {
            entry["reasoningEfforts"] = Value::Object(efforts);
        }
    } else if id.starts_with("deepseek/") {
        entry["reasoningEfforts"] = json!({"off": null, "high": "high", "max": "max"});
    }
    if id.starts_with("deepseek/") {
        entry["compat"] = json!({"thinkingFormat": "deepseek"});
    }
    entry
}

fn build_patch(
    gateway_url: &str,
    model: &str,
    headers: Value,
    models: &[String],
    catalog: &util::ModelCatalog,
) -> Value {
    let mut entries = Vec::new();
    for id in models.iter().map(String::as_str).chain(std::iter::once(model)) {
        if !entries.iter().any(|entry: &Value| entry["id"] == id) {
            entries.push(model_entry(id, catalog));
        }
    }
    json!([
        {
            "id": "llm-pi-ai",
            "config": {
                "providers": {
                    "edgee": {
                        "displayName": "Edgee",
                        "apiKeyEnv": "EDGEE_API_KEY",
                        "api": "openai-completions",
                        "baseURL": format!("{}/v1", gateway_url.trim_end_matches('/')),
                        "headers": headers,
                        "compat": {
                            "supportsDeveloperRole": false,
                            "maxTokensField": "max_tokens"
                        },
                        "models": entries
                    }
                }
            }
        },
        {"id": "agent-default-model", "config": {"provider": "edgee", "model": model}}
    ])
}

fn launch_args(args: &[String], patch_path: &Path) -> Vec<String> {
    // Plugin management does not boot a profile or accept patch overlays.
    if args.first().is_some_and(|arg| arg == "plugin") {
        return args.to_vec();
    }
    let patch = format!("--patch={}", patch_path.display());
    if args.first().is_some_and(|arg| !arg.starts_with('-')) {
        let mut result = vec![args[0].clone(), patch];
        result.extend_from_slice(&args[1..]);
        return result;
    }
    let mut result = vec![patch];
    if !args.iter().any(|arg| arg == "--profile" || arg.starts_with("--profile=")) {
        result.push("--profile=web".to_string());
    }
    result.extend_from_slice(args);
    result
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }
    crate::commands::auth::login::ensure_org_selected().await?;
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("deepseek")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("deepseek").await?;
    }
    creds = crate::config::read()?;
    let api_key = creds.provider_api_key("deepseek")
        .context("DeepSeek key missing. Retry `edgee launch deepseek` to provision it.")?;
    let session_id = uuid::Uuid::new_v4().to_string();
    util::ensure_first_run_installed().await;
    util::spawn_cli_version_report(&creds, &session_id);

    let gateway_url = super::resolve_gateway_base_url(&creds).await;
    let (models, catalog) = tokio::join!(
        util::fetch_gateway_models(gateway_url.trim_end_matches('/'), api_key),
        util::fetch_model_catalog(&creds)
    );
    let models = util::without_app_subscription_models(models, &catalog);
    let model_override = std::env::var("EDGEE_DEEPSEEK_MODEL").ok();
    let model = pick_model(model_override.as_deref());
    let mut headers = json!({
        "x-edgee-api-key": api_key,
        "x-edgee-session-id": session_id,
    });
    if let Some(repo) = crate::git::detect_origin() {
        headers["x-edgee-repo"] = json!(repo);
    }
    if let Some(keypair) = util::resolve_debug_log_keypair()? {
        let debug = keypair.header_values();
        headers["x-edgee-debug-pubkey"] = json!(debug.pubkey);
        headers["x-edgee-debug-salt"] = json!(debug.salt);
    }

    // NamedTempFile restricts permissions and stays alive until the harness exits.
    let mut patch = tempfile::Builder::new().prefix("edgee-deepseek-").suffix(".yml").tempfile()
        .context("Could not create DeepSeek overlay. Check your temporary directory permissions.")?;
    patch.write_all(serde_yaml::to_string(&build_patch(&gateway_url, model, headers, &models, &catalog))?.as_bytes())?;
    patch.flush()?;
    let mut cmd = Command::new(util::resolve_binary("dsh"));
    cmd.env("EDGEE_API_KEY", api_key)
        .env("EDGEE_SESSION_ID", &session_id)
        .env("EDGEE_ORG_SLUG", creds.org_slug.as_deref().unwrap_or_default())
        .args(launch_args(&opts.args, patch.path()));
    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("DeepSeek Harness is not installed. Install it with `npm install -g @deepseek-ai/dsh`.")
        } else {
            anyhow::anyhow!(e).context("Could not launch DeepSeek Harness. Check your dsh installation.")
        }
    })?;
    drop(patch);
    super::print_session_stats(&creds, &session_id, "DeepSeek Harness").await;
    if let Some(code) = status.code() {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_override_and_blank_fallback() {
        assert_eq!(pick_model(None), DEFAULT_MODEL);
        assert_eq!(pick_model(Some("  ")), DEFAULT_MODEL);
        assert_eq!(pick_model(Some(" deepseek/deepseek-v4-flash ")), "deepseek/deepseek-v4-flash");
    }

    #[test]
    fn overlay_routes_requests_and_default_selection() {
        let patch = build_patch("https://gateway.example/", DEFAULT_MODEL, json!({"x-edgee-session-id": "session"}), &[], &util::ModelCatalog::new());
        let provider = &patch[0]["config"]["providers"]["edgee"];
        assert_eq!(provider["baseURL"], "https://gateway.example/v1");
        assert_eq!(provider["api"], "openai-completions");
        assert_eq!(provider["apiKeyEnv"], "EDGEE_API_KEY");
        assert_eq!(provider["headers"]["x-edgee-session-id"], "session");
        assert_eq!(provider["models"][0]["id"], DEFAULT_MODEL);
        assert_eq!(patch[1]["config"]["provider"], "edgee");
        let yaml = serde_yaml::to_string(&patch).unwrap();
        let roundtrip: Value = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(roundtrip, patch);
    }

    #[test]
    fn picker_contains_gateway_models_and_default_without_duplicates() {
        let models = vec!["openai/gpt-5".to_string(), DEFAULT_MODEL.to_string(), "openai/gpt-5".to_string()];
        let patch = build_patch("https://gateway.example", DEFAULT_MODEL, json!({}), &models, &util::ModelCatalog::new());
        let entries = patch[0]["config"]["providers"]["edgee"]["models"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["id"], "openai/gpt-5");
        assert_eq!(entries[1]["id"], DEFAULT_MODEL);
        assert!(entries[0].get("compat").is_none());
        assert_eq!(entries[1]["compat"]["thinkingFormat"], "deepseek");
        assert_eq!(patch[1]["config"]["model"], DEFAULT_MODEL);
    }

    #[test]
    fn override_remains_selectable_when_missing_from_listing() {
        let patch = build_patch("https://gateway.example", "custom/model", json!({}), &[DEFAULT_MODEL.to_string()], &util::ModelCatalog::new());
        let entries = patch[0]["config"]["providers"]["edgee"]["models"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1]["id"], "custom/model");
        assert_eq!(patch[1]["config"]["model"], "custom/model");
    }

    #[test]
    fn picker_uses_catalog_capabilities_and_supported_effort_names() {
        let catalog = util::ModelCatalog::from([
            ("openai/gpt-5".to_string(), util::ModelMetadata {
                context: Some(400_000),
                input_modalities: vec!["text".to_string(), "image".to_string(), "audio".to_string()],
                reasoning_efforts: vec!["none".to_string(), "low".to_string(), "xhigh".to_string(), "unsupported".to_string()],
                ..Default::default()
            }),
            (DEFAULT_MODEL.to_string(), util::ModelMetadata {
                reasoning_efforts: vec!["high".to_string()],
                ..Default::default()
            })
        ]);
        let entry = model_entry("openai/gpt-5", &catalog);
        assert_eq!(entry["contextWindow"], 400_000);
        assert_eq!(entry["input"], json!(["text", "image"]));
        assert_eq!(entry["reasoningEfforts"], json!({"off": "none", "low": "low", "xhigh": "xhigh"}));
        assert!(entry.get("maxTokens").is_none());
        assert_eq!(model_entry(DEFAULT_MODEL, &catalog)["reasoningEfforts"], json!({"high": "high"}));
        assert!(model_entry("unknown/model", &catalog).get("reasoningEfforts").is_none());
    }

    #[test]
    fn args_preserve_profiles_and_agent_flags() {
        let path = Path::new("/tmp/edgee patch.yml");
        for (args, expected) in [
            (vec![], vec!["--patch=/tmp/edgee patch.yml", "--profile=web"]),
            (vec!["web", "--no-open"], vec!["web", "--patch=/tmp/edgee patch.yml", "--no-open"]),
            (vec!["headless", "task", "--help"], vec!["headless", "--patch=/tmp/edgee patch.yml", "task", "--help"]),
            (vec!["--profile", "custom", "--help"], vec!["--patch=/tmp/edgee patch.yml", "--profile", "custom", "--help"]),
            (vec!["--profile=custom", "-p", "prompt"], vec!["--patch=/tmp/edgee patch.yml", "--profile=custom", "-p", "prompt"]),
            (vec!["plugin", "--profile", "web", "add", "package"], vec!["plugin", "--profile", "web", "add", "package"]),
        ] {
            assert_eq!(launch_args(&args.into_iter().map(String::from).collect::<Vec<_>>(), path), expected);
        }
    }
}
