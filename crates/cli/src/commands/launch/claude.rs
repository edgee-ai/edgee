use anyhow::Result;

use super::util;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Launch through a local relay (MITM) proxy — same as `edgee relay claude`.
    #[arg(long)]
    pub relay: bool,

    /// Extra args passed through to the claude CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    if opts.relay {
        return crate::commands::relay::run_for_agent("claude").await;
    }

    let mut creds = crate::config::read()?;

    // Step 1: ensure we are authenticated
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }

    // Step 1b: ensure an org is selected (handles partial state after aborted login)
    crate::commands::auth::login::ensure_org_selected().await?;

    // Step 2: ensure we have a live api_key for Claude. Re-provisions if the
    // cached key was deleted in the console; re-runs onboarding for a fresh key.
    let reprovisioned = crate::commands::auth::login::ensure_valid_provider_key("claude").await?;
    if reprovisioned {
        crate::commands::auth::login::ensure_onboarded("claude").await?;
    }
    creds = crate::config::read()?;

    // Step 3: ensure we have a connection choice (default to "plan")
    if creds
        .claude
        .as_ref()
        .and_then(|c| c.connection.as_deref())
        .is_none()
    {
        let provider = creds.claude.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    // Step 3b: ensure MCP preference is set
    crate::commands::auth::login::ensure_mcp_preference().await?;
    creds = crate::config::read()?;

    // Step 4: launch claude with the correct env vars
    let claude = creds.claude.as_ref().unwrap();
    let api_key = &claude.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();
    let repo_origin = crate::git::detect_origin();
    let repo_header = repo_origin
        .as_ref()
        .map(|url| format!("\nx-edgee-repo: {url}"))
        .unwrap_or_default();

    // First-run: install the persistent user-level statusline integration
    // exactly once (honors the disable marker).
    util::ensure_first_run_installed().await;

    util::spawn_cli_version_report(&creds, &session_id);

    let gateway_url = super::resolve_gateway_base_url(&creds).await;
    let mut cmd = std::process::Command::new(util::resolve_binary("claude"));
    cmd.env("ANTHROPIC_BASE_URL", &gateway_url);
    cmd.env(
        "ANTHROPIC_CUSTOM_HEADERS",
        format!("x-edgee-api-key: {api_key}\nx-edgee-session-id: {session_id}{repo_header}"),
    );
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env(
        "EDGEE_CONSOLE_API_URL",
        crate::config::console_api_base_url(),
    );

    // Step 5: conditionally set up MCP integration
    let use_mcp = creds.enable_mcp.unwrap_or(false);
    if use_mcp {
        let mcp_config_path = write_mcp_config(&creds)?;
        cmd.arg("--mcp-config").arg(&mcp_config_path);
        let session_url = match creds.org_slug.as_deref() {
            Some(slug) if !slug.is_empty() => {
                format!("{}/sessions/{slug}/{session_id}", crate::config::console_base_url())
            }
            _ => format!("{}/sessions/{session_id}", crate::config::console_base_url()),
        };
        cmd.arg("--append-system-prompt").arg(system_prompt(
            &session_id,
            repo_origin.as_deref(),
            &session_url,
        ));
        cmd.arg("--allowedTools").arg("mcp__edgee__setSessionName,mcp__edgee__addSessionPullRequest,mcp__edgee__addSessionCommit,mcp__edgee__setSessionGitRepo");
    }

    cmd.args(&opts.args);

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!(
                "Claude Code is not installed. Install it from https://code.claude.com/docs/en/quickstart"
            )
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    super::print_session_stats(&creds, &session_id, "Claude").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}

/// Writes an MCP config file to the Edgee config directory with the user's auth token.
/// Returns the path to the written file.
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
    let path = dir.join("mcp.json");
    std::fs::write(&path, serde_json::to_string_pretty(&mcp_config)?)?;
    Ok(path)
}

fn system_prompt(session_id: &str, repo: Option<&str>, session_url: &str) -> String {
    let mut prompt = format!(
        r#"You are running inside the Edgee CLI and have access to the Edgee MCP server for tracking session metadata.

Your Edgee session ID is: {session_id}
Your Edgee public session page is: {session_url}

You MUST use the following Edgee MCP tools during this session:

1. `setSessionName` — call this immediately after the user's first message with a short descriptive name (3-6 words) summarizing what the user is asking for. Arguments:
   - sessionId: "{session_id}"
   - name: the descriptive name.
   If at any later point during the session you come up with a clearly better name (e.g., the task's real scope becomes obvious only after exploring the code, or the user pivots the request), call `setSessionName` again with the improved name. Prefer calling it once, but do not hesitate to update when a materially better name emerges.

2. `addSessionPullRequest` — call this EVERY TIME you create OR edit a pull request (e.g., via `gh pr create`, `gh pr edit`, or any other tool). Immediately after the PR is created or modified, call this tool with:
   - sessionId: "{session_id}"
   - pullRequest: the full PR URL.
   This is required for every PR you touch during this session, with no exceptions. Always call it on edits too — the PR may not yet be associated with this session, and the API handles duplicates safely, so redundant calls are harmless."#
    );

    if let Some(repo) = repo {
        prompt.push_str(&format!(
            "\n\n3. `setSessionGitRepo` — call this EXACTLY ONCE at the start of the session, together with (or right after) `setSessionName`. Arguments:\n   - sessionId: \"{session_id}\"\n   - repo: \"{repo}\"\n   Do not call this tool again during the session."
        ));
    }

    prompt
}
