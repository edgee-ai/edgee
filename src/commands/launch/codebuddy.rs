use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;

use super::util;
use crate::commands::util::plugins;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the codebuddy CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// CodeBuddy is a Claude-Code-compatible fork with an identical `--allowedTools`
/// surface, so it accepts the same `mcp__edgee__*` tool-name convention Claude
/// Code itself uses.
const EDGEE_ALLOWED_TOOLS: &str = "mcp__edgee__setSessionName,mcp__edgee__addSessionPullRequest,mcp__edgee__addSessionCommit,mcp__edgee__setSessionGitRepo";

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Step 1: ensure we are authenticated
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }

    // Step 1b: ensure an org is selected (handles partial state after aborted login)
    crate::commands::auth::login::ensure_org_selected().await?;

    // Step 2: ensure we have a live api_key for CodeBuddy. Re-provisions if the
    // cached key was deleted in the console; re-runs onboarding for a fresh key.
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("codebuddy")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("codebuddy").await?;
    }
    creds = crate::config::read()?;

    // Step 3: ensure we have a connection choice (default to "plan" for codebuddy)
    if creds
        .codebuddy
        .as_ref()
        .and_then(|c| c.connection.as_deref())
        .is_none()
    {
        let provider = creds.codebuddy.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    // Step 3b: fetch the org once and derive both the gateway URL and the MCP
    // gate from it, rather than issue a second request later. Done before
    // borrowing `creds.codebuddy` below since a positive gate re-reads `creds`.
    let org = super::fetch_active_org(&creds).await;
    let mcp_disabled = super::mcp_injection_disabled_with_org(org.as_ref());
    if !mcp_disabled {
        crate::commands::auth::login::ensure_mcp_preference().await?;
        creds = crate::config::read()?;
    }

    // Step 4: launch codebuddy with the correct env vars
    let codebuddy = creds.codebuddy.as_ref().unwrap();
    let api_key = &codebuddy.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();

    // First-run: install the persistent user-level statusline integration
    // exactly once. CodeBuddy itself doesn't render an Edgee statusline today,
    // but users typically also use Claude Code in the same shell — running
    // the installer on the first `edgee launch` of any agent matches the
    // "set it up once" flow we want.
    util::ensure_first_run_installed().await;

    util::spawn_cli_version_report(&creds, &session_id);

    let repo_origin = crate::git::detect_origin();
    let repo_entry = repo_origin
        .as_deref()
        .map(|url| format!(",\"x-edgee-repo\"=\"{url}\""))
        .unwrap_or_default();
    let debug_log_header = util::resolve_debug_log_keypair()?
        .map(|keypair| {
            let headers = keypair.header_values();
            format!("\nx-edgee-debug-pubkey: {}\nx-edgee-debug-salt: {}", headers.pubkey, headers.salt)
        })
        .unwrap_or_default();
    let base_url = format!("{}/v1", super::gateway_base_url_with_org(org.as_ref()));
    let mut cmd = std::process::Command::new(util::resolve_binary("codebuddy"));
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env("EDGEE_ORG_SLUG", creds.org_slug.as_deref().unwrap_or_default());
    cmd.env("CODEBUDDY_BASE_URL", &base_url);
    cmd.env(
        "CODEBUDDY_CUSTOM_HEADERS",
        format!(
            "x-edgee-api-key: {api_key}\nx-edgee-session-id: {session_id}{repo_entry}{debug_log_header}"
        ),
    );

    let use_mcp = creds.enable_mcp.unwrap_or(false) && !mcp_disabled;
    if use_mcp {
        let mcp_config_path = write_mcp_config(&creds)?;
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
        cmd.args(mcp_injection_args(
            &mcp_config_path,
            &super::mcp::session_instructions(&session_id, repo_origin.as_deref(), &session_url),
        ));
    }

    // Org plugins. CodeBuddy documents CODEBUDDY_PLUGIN_DIRS as the env-var form
    // of `--plugin-dir` and reads Claude Code's bundle format, so it receives the
    // byte-identical tree — nothing is written into the user's ~/.codebuddy.
    let plugins = plugins::sync_for_target(&creds, plugins::Target::Codebuddy).await;
    if !plugins.plugin_dirs.is_empty() {
        // Colon-separated, per CodeBuddy's own documentation.
        let dirs = plugins
            .plugin_dirs
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(":");
        cmd.env("CODEBUDDY_PLUGIN_DIRS", dirs);
    }
    plugins::report_launch(&plugins);

    cmd.args(&opts.args);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "CodeBuddy is not installed. Install it from https://cnb.cool/codebuddy/codebuddy-code"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    super::print_session_stats(&creds, &session_id, "CodeBuddy").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

/// Flags that wire the Edgee MCP server into a CodeBuddy session — same
/// `--flag=value` shape as `claude.rs`'s `mcp_injection_args`, kept a local
/// duplicate rather than shared since this commit must cherry-pick in
/// isolation. `--mcp-config` is non-strict (merges with, rather than
/// replaces, the user's own MCP servers) — `--strict-mcp-config` would drop
/// them, so it's deliberately not used here.
fn mcp_injection_args(config_path: &Path, system_prompt: &str) -> Vec<OsString> {
    let mut mcp_config = OsString::from("--mcp-config=");
    mcp_config.push(config_path);

    vec![
        mcp_config,
        OsString::from("--append-system-prompt"),
        OsString::from(system_prompt),
        OsString::from(format!("--allowedTools={EDGEE_ALLOWED_TOOLS}")),
    ]
}

/// Writes an MCP config file to the Edgee config directory with the user's
/// auth token. Uses a distinct filename from Claude's own `mcp.json` — both
/// share `crate::config::config_dir()`, so a same-named file would clobber
/// whichever agent wrote last.
fn write_mcp_config(creds: &crate::config::Credentials) -> Result<std::path::PathBuf> {
    let token = creds.user_token.as_deref().unwrap_or("");
    let mcp_config = serde_json::json!({
        "mcpServers": {
            "edgee": {
                "type": "http",
                "url": crate::config::mcp_base_url(),
                "headers": {
                    "Authorization": format!("Bearer {token}")
                }
            }
        }
    });

    let dir = crate::config::config_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("codebuddy-mcp.json");
    std::fs::write(&path, serde_json::to_string_pretty(&mcp_config)?)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variadic_injected_flags_use_equals_form() {
        let injected = mcp_injection_args(Path::new("/tmp/mcp.json"), "sys prompt");

        assert!(injected.contains(&OsString::from("--mcp-config=/tmp/mcp.json")));
        assert!(injected
            .iter()
            .any(|a| a.to_string_lossy() == format!("--allowedTools={EDGEE_ALLOWED_TOOLS}")));
        assert!(
            !injected
                .iter()
                .any(|a| a == "--mcp-config" || a == "--allowedTools"),
            "variadic flags must not be passed as a space-separated pair: {injected:?}"
        );
    }

    #[test]
    fn append_system_prompt_stays_a_separate_value_arg() {
        let injected = mcp_injection_args(Path::new("/tmp/mcp.json"), "sys prompt");
        let at = injected
            .iter()
            .position(|a| a == "--append-system-prompt")
            .expect("flag present");

        assert_eq!(injected[at + 1], OsString::from("sys prompt"));
    }
}
