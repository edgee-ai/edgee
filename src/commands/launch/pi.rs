//! `edgee launch pi` — Pi CLI (<https://pi.dev>).
//!
//! Pi resolves models through `<agent dir>/models.json`, where custom providers
//! merge into the built-in catalog by `provider + id`. Registering a provider
//! named `edgee` is therefore purely additive: every provider, model and login
//! the user already had keeps working untouched.
//!
//! ## Why this writes the user's real config instead of a temp one
//!
//! `opencode.rs` and `crush.rs` build a merged config in `$TMPDIR` and point the
//! agent at it (`OPENCODE_CONFIG`, `CRUSH_GLOBAL_CONFIG`), leaving the user's
//! files untouched. Pi has no equivalent: its only relevant env var,
//! `PI_CODING_AGENT_DIR`, relocates the *entire* agent directory — not just
//! `models.json` but `auth.json`, `settings.json`, `keybindings.json`,
//! `sessions/`, `themes/`, `tools/`, `prompts/`, `bin/`, plus extension,
//! skill and plugin discovery. Pointing it at a temp dir would drop the user
//! into pi with no history, no logins, no settings and none of their installed
//! plugins. There is no narrower lever: `--models` takes model *patterns* for
//! Ctrl+P cycling, not a config path.
//!
//! So the provider block is written into the real `models.json` under a single
//! namespaced key. This needs no patch-and-revert dance (unlike
//! `codex_desktop.rs`) precisely because it is additive rather than a hijack of
//! a provider the user already relies on.
//!
//! ## No credential at rest
//!
//! Pi resolves `apiKey` and header values through `resolveConfigValue`, which
//! expands `$NAME` / `${NAME}` references against the environment. So the block
//! stores *references* — `$EDGEE_API_KEY`, `$EDGEE_SESSION_ID` — and `run`
//! supplies the values at spawn time. The Edgee key is never written to disk,
//! unlike the OpenCode and Crush temp configs, which embed it.
//!
//! **This requires pi 0.79.4 or newer**, and the version boundary is a trap
//! worth knowing about. Before 0.79.4 the whole value was the variable name
//! (bare `EDGEE_API_KEY`) and an unset variable silently fell through to the
//! literal string; 0.79.4 reversed that, making bare uppercase values literals
//! and `$NAME` the only env reference (upstream #5661). The two spellings are
//! mutually exclusive — each is an inert literal on the other side of that
//! boundary, and the symptom is identical either way: the gateway answers 401
//! because it was handed the string `EDGEE_API_KEY` or `$EDGEE_API_KEY` as a
//! credential. We emit the current, documented form.
//!
//! Modern pi resolves an unset reference to `undefined` rather than leaking the
//! literal, so the failure mode there is at least a clean "no credential".
//!
//! The flip side of storing references: a bare `pi` run outside
//! `edgee launch pi` can see the Edgee models but cannot authenticate them. That
//! is the deliberate trade — the credential stays out of the config file.

use anyhow::{Context, Result};
use serde_json::Value;

use super::util;

/// Provider key under `providers` in `models.json`. Everything this command
/// writes lives under it; nothing else in the file is touched.
const PROVIDER_KEY: &str = "edgee";

/// Env vars whose values `run` supplies at spawn time. The config embeds them
/// as `$NAME` references (see [`env_ref`]), never their values.
const API_KEY_ENV: &str = "EDGEE_API_KEY";
const SESSION_ID_ENV: &str = "EDGEE_SESSION_ID";

/// Renders an env var name as the `$NAME` reference pi expands at request time.
fn env_ref(name: &str) -> String {
    format!("${name}")
}

/// Pi's picker uses fixed slot names. The catalog's `none` effort maps to
/// Pi's disabled `off` slot.
const PI_THINKING_LEVELS: [(&str, &str); 7] = [
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
        PI_THINKING_LEVELS
            .into_iter()
            .map(|(pi_level, catalog_effort)| {
                let value = if efforts.iter().any(|effort| effort == catalog_effort) {
                    Value::String(catalog_effort.to_string())
                } else {
                    Value::Null
                };
                (pi_level.to_string(), value)
            })
            .collect(),
    )
}

/// Output cap declared for every model. The gateway catalog carries a context
/// window but no per-model output limit, and pi has no "unset" for `maxTokens`
/// short of omitting it — which makes pi fall back to a conservative built-in
/// default. This is high enough not to truncate coding turns.
const PI_OUTPUT_TOKEN_MAX: u64 = 64_000;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the pi CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(std::path::PathBuf::from)
}

/// Pi's agent directory, mirroring its own resolution order: an explicit
/// `PI_CODING_AGENT_DIR` (tilde-expanded, as pi does) wins over `~/.pi/agent`.
/// Honoring the override matters — a user who has relocated their pi config
/// should have the provider written where pi will actually read it.
fn agent_dir() -> Option<std::path::PathBuf> {
    if let Some(dir) = std::env::var("PI_CODING_AGENT_DIR")
        .ok()
        .filter(|d| !d.is_empty())
    {
        if dir == "~" {
            return home_dir();
        }
        if let Some(rest) = dir.strip_prefix("~/") {
            return home_dir().map(|h| h.join(rest));
        }
        return Some(std::path::PathBuf::from(dir));
    }
    home_dir().map(|h| h.join(".pi").join("agent"))
}

/// Reads the user's `models.json`, or an empty document when absent. A file we
/// cannot parse is *not* overwritten — see [`write_provider`].
fn read_models_json(path: &std::path::Path) -> Result<Option<Value>> {
    if !path.exists() {
        return Ok(Some(serde_json::json!({ "providers": {} })));
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    // An empty file is equivalent to no file; pi treats both as "no overrides".
    if content.trim().is_empty() {
        return Ok(Some(serde_json::json!({ "providers": {} })));
    }
    Ok(serde_json::from_str(&content).ok())
}

/// Inserts the Edgee provider into `models.json`, preserving every other key.
///
/// Bails rather than clobbering when the existing file does not parse: pi
/// tolerates comments in `models.json` (`stripJsonComments`), and a user's
/// annotated config is not ours to silently rewrite into canonical JSON.
fn write_provider(path: &std::path::Path, provider: Value) -> Result<()> {
    let Some(mut config) = read_models_json(path)? else {
        anyhow::bail!(
            "{} exists but is not valid JSON.\nFix or remove it, then run `edgee launch pi` again.",
            path.display()
        )
    };

    if !config.is_object() {
        anyhow::bail!(
            "{} does not contain a JSON object.\nFix or remove it, then run `edgee launch pi` again.",
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
    providers
        .as_object_mut()
        .expect("just ensured object")
        .insert(PROVIDER_KEY.to_string(), provider);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let rendered = serde_json::to_string_pretty(&config)?;
    std::fs::write(path, format!("{rendered}\n"))
        .with_context(|| format!("Failed to write {}", path.display()))
}

/// Builds the `providers.edgee` block.
///
/// `api` is `anthropic-messages` and `baseUrl` carries no `/v1` suffix, matching
/// pi's built-in Anthropic provider (`https://api.anthropic.com`) — pi appends
/// `/v1/messages` itself, exactly as `ANTHROPIC_BASE_URL` does for Claude Code.
/// The gateway translates that shape for the whole catalog, so non-Anthropic
/// models route through it too.
fn build_edgee_provider(
    gateway_url: &str,
    models: &[String],
    catalog: &util::ModelCatalog,
    debug_log_headers: Option<crate::crypto::DebugLogHeaderValues>,
) -> Value {
    let mut headers = serde_json::json!({
        "x-edgee-api-key": env_ref(API_KEY_ENV),
        "x-edgee-session-id": env_ref(SESSION_ID_ENV),
    });
    // Unlike the key and session id, these are embedded literally: they derive
    // from the profile passphrase and so are stable across launches, and a
    // public key plus salt is not a secret. Env-var indirection would only add
    // a way for them to resolve to nothing on a bare `pi` run.
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
        "name": "Edgee",
        "baseUrl": gateway_url,
        "api": "anthropic-messages",
        "apiKey": env_ref(API_KEY_ENV),
        "headers": headers,
    });

    if !models.is_empty() {
        let entries: Vec<Value> = models
            .iter()
            .map(|id| {
                // The gateway id (`anthropic/claude-sonnet-5`) is the routing
                // identifier, so it doubles as pi's model id. `--model` matches
                // on id as well as name, so `--model anthropic/claude-sonnet-5`
                // works without a provider prefix.
                let mut entry = serde_json::json!({ "id": id, "name": id });
                let metadata = catalog.get(id);
                if let Some(context) = metadata.and_then(|m| m.context) {
                    entry["contextWindow"] = serde_json::json!(context);
                    entry["maxTokens"] = serde_json::json!(PI_OUTPUT_TOKEN_MAX);
                }
                // Dollars per million tokens — the same unit pi's built-in
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
                    // This provider always talks to the gateway. Adaptive
                    // thinking preserves the exact categorical effort here;
                    // the gateway then translates it for the routed provider.
                    entry["compat"] = serde_json::json!({
                        "forceAdaptiveThinking": true,
                    });
                }
                entry
            })
            .collect();
        provider["models"] = Value::Array(entries);
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

    // Step 2: ensure we have a live api_key for Pi. Re-provisions if the cached
    // key was deleted in the console; re-runs onboarding for a fresh key.
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("pi")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("pi").await?;
    }
    creds = crate::config::read()?;

    // Step 3: ensure we have a connection choice (default to "plan")
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
    let session_id = uuid::Uuid::new_v4().to_string();
    util::spawn_cli_version_report(&creds, &session_id);

    // First-run: install the persistent user-level statusline integration
    // exactly once (Claude Code-targeted; honors the disable marker).
    util::ensure_first_run_installed().await;

    // Step 4: register the Edgee provider in the user's models.json
    let gateway_url = super::resolve_gateway_base_url(&creds).await;

    let (models, catalog) = tokio::join!(
        util::fetch_gateway_models(&gateway_url, api_key),
        util::fetch_model_catalog(&creds)
    );
    let models = util::without_app_subscription_models(models, &catalog);
    // `fetch_gateway_models` is best-effort and yields an empty list on any
    // failure. OpenCode survives that — its provider still works without an
    // explicit model map. Pi does not: a custom provider is *defined* by its
    // models, so an empty list registers a provider pi can offer nothing from
    // and the session opens on "No models available". Stop before touching the
    // user's config rather than launching into that dead end.
    if models.is_empty() {
        // Distinguish the two causes, because they look identical from here and
        // the second one is easy to misread as an auth problem: the gateway may
        // be unreachable, or up and serving an empty catalog — it proxies
        // `/v1/models` from the console API, so a dev stack with an unseeded
        // model table answers 200 with `{"data":[]}` for any key, valid or not.
        anyhow::bail!(
            "The gateway at {gateway_url} returned no models, and Pi needs an explicit model list.\n\
             Check that the gateway is reachable and that its model catalog is populated \
             (`curl {gateway_url}/v1/models`), then run `edgee launch pi` again."
        );
    }
    let debug_log_headers = util::resolve_debug_log_keypair()?.map(|k| k.header_values());
    let provider = build_edgee_provider(&gateway_url, &models, &catalog, debug_log_headers);

    let agent_dir = agent_dir().context("Could not determine your home directory")?;
    write_provider(&agent_dir.join("models.json"), provider)?;

    // Step 5: launch pi with the values its config refers to by name
    let mut cmd = std::process::Command::new(util::resolve_binary("pi"));
    cmd.env(API_KEY_ENV, api_key);
    cmd.env(SESSION_ID_ENV, &session_id);
    cmd.env("EDGEE_ORG_SLUG", creds.org_slug.as_deref().unwrap_or_default());
    cmd.args(&opts.args);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Pi is not installed. Install it with `npm install -g @mariozechner/pi-coding-agent`"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    super::print_session_stats(&creds, &session_id, "Pi").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
                app_subscription_only: false,
            },
        );
        catalog
    }

    fn catalog_with(id: &str, context: Option<u64>) -> util::ModelCatalog {
        catalog_with_efforts(id, context, &[])
    }

    #[test]
    fn stores_env_references_not_the_credential() {
        let provider =
            build_edgee_provider("https://api.edgee.ai", &[], &util::ModelCatalog::new(), None);

        // `$NAME`, the syntax pi has expanded since 0.79.4. A bare `EDGEE_API_KEY`
        // is a plain literal there and would be sent verbatim as the credential.
        assert_eq!(provider["apiKey"], "$EDGEE_API_KEY");
        assert_eq!(provider["headers"]["x-edgee-api-key"], "$EDGEE_API_KEY");
        assert_eq!(
            provider["headers"]["x-edgee-session-id"],
            "$EDGEE_SESSION_ID"
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
    fn base_url_carries_no_v1_suffix() {
        // Pi appends `/v1/messages` itself, matching its built-in Anthropic
        // provider. A `/v1` here would produce `/v1/v1/messages`.
        let provider =
            build_edgee_provider("https://api.edgee.ai", &[], &util::ModelCatalog::new(), None);
        assert_eq!(provider["baseUrl"], "https://api.edgee.ai");
        assert_eq!(provider["api"], "anthropic-messages");
    }

    #[test]
    fn declares_context_window_only_when_the_catalog_has_one() {
        let models = vec!["anthropic/claude-sonnet-5".to_string(), "zai/glm-5.2".to_string()];
        let catalog = catalog_with("anthropic/claude-sonnet-5", Some(1_000_000));

        let provider = build_edgee_provider("https://api.edgee.ai", &models, &catalog, None);
        let entries = provider["models"].as_array().unwrap();

        assert_eq!(entries[0]["id"], "anthropic/claude-sonnet-5");
        assert_eq!(entries[0]["contextWindow"], 1_000_000);
        assert_eq!(entries[0]["maxTokens"], PI_OUTPUT_TOKEN_MAX);
        // Unknown to the catalog: better to let pi apply its own defaults than
        // to declare a fabricated window.
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
        assert_eq!(model["compat"]["forceAdaptiveThinking"], true);
    }

    #[test]
    fn hides_pi_levels_the_catalog_does_not_support() {
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
    fn preserves_unrelated_config_when_registering() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        std::fs::write(
            &path,
            r#"{"providers":{"ollama":{"baseUrl":"http://localhost:11434/v1","api":"openai-completions","apiKey":"ollama","models":[{"id":"llama3.1:8b"}]}}}"#,
        )
        .unwrap();

        let provider =
            build_edgee_provider("https://api.edgee.ai", &[], &util::ModelCatalog::new(), None);
        write_provider(&path, provider).unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // The user's own provider survives untouched...
        assert_eq!(written["providers"]["ollama"]["apiKey"], "ollama");
        assert_eq!(
            written["providers"]["ollama"]["models"][0]["id"],
            "llama3.1:8b"
        );
        // ...alongside ours.
        assert_eq!(written["providers"]["edgee"]["name"], "Edgee");
    }

    #[test]
    fn replaces_only_its_own_provider_on_relaunch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");

        let first = build_edgee_provider(
            "https://api.edgee.ai",
            &["anthropic/claude-sonnet-5".to_string()],
            &catalog_with("anthropic/claude-sonnet-5", Some(1_000_000)),
            None,
        );
        write_provider(&path, first).unwrap();
        let second = build_edgee_provider(
            "https://gateway.example.com",
            &[],
            &util::ModelCatalog::new(),
            None,
        );
        write_provider(&path, second).unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Rewritten wholesale rather than merged, so a model dropped from the
        // gateway catalog does not linger in the user's picker forever.
        assert_eq!(
            written["providers"]["edgee"]["baseUrl"],
            "https://gateway.example.com"
        );
        assert!(written["providers"]["edgee"].get("models").is_none());
    }

    #[test]
    fn refuses_to_clobber_an_unparseable_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("models.json");
        let original = "{ this is not json";
        std::fs::write(&path, original).unwrap();

        let provider =
            build_edgee_provider("https://api.edgee.ai", &[], &util::ModelCatalog::new(), None);
        assert!(write_provider(&path, provider).is_err());
        // The user's file is left exactly as it was.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn creates_the_agent_dir_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("agent").join("models.json");

        let provider =
            build_edgee_provider("https://api.edgee.ai", &[], &util::ModelCatalog::new(), None);
        write_provider(&path, provider).unwrap();

        let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["providers"]["edgee"]["name"], "Edgee");
    }
}
