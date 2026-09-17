//! `edgee launch hermes` — Hermes Agent CLI with Edgee as a custom provider.
//!
//! Hermes exposes named custom providers under `providers:` in
//! `$HERMES_HOME/config.yaml`. Registering `providers.edgee` is additive: it
//! leaves the user's selected provider, credentials, history, skills, hooks,
//! MCP servers, and every unrelated config key untouched. Launch pins only the
//! child process to `--provider=edgee`.
//!
//! ## Why this writes the user's real config
//!
//! Hermes has no config-file-only override. `HERMES_HOME` relocates the whole
//! profile, so pointing it at a temporary directory would hide user state.
//! The provider block is therefore persisted under Edgee's own key. Its
//! credential is referenced through `EDGEE_API_KEY` and supplied only to the
//! child process; no API key is written to disk.
//!
//! ## Wire shape
//!
//! Hermes uses OpenAI Chat Completions for named custom providers, so the
//! gateway URL includes `/v1`. Tool-name casing still needs live wire
//! verification before the API assigns a Hermes-specific trimming flavor.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_yaml::{Mapping, Value};

use super::util;
use crate::commands::util::plugins;

const PROVIDER_KEY: &str = "edgee";
const API_KEY_ENV: &str = "EDGEE_API_KEY";

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the Hermes CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn process_hermes_home() -> Option<PathBuf> {
    std::env::var_os("HERMES_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

fn default_hermes_root(process_home: Option<&Path>) -> Option<PathBuf> {
    if let Some(home) = process_home {
        return Some(
            if home.parent().is_some_and(|parent| {
                parent.file_name().is_some_and(|name| name == "profiles")
            }) {
                home.parent()?.parent()?.to_path_buf()
            } else {
                home.to_path_buf()
            },
        );
    }

    #[cfg(windows)]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(local_app_data).join("hermes"));
        }
        return home_dir().map(|home| home.join("AppData").join("Local").join("hermes"));
    }

    #[cfg(not(windows))]
    home_dir().map(|home| home.join(".hermes"))
}

fn inside_mcp_add_args(args: &[String], index: usize) -> bool {
    let before = &args[..index];
    let Some(mcp) = before.iter().position(|arg| arg == "mcp") else {
        return false;
    };
    before[mcp + 1..].iter().any(|arg| arg == "add")
}

/// Mirrors Hermes' pre-argparse profile scan closely enough to write the
/// provider into the same profile the child will load. Value-taking flags are
/// skipped so a prompt containing `-p` is never mistaken for a profile.
fn explicit_profile(args: &[String]) -> Option<String> {
    const VALUE_FLAGS: &[&str] = &[
        "-z",
        "--oneshot",
        "-m",
        "--model",
        "--provider",
        "--reasoning",
        "-t",
        "--toolsets",
        "-r",
        "--resume",
        "-s",
        "--skills",
        "--usage-file",
        "--in",
    ];
    const OPTIONAL_VALUE_FLAGS: &[&str] = &["-c", "--continue"];

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" || (arg == "--args" && inside_mcp_add_args(args, index)) {
            break;
        }
        if matches!(arg.as_str(), "-p" | "--profile") && index + 1 < args.len() {
            return Some(args[index + 1].trim().to_lowercase());
        }
        if let Some(profile) = arg.strip_prefix("--profile=") {
            return Some(profile.trim().to_lowercase());
        }

        let takes_value = !arg.contains('=')
            && index + 1 < args.len()
            && (VALUE_FLAGS.contains(&arg.as_str())
                || (OPTIONAL_VALUE_FLAGS.contains(&arg.as_str())
                    && !args[index + 1].starts_with('-')));
        index += if takes_value { 2 } else { 1 };
    }
    None
}

fn valid_profile_name(profile: &str) -> bool {
    let mut chars = profile.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit())
        && profile.len() <= 64
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '_' | '-'))
}

fn selected_hermes_home(
    root: &Path,
    process_home: Option<&Path>,
    active_profile: Option<&str>,
    explicit_profile: Option<&str>,
) -> Result<PathBuf> {
    if let Some(profile) = explicit_profile {
        if !valid_profile_name(profile) {
            anyhow::bail!(
                "Invalid Hermes profile `{profile}`. Run `hermes profile list` to see your profiles"
            );
        }
        if profile == "default" {
            return Ok(root.to_path_buf());
        }
        let home = root.join("profiles").join(profile);
        if !home.is_dir() {
            anyhow::bail!(
                "Hermes profile `{profile}` does not exist. Run `hermes profile list` to see your profiles"
            );
        }
        return Ok(home);
    }

    if let Some(home) = process_home.filter(|home| {
        home.parent()
            .is_some_and(|parent| parent.file_name().is_some_and(|name| name == "profiles"))
    }) {
        return Ok(home.to_path_buf());
    }

    if let Some(profile) =
        active_profile.filter(|profile| !profile.is_empty() && *profile != "default")
    {
        if !valid_profile_name(profile) {
            anyhow::bail!(
                "Invalid active Hermes profile `{profile}`. Run `hermes profile use default` to repair it"
            );
        }
        let home = root.join("profiles").join(profile);
        if !home.is_dir() {
            anyhow::bail!(
                "Hermes profile `{profile}` does not exist. Run `hermes profile list` to repair your active profile"
            );
        }
        return Ok(home);
    }

    Ok(root.to_path_buf())
}

fn hermes_config_path(args: &[String]) -> Result<PathBuf> {
    let process_home = process_hermes_home();
    let root = default_hermes_root(process_home.as_deref())
        .context("Could not determine Hermes home directory")?;
    let active_profile = std::fs::read_to_string(root.join("active_profile"))
        .ok()
        .map(|profile| profile.trim().to_string());
    let profile = explicit_profile(args);
    Ok(selected_hermes_home(
        &root,
        process_home.as_deref(),
        active_profile.as_deref(),
        profile.as_deref(),
    )?
    .join("config.yaml"))
}

fn build_edgee_provider(
    gateway_url: &str,
    session_id: &str,
    repo: Option<&str>,
    debug_log_headers: Option<crate::crypto::DebugLogHeaderValues>,
) -> Value {
    let mut headers = Mapping::new();
    headers.insert(
        Value::String("x-edgee-session-id".to_string()),
        Value::String(session_id.to_string()),
    );
    if let Some(repo) = repo {
        headers.insert(
            Value::String("x-edgee-repo".to_string()),
            Value::String(repo.to_string()),
        );
    }
    if let Some(debug_headers) = debug_log_headers {
        headers.insert(
            Value::String("x-edgee-debug-pubkey".to_string()),
            Value::String(debug_headers.pubkey),
        );
        headers.insert(
            Value::String("x-edgee-debug-salt".to_string()),
            Value::String(debug_headers.salt),
        );
    }

    let mut provider = Mapping::new();
    provider.insert(
        Value::String("name".to_string()),
        Value::String("Edgee".to_string()),
    );
    provider.insert(
        Value::String("api".to_string()),
        Value::String(format!("{}/v1", gateway_url.trim_end_matches('/'))),
    );
    provider.insert(
        Value::String("key_env".to_string()),
        Value::String(API_KEY_ENV.to_string()),
    );
    provider.insert(
        Value::String("transport".to_string()),
        Value::String("chat_completions".to_string()),
    );
    provider.insert(
        Value::String("discover_models".to_string()),
        Value::Bool(true),
    );
    provider.insert(
        Value::String("extra_headers".to_string()),
        Value::Mapping(headers),
    );
    Value::Mapping(provider)
}

fn write_edgee_provider(path: &Path, provider: Value) -> Result<Option<String>> {
    let mut config = if path.is_file() {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read Hermes config {}", path.display()))?;
        let parsed: Value = serde_yaml::from_str(&content)
            .with_context(|| format!("Hermes config is invalid YAML: {}", path.display()))?;
        if parsed.is_null() {
            Value::Mapping(Mapping::new())
        } else {
            parsed
        }
    } else {
        Value::Mapping(Mapping::new())
    };

    let root = config.as_mapping_mut().with_context(|| {
        format!(
            "Hermes config root must be a mapping: {}",
            path.display()
        )
    })?;
    let model_value = root.get(Value::String("model".to_string()));
    let configured_model = model_value
        .and_then(|model| {
            model.as_str().or_else(|| {
                model
                    .as_mapping()
                    .and_then(|model| model.get(Value::String("default".to_string())))
                    .and_then(Value::as_str)
            })
        })
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string);
    let providers_key = Value::String("providers".to_string());
    if !root.contains_key(&providers_key) || root[&providers_key].is_null() {
        root.insert(providers_key.clone(), Value::Mapping(Mapping::new()));
    }
    let providers = root
        .get_mut(&providers_key)
        .and_then(Value::as_mapping_mut)
        .with_context(|| {
            format!(
                "Hermes config `providers` must be a mapping: {}",
                path.display()
            )
        })?;
    providers.insert(Value::String(PROVIDER_KEY.to_string()), provider);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let rendered = serde_yaml::to_string(&config)?;
    if std::fs::read_to_string(path).ok().as_deref() != Some(rendered.as_str()) {
        std::fs::write(path, rendered)
            .with_context(|| format!("Failed to write Hermes config {}", path.display()))?;
    }
    Ok(configured_model)
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }
    crate::commands::auth::login::ensure_org_selected().await?;

    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("hermes")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("hermes").await?;
    }
    creds = crate::config::read()?;

    if creds
        .hermes
        .as_ref()
        .and_then(|provider| provider.connection.as_deref())
        .is_none()
    {
        let provider = creds.hermes.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    let api_key = creds
        .hermes
        .as_ref()
        .context("Hermes API key was not provisioned")?
        .api_key
        .clone();
    let session_id = uuid::Uuid::new_v4().to_string();
    util::spawn_cli_version_report(&creds, &session_id);
    util::ensure_first_run_installed().await;

    let gateway_url = super::resolve_gateway_base_url(&creds).await;
    let repo = crate::git::detect_origin();
    let debug_log_headers = util::resolve_debug_log_keypair()?.map(|key| key.header_values());
    let provider = build_edgee_provider(
        &gateway_url,
        &session_id,
        repo.as_deref(),
        debug_log_headers,
    );
    let configured_model = write_edgee_provider(&hermes_config_path(&opts.args)?, provider)?;

    let plugin_report = plugins::sync_for_target(&creds, plugins::Target::Hermes).await;
    plugins::report_launch(&plugin_report);

    let mut cmd = std::process::Command::new(util::resolve_binary("hermes"));
    cmd.env(API_KEY_ENV, api_key);
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env(
        "EDGEE_ORG_SLUG",
        creds.org_slug.as_deref().unwrap_or_default(),
    );
    if std::env::var_os("HERMES_INFERENCE_MODEL")
        .filter(|model| !model.is_empty())
        .is_none()
    {
        if let Some(model) = configured_model {
            // Hermes one-shot mode rejects --provider without a model. Reuse the
            // profile's configured default while preserving explicit -m/--model.
            cmd.env("HERMES_INFERENCE_MODEL", model);
        }
    }
    cmd.arg("--provider=edgee");
    cmd.args(&opts.args);

    let status = cmd.status().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            #[cfg(windows)]
            let install = "irm https://hermes-agent.nousresearch.com/install.ps1 | iex";
            #[cfg(not(windows))]
            let install = "curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash";
            anyhow::anyhow!("Hermes Agent is not installed. Install it with `{install}`")
        } else {
            anyhow::Error::new(error).context("Failed to launch Hermes Agent")
        }
    })?;

    super::print_session_stats(&creds, &session_id, "Hermes Agent").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_uses_edgee_without_persisting_api_key() {
        let provider = build_edgee_provider(
            "https://gateway.example/",
            "session-123",
            Some("git@example.com:org/repo.git"),
            None,
        );

        assert_eq!(provider["api"], "https://gateway.example/v1");
        assert_eq!(provider["key_env"], API_KEY_ENV);
        assert_eq!(provider["transport"], "chat_completions");
        assert_eq!(
            provider["extra_headers"]["x-edgee-session-id"],
            "session-123"
        );
        assert_eq!(
            provider["extra_headers"]["x-edgee-repo"],
            "git@example.com:org/repo.git"
        );
        assert!(provider.get("api_key").is_none());
    }

    #[test]
    fn provider_write_preserves_unrelated_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "model:\n  provider: anthropic\nproviders:\n  local:\n    api: http://localhost:1234/v1\n",
        )
        .unwrap();

        let configured_model = write_edgee_provider(
            &path,
            build_edgee_provider("https://gateway.example", "session", None, None),
        )
        .unwrap();

        let config: Value =
            serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(config["model"]["provider"], "anthropic");
        assert_eq!(configured_model.as_deref(), None);
        assert_eq!(
            config["providers"]["local"]["api"],
            "http://localhost:1234/v1"
        );
        assert_eq!(
            config["providers"][PROVIDER_KEY]["api"],
            "https://gateway.example/v1"
        );
    }

    #[test]
    fn provider_write_accepts_empty_config_and_null_providers() {
        for initial in ["", "providers:\n"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("config.yaml");
            std::fs::write(&path, initial).unwrap();

            write_edgee_provider(
                &path,
                build_edgee_provider("https://gateway.example", "session", None, None),
            )
            .unwrap();

            let config: Value =
                serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            assert_eq!(
                config["providers"][PROVIDER_KEY]["api"],
                "https://gateway.example/v1"
            );
        }
    }

    #[test]
    fn profile_scan_matches_hermes_preparser_boundaries() {
        assert_eq!(
            explicit_profile(&["--profile".into(), "Work".into()]),
            Some("work".into())
        );
        assert_eq!(
            explicit_profile(&["chat".into(), "--profile=Review".into()]),
            Some("review".into())
        );
        assert_eq!(
            explicit_profile(&["-z".into(), "prompt with -p work".into()]),
            None
        );
        assert_eq!(
            explicit_profile(&[
                "mcp".into(),
                "add".into(),
                "--args".into(),
                "tool".into(),
                "-p".into(),
                "child".into(),
            ]),
            None
        );
    }

    #[test]
    fn profile_resolution_honors_explicit_and_active_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("hermes");
        let work = root.join("profiles").join("work");
        std::fs::create_dir_all(&work).unwrap();

        assert_eq!(
            selected_hermes_home(&root, None, Some("work"), None).unwrap(),
            work
        );
        assert_eq!(
            selected_hermes_home(&root, None, Some("work"), Some("default")).unwrap(),
            root
        );
        assert!(selected_hermes_home(&root, None, None, Some("Bad Profile")).is_err());
    }
}
