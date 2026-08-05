//! `edgee launch <target>` — public entry points for coding agents and apps.
//!
//! Naming rules for targets (CLI vs apps, suffixes, provider keys vs transport)
//! live in [`README.md`](README.md) in this directory. Read that before adding
//! a new agent.

pub mod claude;
pub mod codebuddy;
pub mod codex;
pub mod crush;
pub mod cursor;
pub mod copilot_vscode;
pub mod opencode;
pub(crate) mod util;

use anyhow::Result;
use console::style;

use super::util::session_log;

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Claude Code CLI
    #[command(next_help_heading = "CLI agents")]
    Claude(claude::Options),
    /// Codex CLI
    Codex(codex::Options),
    /// OpenCode CLI
    #[command(name = "opencode")]
    OpenCode(opencode::Options),
    /// CodeBuddy CLI
    #[command(name = "codebuddy")]
    CodeBuddy(codebuddy::Options),
    /// Crush CLI
    Crush(crush::Options),
    /// Cursor IDE
    #[command(next_help_heading = "Apps & editors")]
    Cursor(cursor::Options),
    /// GitHub Copilot in VS Code
    #[command(name = "copilot-vscode", alias = "vscode-copilot")]
    CopilotVscode(copilot_vscode::Options),
}

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    command: Command,
}

pub async fn run(opts: Options) -> anyhow::Result<()> {
    match opts.command {
        Command::Claude(o) => claude::run(o).await,
        Command::CodeBuddy(o) => codebuddy::run(o).await,
        Command::Codex(o) => codex::run(o).await,
        Command::OpenCode(o) => opencode::run(o).await,
        Command::Crush(o) => crush::run(o).await,
        Command::Cursor(o) => cursor::run(o).await,
        Command::CopilotVscode(o) => copilot_vscode::run(o).await,
    }
}

async fn print_session_stats(
    creds: &crate::config::Credentials,
    session_id: &str,
    tool_name: &str,
) {
    let logs_url = session_log::logs_url_for_session(creds, session_id);

    println!();
    println!(
        "  {} {}",
        style("Session ended.").bold(),
        style(format!("Thanks for using Edgee + {}!", tool_name)).dim()
    );

    let stats = match fetch_stats(creds, session_id).await {
        Ok(s) => s,
        Err(_) => {
            println!(
                "  {} {}",
                style("View your usage & compression stats at").dim(),
                style(&logs_url).cyan().underlined()
            );
            println!();
            return;
        }
    };

    match session_log::build_session_log_entry(
        session_id,
        tool_name,
        logs_url.clone(),
        stats.clone(),
    ) {
        Ok(entry) => {
            let _ = session_log::store_session_log(&entry);
            session_log::render_session_stats(&entry, None);
        }
        Err(_) => {
            let fallback = session_log::SessionLogEntry {
                session_id: session_id.to_string(),
                tool_name: tool_name.to_string(),
                ended_at: "unknown".to_string(),
                ended_at_unix: 0,
                logs_url,
                stats,
            };
            session_log::render_session_stats(&fallback, None);
        }
    }
}

/// Best-effort fetch of the active organization.
///
/// Returns `None` when there is no token, no selected org, or the API is
/// unreachable. Every caller treats that as "no server-side config" and falls
/// back to local defaults, so a transient failure never breaks a launch.
pub async fn fetch_active_org(
    creds: &crate::config::Credentials,
) -> Option<crate::api::Organization> {
    let token = creds.user_token.as_deref().filter(|t| !t.is_empty())?;
    let org_id = creds.org_id.as_deref().filter(|o| !o.is_empty())?;
    let client = crate::api::ApiClient::new(token).ok()?;
    client.get_organization(org_id).await.ok()
}

/// Whether Edgee MCP injection is off for this launch, with the org already
/// fetched (or `None`).
///
/// Precedence (highest first):
/// 1. `EDGEE_MCP_INJECTION_DISABLED` env var — the explicit, ephemeral escape
///    hatch, and the only way to re-enable injection when the org turned it off.
/// 2. The org's console-configured `mcp_injection_disabled` — an admin decision
///    that binds members, so it deliberately outranks the local profile.
/// 3. Not disabled.
///
/// Note this is the inverse of [`gateway_base_url_with_org`], where the local
/// profile beats the org: the gateway URL is a user's own plumbing, whereas MCP
/// injection is org policy. The member's own `enable_mcp` preference still
/// applies underneath — it can decline injection, but cannot force it on
/// against the org.
pub fn mcp_injection_disabled_with_org(org: Option<&crate::api::Organization>) -> bool {
    if let Some(env_disabled) = crate::config::mcp_injection_disabled_env_override() {
        return env_disabled;
    }

    org.is_some_and(|o| o.mcp_injection_disabled)
}

/// Gateway base URL precedence, with the org already fetched (or `None`).
///
/// Split out of [`resolve_gateway_base_url`] so a caller that needs other
/// fields off the same org (e.g. `mcp_injection_disabled`) can fetch it once
/// and reuse it here instead of issuing a second request.
pub fn gateway_base_url_with_org(org: Option<&crate::api::Organization>) -> String {
    if let Some(env_url) = crate::config::gateway_url_env_override() {
        return env_url;
    }

    if let Some(profile_url) = crate::config::gateway_url_profile_override() {
        return profile_url;
    }

    if let Some(url) = org
        .and_then(|o| o.gateway_url.as_deref())
        .filter(|s| !s.is_empty())
    {
        return url.to_string();
    }

    crate::config::DEFAULT_GATEWAY_URL.to_string()
}

/// Resolves the gateway base URL for a launch.
///
/// Precedence (highest first):
/// 1. `EDGEE_API_URL` env var — the explicit, ephemeral escape hatch (local
///    debugging, incident response).
/// 2. The active profile's persisted `gateway_url` — the user's local choice.
/// 3. The org's console-configured `gateway_api_url` — server default when the
///    user hasn't set anything locally.
/// 4. The built-in default.
///
/// Local overrides win over the server value; the server only fills in when the
/// user has no local preference. The org fetch is best-effort: any failure
/// falls through to the next source so launch never breaks (offline, no org
/// selected, or no configured gateway). A local override short-circuits before
/// the fetch, so the network is only touched when the org value could matter.
pub async fn resolve_gateway_base_url(creds: &crate::config::Credentials) -> String {
    if crate::config::gateway_url_env_override().is_some()
        || crate::config::gateway_url_profile_override().is_some()
    {
        return gateway_base_url_with_org(None);
    }

    gateway_base_url_with_org(fetch_active_org(creds).await.as_ref())
}

async fn fetch_stats(
    creds: &crate::config::Credentials,
    session_id: &str,
) -> Result<crate::api::SessionStats> {
    let token = creds
        .user_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow::anyhow!("not authenticated"))?;
    let org_id = creds
        .org_id
        .as_deref()
        .filter(|o| !o.is_empty())
        .ok_or_else(|| anyhow::anyhow!("no org selected"))?;
    let client = crate::api::ApiClient::new(token)?;
    client.get_session_stats(org_id, session_id).await
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    fn claude_args(argv: &[&str]) -> Vec<String> {
        let opts = crate::Options::try_parse_from(argv).expect("parses");
        assert!(
            opts.profile.is_none(),
            "edgee consumed a flag meant for the agent: {argv:?}"
        );
        match opts.command {
            crate::commands::Command::Launch(launch) => match launch.command {
                Command::Claude(c) => c.args,
                other => panic!("wrong target: {other:?}"),
            },
            other => panic!("wrong subcommand: {other:?}"),
        }
    }

    // The invariant: everything after the target name belongs to the agent. An
    // edgee flag that is also live on the target wins against the passthrough
    // when the user puts it first, silently swallowing the agent's own flag of
    // that name — `-p/--profile` did exactly that to Claude Code's `-p/--print`,
    // turning `claude -p "my prompt"` into a switch to a profile named
    // "my prompt". So: keep edgee's flags off the targets, and never `global`.
    //
    // `--relay` is the one exception still declared on a target (claude), which
    // is safe only because Claude Code has no flag of that name. The shims pass
    // `--` before the agent's args, so even that one cannot be shadowed there.
    #[test]
    fn edgee_flags_do_not_shadow_agent_flags() {
        for flag in ["-p", "--profile"] {
            assert_eq!(
                claude_args(&["edgee", "launch", "claude", flag, "my prompt"]),
                [flag, "my prompt"],
            );
        }
    }

    // Both at once: edgee's `-p` ahead of the target and the agent's own `-p`
    // after it. This is the combination `global = true` made unrepresentable.
    #[test]
    fn edgee_and_agent_can_both_use_dash_p() {
        let opts = crate::Options::try_parse_from([
            "edgee", "-p", "staging", "launch", "claude", "-p", "my prompt",
        ])
        .expect("parses");
        assert_eq!(opts.profile.as_deref(), Some("staging"));

        match opts.command {
            crate::commands::Command::Launch(launch) => match launch.command {
                Command::Claude(c) => assert_eq!(c.args, ["-p", "my prompt"]),
                other => panic!("wrong target: {other:?}"),
            },
            other => panic!("wrong subcommand: {other:?}"),
        }
    }

    // The `--` the shims emit is consumed by clap, not forwarded to the agent.
    #[test]
    fn shim_separator_is_not_forwarded_to_the_agent() {
        assert_eq!(claude_args(&["edgee", "launch", "claude", "--"]), [] as [String; 0]);
        assert_eq!(
            claude_args(&["edgee", "launch", "claude", "--", "-p", "my prompt"]),
            ["-p", "my prompt"],
        );
    }

    #[test]
    fn edgee_flags_still_work_ahead_of_the_target() {
        let opts = crate::Options::try_parse_from(["edgee", "-p", "dev", "launch", "claude"])
            .expect("parses");
        assert_eq!(opts.profile.as_deref(), Some("dev"));
    }
}
