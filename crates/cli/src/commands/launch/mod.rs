pub mod claude;
pub mod codex;
pub mod crush;
pub mod opencode;
mod util;

use anyhow::Result;
use console::style;

use super::util::session_log;

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Launch Claude Code routed through Edgee
    Claude(claude::Options),
    /// Launch Codex routed through Edgee
    Codex(codex::Options),
    /// Launch OpenCode routed through Edgee
    #[command(name = "opencode")]
    OpenCode(opencode::Options),
    /// Launch Crush routed through Edgee
    Crush(crush::Options),
}

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    command: Command,
}

pub async fn run(opts: Options) -> anyhow::Result<()> {
    match opts.command {
        Command::Claude(o) => claude::run(o).await,
        Command::Codex(o) => codex::run(o).await,
        Command::OpenCode(o) => opencode::run(o).await,
        Command::Crush(o) => crush::run(o).await,
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

/// Resolves the gateway base URL for a launch.
///
/// Precedence (highest first):
/// 1. `EDGEE_API_URL` env var — the explicit escape hatch (local debugging,
///    incident response). It outranks the server so an operator can always
///    force a value.
/// 2. The org's console-configured `gateway_api_url` — central, admin-managed
///    default for the normal case.
/// 3. The active profile's persisted `gateway_url`.
/// 4. The built-in default.
///
/// The org fetch is best-effort: any failure falls through to the next source
/// so launch never breaks (offline, no org selected, or no configured gateway).
pub async fn resolve_gateway_base_url(creds: &crate::config::Credentials) -> String {
    if let Some(env_url) = crate::config::gateway_url_env_override() {
        return env_url;
    }

    if let (Some(token), Some(org_id)) = (
        creds.user_token.as_deref().filter(|t| !t.is_empty()),
        creds.org_id.as_deref().filter(|o| !o.is_empty()),
    ) {
        if let Ok(client) = crate::api::ApiClient::new(token) {
            if let Ok(org) = client.get_organization(org_id).await {
                if let Some(url) = org.gateway_url.filter(|s| !s.is_empty()) {
                    return url;
                }
            }
        }
    }

    if let Some(profile_url) = crate::config::gateway_url_profile_override() {
        return profile_url;
    }

    crate::config::DEFAULT_GATEWAY_URL.to_string()
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
