//! `edgee launch kimi` — Kimi Code CLI (<https://moonshotai.github.io/kimi-code/>).
//!
//! ## Why this target is env-only
//!
//! Kimi Code deliberately does **not** read provider credentials from the shell:
//! `api_key` / `base_url` come from `config.toml` (or its `[providers.<n>.env]`
//! sub-table), and exporting `KIMI_API_KEY` has no effect at all. The one
//! documented exception is the `KIMI_MODEL_*` family — "an explicit channel that
//! *does* read credentials from the shell". Setting `KIMI_MODEL_NAME` makes the
//! CLI synthesize a provider **and** a model alias in memory, taking priority
//! over `default_model` in `config.toml` and vanishing when the process exits.
//!
//! That makes this the closest analogue to `claude.rs` in the catalogue: nothing
//! is written to the user's files, a bare `kimi` run afterwards is untouched,
//! and there is no patch-and-revert dance (`codex_desktop.rs`) or additive
//! config block (`pi.rs`) to maintain.
//!
//! ## Wire shape
//!
//! `KIMI_MODEL_PROVIDER_TYPE=anthropic` selects Kimi's Anthropic Messages
//! implementation, and — exactly like `ANTHROPIC_BASE_URL` for Claude Code and
//! `baseUrl` for pi — the base URL carries **no `/v1` suffix**: the SDK appends
//! `/v1/messages` itself. The gateway translates that shape for its whole
//! catalog, so a `moonshotai/…` model routes through it fine.
//!
//! ## Session attribution
//!
//! `KIMI_MODEL_*` carries no headers of its own, but Kimi Code 0.20.2 added
//! `KIMI_CODE_CUSTOM_HEADERS` — one `Name: Value` per line, applied to outbound
//! LLM requests. That is the same shape as `ANTHROPIC_CUSTOM_HEADERS`, so this
//! target sends the full Edgee header set (`x-edgee-session-id`, `x-edgee-repo`,
//! debug-log keys) exactly like `claude.rs`, and sessions group in the console.
//!
//! The credential still rides as `KIMI_MODEL_API_KEY` rather than as
//! `x-edgee-api-key` alone: the gateway resolves a key from `x-api-key` /
//! `Authorization: Bearer` and answers 401 to `x-edgee-api-key` by itself, and
//! `KIMI_MODEL_API_KEY` is a required variable regardless. The header is sent
//! too, for parity with the other targets.
//!
//! Note the env var was undocumented on the docs site at the time of writing —
//! it is in the 0.20.2 release notes and in the binary, not in the environment
//! variables reference. If a future release drops it, the fallback is an
//! additive `[providers.edgee]` block with `custom_headers` in the user's
//! `config.toml`, the way `pi.rs` writes `models.json`.

use anyhow::Result;

use super::util;

/// Model id handed to `KIMI_MODEL_NAME`, and therefore the single entry point
/// the session runs on. One model is enough by design: the gateway's routing
/// engine decides what actually serves each request, so this only has to name a
/// sensible default from the catalog.
const DEFAULT_MODEL: &str = "moonshotai/kimi-k2.7-code";

/// Escape hatch for the model above.
const MODEL_ENV_OVERRIDE: &str = "EDGEE_KIMI_MODEL";

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

    let kimi = creds.kimi.as_ref().unwrap();
    let api_key = &kimi.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();

    // First-run: install the persistent user-level statusline integration
    // exactly once (Claude Code-targeted; honors the disable marker).
    util::ensure_first_run_installed().await;

    util::spawn_cli_version_report(&creds, &session_id);

    let gateway_url = super::resolve_gateway_base_url(&creds).await;

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
        "EDGEE_CONSOLE_API_URL",
        crate::config::console_api_base_url(),
    );

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
}
