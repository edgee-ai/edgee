//! `edgee launch kimi`: Kimi Code CLI (<https://moonshotai.github.io/kimi-code/>).
//!
//! ## Provider: env only
//!
//! Kimi Code ignores provider credentials from the shell (`KIMI_API_KEY` does
//! nothing), except for the `KIMI_MODEL_*` family: setting `KIMI_MODEL_NAME`
//! synthesizes an in-memory provider and model alias that override
//! `config.toml` and vanish on exit. `KIMI_MODEL_PROVIDER_TYPE=anthropic` takes
//! a base URL without `/v1`, like `ANTHROPIC_BASE_URL` in `claude.rs`.
//!
//! Session attribution rides on `KIMI_CODE_CUSTOM_HEADERS` (0.20.2+, one
//! `Name: Value` per line, same shape as `ANTHROPIC_CUSTOM_HEADERS`). It is
//! absent from the docs site, so if a release drops it, fall back to an
//! additive `[providers.edgee]` block with `custom_headers` in `config.toml`.
//! The key still goes in `KIMI_MODEL_API_KEY`: the gateway rejects
//! `x-edgee-api-key` on its own.
//!
//! ## MCP and nudge: an Edgee-owned plugin
//!
//! Kimi has no per-launch MCP or prompt flag (2.0 removed `kimi mcp add`, and
//! the TUI ignores `--agent-file`), so this target installs a plugin under
//! `$KIMI_CODE_HOME/plugins/managed/edgee/`, registered in Kimi's undocumented
//! `plugins/installed.json` (only the `edgee` record is touched, and its
//! `enabled` flag is preserved). It carries:
//!
//! - the `edgee` MCP server, token read from [`MCP_TOKEN_ENV`] via
//!   `bearerTokenEnvVar`, so no secret lands on disk;
//! - a `systemPrompt` with this session's instructions. Kimi freezes it at
//!   session startup, so it is rewritten per launch and cleared on exit.
//!   Two launches racing through startup can still swap session ids.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::util;

/// Model id handed to `KIMI_MODEL_NAME`, and therefore the single entry point
/// the session runs on. One model is enough by design: the gateway's routing
/// engine decides what actually serves each request, so this only has to name a
/// sensible default from the catalog.
const DEFAULT_MODEL: &str = "moonshotai/kimi-k2.7-code";

/// Escape hatch for the model above.
const MODEL_ENV_OVERRIDE: &str = "EDGEE_KIMI_MODEL";

/// Env var the plugin's MCP entry reads its bearer token from.
const MCP_TOKEN_ENV: &str = "EDGEE_KIMI_MCP_TOKEN";

/// Plugin id, directory name under `plugins/managed/`, and manifest `name`.
const PLUGIN_ID: &str = "edgee";

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the kimi CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// Picks the model id from an override value, falling back to [`DEFAULT_MODEL`].
///
/// A blank export reads as "not set": an empty `KIMI_MODEL_NAME` would make kimi
/// fail at startup, and that is a worse answer than the default.
fn pick_model(override_value: Option<&str>) -> &str {
    override_value
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or(DEFAULT_MODEL)
}

/// The model id this launch pins, honoring [`MODEL_ENV_OVERRIDE`].
fn resolve_model() -> String {
    let raw = std::env::var(MODEL_ENV_OVERRIDE).ok();
    pick_model(raw.as_deref()).to_string()
}

/// The plugin's `kimi.plugin.json`. Rewritten on every launch, since the MCP
/// URL follows the active profile and the prompt carries the session id.
fn plugin_manifest(mcp_url: &str, system_prompt: Option<&str>) -> Value {
    let mut manifest = serde_json::json!({
        "name": PLUGIN_ID,
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Edgee session tracking for Kimi Code, installed by `edgee launch kimi`",
        "interface": { "displayName": "Edgee" },
        "mcpServers": {
            "edgee": { "url": mcp_url, "bearerTokenEnvVar": MCP_TOKEN_ENV }
        },
    });
    if let Some(prompt) = system_prompt {
        manifest["systemPrompt"] = Value::String(prompt.to_string());
    }
    manifest
}

/// Upserts the `edgee` record in `installed.json`, preserving every other
/// plugin. An existing record keeps its `enabled` flag and `installedAt`.
/// Bails rather than clobbering a file Kimi itself would refuse to load.
fn register_plugin(installed_path: &Path, root: &Path, now: &str) -> Result<()> {
    let content = match std::fs::read_to_string(installed_path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e).with_context(|| format!("Failed to read {}", installed_path.display()))
        }
    };
    let invalid = || {
        anyhow::anyhow!(
            "{} is not a valid Kimi plugin registry.\nFix it (or run `/plugins reload` in kimi to see the error), then launch kimi again.",
            installed_path.display()
        )
    };
    let mut file: Value = if content.trim().is_empty() {
        serde_json::json!({ "version": 1, "plugins": [] })
    } else {
        serde_json::from_str(&content).map_err(|_| invalid())?
    };
    let plugins = file
        .get_mut("plugins")
        .and_then(Value::as_array_mut)
        .ok_or_else(invalid)?;

    let root = root.to_string_lossy();
    let existing = plugins.iter().position(|p| p["id"] == PLUGIN_ID);
    let previous = existing.map(|i| plugins.remove(i));
    let enabled = previous
        .as_ref()
        .and_then(|p| p["enabled"].as_bool())
        .unwrap_or(true);
    let installed_at = previous
        .as_ref()
        .and_then(|p| p["installedAt"].as_str())
        .unwrap_or(now)
        .to_string();
    let record = serde_json::json!({
        "id": PLUGIN_ID,
        "root": root,
        "source": "local-path",
        "enabled": enabled,
        "installedAt": installed_at,
        "updatedAt": now,
        "originalSource": root,
    });
    plugins.insert(existing.unwrap_or(plugins.len()), record);

    std::fs::write(
        installed_path,
        format!("{}\n", serde_json::to_string_pretty(&file)?),
    )
    .with_context(|| format!("Failed to write {}", installed_path.display()))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// Kimi Code's config directory, honoring `KIMI_CODE_HOME` (tilde-expanded,
/// as Kimi Code itself does) over the default `~/.kimi-code`.
pub(crate) fn kimi_code_home() -> Option<PathBuf> {
    if let Some(dir) = std::env::var("KIMI_CODE_HOME").ok().filter(|d| !d.is_empty()) {
        if dir == "~" {
            return home_dir();
        }
        if let Some(rest) = dir.strip_prefix("~/") {
            return home_dir().map(|h| h.join(rest));
        }
        return Some(PathBuf::from(dir));
    }
    home_dir().map(|h| h.join(".kimi-code"))
}

fn plugin_root(kimi_home: &Path) -> PathBuf {
    kimi_home.join("plugins").join("managed").join(PLUGIN_ID)
}

fn write_manifest(root: &Path, mcp_url: &str, system_prompt: Option<&str>) -> Result<()> {
    let path = root.join("kimi.plugin.json");
    let manifest = plugin_manifest(mcp_url, system_prompt);
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&manifest)?),
    )
    .with_context(|| format!("Failed to write {}", path.display()))
}

/// Writes the plugin under `kimi_home` and registers it.
fn install_plugin(kimi_home: &Path, mcp_url: &str, system_prompt: &str, now: &str) -> Result<()> {
    let root = plugin_root(kimi_home);
    std::fs::create_dir_all(&root)
        .with_context(|| format!("Failed to create {}", root.display()))?;
    write_manifest(&root, mcp_url, Some(system_prompt))?;

    // Kimi stores the resolved root and checks paths after symlink resolution.
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    register_plugin(
        &kimi_home.join("plugins").join("installed.json"),
        &root,
        now,
    )
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Step 1: ensure we are authenticated
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }

    // Step 1b: ensure an org is selected (handles partial state after aborted login)
    crate::commands::auth::login::ensure_org_selected().await?;

    // Step 2: ensure we have a live api_key for Kimi Code. Re-provisions if the
    // cached key was deleted in the console; re-runs onboarding for a fresh key.
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("kimi")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("kimi").await?;
    }
    creds = crate::config::read()?;

    // Step 3: ensure we have a connection choice (default to "plan")
    if creds
        .kimi
        .as_ref()
        .and_then(|c| c.connection.as_deref())
        .is_none()
    {
        let provider = creds.kimi.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    // Step 3b: fetch the org once and derive both the gateway URL and the MCP
    // gate from it. `ensure_mcp_preference` may prompt and persist, so `creds`
    // is re-read before the long-lived `kimi` borrow starts.
    let org = super::fetch_active_org(&creds).await;
    let gateway_url = super::gateway_base_url_with_org(org.as_ref());
    let mcp_disabled = super::mcp_injection_disabled_with_org(org.as_ref());
    if !mcp_disabled {
        crate::commands::auth::login::ensure_mcp_preference().await?;
        creds = crate::config::read()?;
    }
    let use_mcp = creds.enable_mcp.unwrap_or(false) && !mcp_disabled;

    let kimi = creds.kimi.as_ref().unwrap();
    let api_key = &kimi.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();

    // First-run: install the persistent user-level statusline integration
    // exactly once (Claude Code-targeted; honors the disable marker).
    util::ensure_first_run_installed().await;

    // First-run: install Kimi's own statusline integration exactly once
    // (honors its own, separate disable marker).
    util::ensure_kimi_first_run_installed().await;

    util::spawn_cli_version_report(&creds, &session_id);

    let repo_origin = crate::git::detect_origin();
    let repo_header = repo_origin
        .as_ref()
        .map(|url| format!("\nx-edgee-repo: {url}"))
        .unwrap_or_default();
    let debug_log_header = util::resolve_debug_log_keypair()?
        .map(|keypair| {
            let headers = keypair.header_values();
            format!(
                "\nx-edgee-debug-pubkey: {}\nx-edgee-debug-salt: {}",
                headers.pubkey, headers.salt
            )
        })
        .unwrap_or_default();

    // Step 4: launch kimi on an in-memory provider pointed at the gateway.
    // `KIMI_MODEL_NAME` is both the model id and the enable switch: unset, none
    // of the others are read; set with a required one missing, kimi fails fast
    // at startup rather than silently talking to Moonshot.
    let mut cmd = std::process::Command::new(util::resolve_binary("kimi"));
    cmd.env("KIMI_MODEL_NAME", resolve_model())
        .env("KIMI_MODEL_API_KEY", api_key)
        .env("KIMI_MODEL_BASE_URL", &gateway_url)
        .env("KIMI_MODEL_PROVIDER_TYPE", "anthropic");

    // Session attribution. `KIMI_CODE_CUSTOM_HEADERS` takes one `Name: Value`
    // per line and applies to outbound LLM requests — the same shape as
    // `ANTHROPIC_CUSTOM_HEADERS` in `claude.rs`.
    cmd.env(
        "KIMI_CODE_CUSTOM_HEADERS",
        format!(
            "x-edgee-api-key: {api_key}\nx-edgee-session-id: {session_id}{repo_header}{debug_log_header}"
        ),
    );

    // Set up the environment for Edgee session tracking and console API access.
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env(
        "EDGEE_ORG_SLUG",
        creds.org_slug.as_deref().unwrap_or_default(),
    );
    cmd.env(
        "EDGEE_CONSOLE_API_URL",
        crate::config::console_api_base_url(),
    );

    // Step 5: register Edgee's own MCP tools and nudge the model to use them.
    let mut installed_plugin: Option<(PathBuf, String)> = None;
    if use_mcp {
        let kimi_home = kimi_code_home()
            .context("Could not determine your home directory")?;
        let mcp_url = crate::config::mcp_base_url();
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .context("Failed to format the current time")?;
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
        install_plugin(&kimi_home, &mcp_url, &text, &now)?;
        cmd.env(MCP_TOKEN_ENV, creds.user_token.as_deref().unwrap_or(""));
        installed_plugin = Some((plugin_root(&kimi_home), mcp_url));
    }

    cmd.args(&opts.args);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Kimi Code is not installed. Install it with `curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash`"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    // Drop this session's instructions so a bare `kimi` run doesn't inherit
    // a stale session id. Best effort: the next launch rewrites it anyway.
    if let Some((root, mcp_url)) = installed_plugin {
        let _ = write_manifest(&root, &mcp_url, None);
    }

    // No-ops when the gateway saw no traffic for this session.
    super::print_session_stats(&creds, &session_id, "Kimi Code").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_the_default_model() {
        assert_eq!(pick_model(None), DEFAULT_MODEL);
    }

    #[test]
    fn override_wins_when_set() {
        assert_eq!(pick_model(Some("moonshotai/kimi-k3")), "moonshotai/kimi-k3");
    }

    // An empty `KIMI_MODEL_NAME` is a startup error in kimi, so a blank export
    // must read as "not set" rather than be forwarded verbatim.
    #[test]
    fn blank_override_falls_back_to_the_default() {
        assert_eq!(pick_model(Some("   ")), DEFAULT_MODEL);
        assert_eq!(pick_model(Some("")), DEFAULT_MODEL);
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    const NOW: &str = "2026-09-23T00:00:00Z";

    #[test]
    fn manifest_registers_mcp_without_a_secret() {
        let manifest = plugin_manifest("https://mcp.test/mcp", None);
        let edgee = &manifest["mcpServers"]["edgee"];
        assert_eq!(edgee["url"], "https://mcp.test/mcp");
        assert_eq!(edgee["bearerTokenEnvVar"], MCP_TOKEN_ENV);
        assert!(edgee.get("headers").is_none());
    }

    #[test]
    fn manifest_carries_the_prompt_only_when_given() {
        assert!(plugin_manifest("u", None).get("systemPrompt").is_none());
        assert_eq!(
            plugin_manifest("u", Some("track 42999158-ae9f-5b44-8834-27675aacf427"))
                ["systemPrompt"],
            "track 42999158-ae9f-5b44-8834-27675aacf427"
        );
    }

    #[test]
    fn install_writes_the_manifest_and_registers_it() {
        let dir = tempfile::tempdir().unwrap();

        install_plugin(dir.path(), "https://mcp.test/mcp", "nudge", NOW).unwrap();

        let root = plugin_root(dir.path());
        let manifest = read_json(&root.join("kimi.plugin.json"));
        assert_eq!(manifest["name"], PLUGIN_ID);
        assert_eq!(manifest["systemPrompt"], "nudge");
        let installed = read_json(&dir.path().join("plugins/installed.json"));
        assert_eq!(installed["version"], 1);
        let record = &installed["plugins"][0];
        assert_eq!(record["id"], PLUGIN_ID);
        assert_eq!(record["enabled"], true);
        assert_eq!(
            record["root"],
            std::fs::canonicalize(&root)
                .unwrap()
                .to_string_lossy()
                .as_ref()
        );
    }

    // What `run` does on exit: the MCP entry stays, the session prompt goes.
    #[test]
    fn rewriting_without_a_prompt_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        install_plugin(dir.path(), "https://mcp.test/mcp", "nudge", NOW).unwrap();
        let root = plugin_root(dir.path());

        write_manifest(&root, "https://mcp.test/mcp", None).unwrap();

        let manifest = read_json(&root.join("kimi.plugin.json"));
        assert!(manifest.get("systemPrompt").is_none());
        assert!(manifest["mcpServers"]["edgee"].is_object());
    }

    #[test]
    fn register_preserves_other_plugins_and_user_choices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installed.json");
        std::fs::write(
            &path,
            r#"{"version":1,"plugins":[
                {"id":"other","root":"/p/other","enabled":true},
                {"id":"edgee","root":"/old","enabled":false,"installedAt":"2026-01-01T00:00:00Z"}
            ]}"#,
        )
        .unwrap();

        register_plugin(&path, Path::new("/new"), NOW).unwrap();

        let plugins = read_json(&path)["plugins"].as_array().unwrap().clone();
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0]["id"], "other");
        assert_eq!(plugins[1]["root"], "/new");
        assert_eq!(plugins[1]["enabled"], false);
        assert_eq!(plugins[1]["installedAt"], "2026-01-01T00:00:00Z");
        assert_eq!(plugins[1]["updatedAt"], NOW);
    }

    #[test]
    fn register_refuses_to_clobber_an_invalid_registry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("installed.json");
        for original in ["{ this is not json", r#"{"version":1}"#] {
            std::fs::write(&path, original).unwrap();
            assert!(register_plugin(&path, Path::new("/new"), NOW).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }
}
