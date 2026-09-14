use anyhow::Result;

use super::util;
use crate::commands::util::plugins;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the codex CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// Bare MCP tool names, matching how Codex's `enabled_tools` addresses tools on
/// a named server — unlike Claude's `mcp__edgee__<tool>` allowedTools entries,
/// which are Claude's own namespacing convention, not the underlying tool name.
const EDGEE_ENABLED_TOOLS: &[&str] = &[
    "setSessionName",
    "addSessionPullRequest",
    "addSessionCommit",
    "setSessionGitRepo",
];

/// `-c mcp_servers.edgee.*` overrides for Edgee's own session-tracking MCP
/// server — the same server claude.rs points `--mcp-config` at, delivered here
/// as TOML overrides instead of a JSON file, the same way `codex_mcp_args`
/// delivers plugin MCP servers.
///
/// No system-prompt nudge: `experimental_instructions_file` replaces Codex's
/// built-in instructions rather than appending, so tool adherence relies on
/// the model reading the MCP tool descriptions on its own.
fn edgee_mcp_args(token: &str) -> Vec<String> {
    let mut args = vec![format!(
        "mcp_servers.edgee.url={}",
        plugins::config::toml_string(&crate::config::mcp_base_url())
    )];

    let mut headers = std::collections::HashMap::new();
    headers.insert("Authorization".to_string(), format!("Bearer {token}"));
    args.push(format!(
        "mcp_servers.edgee.http_headers={}",
        plugins::config::toml_table(&headers)
    ));

    let tools = EDGEE_ENABLED_TOOLS
        .iter()
        .map(|t| plugins::config::toml_string(t))
        .collect::<Vec<_>>()
        .join(", ");
    args.push(format!("mcp_servers.edgee.enabled_tools=[{tools}]"));

    args
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Step 1: ensure we are authenticated
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }

    // Step 1b: ensure an org is selected (handles partial state after aborted login)
    crate::commands::auth::login::ensure_org_selected().await?;

    // Step 2: ensure we have a live api_key for Codex. Re-provisions if the
    // cached key was deleted in the console; re-runs onboarding for a fresh key.
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("codex")
        .await?
        .created;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("codex").await?;
    }
    creds = crate::config::read()?;

    // Step 3: ensure we have a connection choice (default to "plan" for codex)
    if creds
        .codex
        .as_ref()
        .and_then(|c| c.connection.as_deref())
        .is_none()
    {
        let provider = creds.codex.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    let org = super::fetch_active_org(&creds).await;
    let mcp_disabled = super::mcp_injection_disabled_with_org(org.as_ref());
    if !mcp_disabled {
        crate::commands::auth::login::ensure_mcp_preference().await?;
        creds = crate::config::read()?;
    }

    // Step 3: launch codex with the correct env vars
    let codex = creds.codex.as_ref().unwrap();
    let api_key = &codex.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();

    // First-run: install the persistent user-level statusline integration
    // exactly once. Codex itself doesn't render an Edgee statusline today,
    // but users typically also use Claude Code in the same shell — running
    // the installer on the first `edgee launch` of any agent matches the
    // "set it up once" flow we want.
    util::ensure_first_run_installed().await;

    util::spawn_cli_version_report(&creds, &session_id);

    let repo_entry = crate::git::detect_origin()
        .map(|url| format!(",\"x-edgee-repo\"=\"{url}\""))
        .unwrap_or_default();
    let debug_log_entry = util::resolve_debug_log_keypair()?
        .map(|keypair| {
            let headers = keypair.header_values();
            format!(",\"x-edgee-debug-pubkey\"=\"{}\",\"x-edgee-debug-salt\"=\"{}\"", headers.pubkey, headers.salt)
        })
        .unwrap_or_default();
    let base_url = format!("{}/v1", super::gateway_base_url_with_org(org.as_ref()));
    let mut cmd = std::process::Command::new(util::resolve_binary("codex"));
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env("EDGEE_ORG_SLUG", creds.org_slug.as_deref().unwrap_or_default());
    cmd.args([
        "-c", "model_provider=\"edgee-cli\"",
        "-c", "model_providers.edgee-cli.name=\"EDGEE\"",
        "-c", &format!("model_providers.edgee-cli.base_url=\"{base_url}\""),
        "-c", &format!("model_providers.edgee-cli.http_headers={{\"x-edgee-api-key\"=\"{api_key}\",\"x-edgee-session-id\"=\"{session_id}\"{repo_entry}{debug_log_entry}}}"),
        "-c", "model_providers.edgee-cli.wire_api=\"responses\"",
        // Codex only attaches the user's ChatGPT `Authorization: Bearer` token to a
        // custom provider when this is set. It used to default to on; codex 0.149
        // flipped it, and every plan-connection request started 401ing gateway-side
        // because no upstream credential arrived. Harmless when the user isn't
        // logged in to codex — the header is simply omitted.
        "-c", "model_providers.edgee-cli.requires_openai_auth=true",
    ]);
    // Org plugins. Codex exposes no way to add a skills directory — only to
    // relocate the whole config root via CODEX_HOME — so we build a symlink
    // mirror of it and add ours there. Symlinks mean the user's auth.json,
    // history and databases are referenced, never copied, and their real
    // ~/.codex is never written to. MCP servers ride `-c` overrides, the same
    // mechanism already used above for the provider.
    let plugin_report = plugins::sync_for_target(&creds, plugins::Target::Codex).await;
    for arg in plugins::config::codex_mcp_args(&plugin_report.plugins) {
        cmd.args(["-c", &arg]);
    }

    // Edgee's own session-tracking MCP server, gated the same way claude.rs
    // gates injection (org policy, then the member's own preference).
    let use_mcp = creds.enable_mcp.unwrap_or(false) && !mcp_disabled;
    if use_mcp {
        let token = creds.user_token.as_deref().unwrap_or("");
        for arg in edgee_mcp_args(token) {
            cmd.args(["-c", &arg]);
        }
    }
    if let Some(skills_root) = plugin_report.skills_root.as_ref() {
        if let Some(mirror) = plugins::codex_home_mirror() {
            let source = plugins::codex_home_source();
            // Delivering nothing beats delivering into a half-built mirror, so a
            // failure here simply leaves CODEX_HOME alone.
            if plugins::mirror::build(&source, &mirror, &["skills"]).is_ok()
                && plugins::mirror::link_children(skills_root, &mirror.join("skills")).is_ok()
            {
                cmd.env("CODEX_HOME", &mirror);
            }
        }
    }
    plugins::report_launch(&plugin_report);

    cmd.args(&opts.args);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Codex CLI is not installed. Install it from https://developers.openai.com/codex/cli"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    super::print_session_stats(&creds, &session_id, "Codex").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edgee_mcp_args_are_well_formed_toml_overrides() {
        let args = edgee_mcp_args("tok_123");

        assert!(args
            .iter()
            .any(|a| a.starts_with("mcp_servers.edgee.url=") && a.contains(&crate::config::mcp_base_url())));
        assert!(args
            .contains(&r#"mcp_servers.edgee.http_headers={"Authorization"="Bearer tok_123"}"#.to_string()));
        assert!(args.contains(&format!(
            "mcp_servers.edgee.enabled_tools=[{}]",
            EDGEE_ENABLED_TOOLS
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    /// A token with a quote or newline must not break out of the TOML string —
    /// same hazard `codex_toml_strings_escape_control_characters` guards for
    /// plugin MCP servers.
    #[test]
    fn edgee_mcp_args_escape_control_characters_in_the_token() {
        let args = edgee_mcp_args("say \"hi\"\nthere");
        let headers = args
            .iter()
            .find(|a| a.starts_with("mcp_servers.edgee.http_headers="))
            .unwrap();

        assert!(headers.contains(r#"\""#));
        assert!(headers.contains(r"\n"));
        assert!(!headers.contains('\n'));
    }
}
