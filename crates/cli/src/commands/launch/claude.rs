use anyhow::Result;

#[derive(Debug, clap::Parser)]
pub struct Options {
    /// Extra args passed through to the claude CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub async fn run(opts: Options) -> Result<()> {
    let mut creds = crate::config::read()?;

    // Step 1: ensure we are authenticated
    if creds.user_token.as_deref().unwrap_or("").is_empty() {
        crate::commands::auth::login::perform_login().await?;
    }

    // Step 1b: ensure an org is selected (handles partial state after aborted login)
    crate::commands::auth::login::ensure_org_selected().await?;
    creds = crate::config::read()?;

    // Step 2: ensure we have an api_key for Claude
    if creds.claude.as_ref().map(|c| c.api_key.is_empty()).unwrap_or(true) {
        crate::commands::auth::login::ensure_provider_key("claude").await?;
        creds = crate::config::read()?;
    }

    // Step 3: ensure we have a connection choice (default to "plan")
    if creds.claude.as_ref().and_then(|c| c.connection.as_deref()).is_none() {
        let provider = creds.claude.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    // Step 4: write MCP config with auth token
    let mcp_config_path = write_mcp_config(&creds)?;

    // Step 5: launch claude with the correct env vars
    let claude = creds.claude.as_ref().unwrap();
    let api_key = &claude.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();
    let repo_origin = crate::git::detect_origin();
    let repo_header = repo_origin
        .as_ref()
        .map(|url| format!("\nx-edgee-repo: {}", url))
        .unwrap_or_default();

    // Install Edgee status line (best-effort); guard restores it on drop.
    let _statusline_guard = crate::commands::launch::statusline::install(
        &session_id,
        &crate::config::console_api_base_url(),
    )
    .ok();
    let mut cmd = std::process::Command::new(crate::commands::launch::resolve_binary("claude"));

    cmd.env("ANTHROPIC_BASE_URL", crate::config::gateway_base_url());
    cmd.env(
        "ANTHROPIC_CUSTOM_HEADERS",
        format!("x-edgee-api-key: {}\nx-edgee-session-id: {}{}", api_key, session_id, repo_header),
    );
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env("EDGEE_CONSOLE_API_URL", crate::config::console_api_base_url());

    cmd.arg("--mcp-config").arg(&mcp_config_path);
    let session_url = format!("{}/session/{}", crate::config::console_base_url(), session_id);
    cmd.arg("--append-system-prompt").arg(system_prompt(&session_id, repo_origin.as_deref(), &session_url));
    cmd.arg("--allowedTools").arg("mcp__edgee__setSessionName,mcp__edgee__addSessionPullRequest,mcp__edgee__setSessionGitHubRepo");
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

    // Restore previous status line setting
    drop(_statusline_guard);

    crate::commands::launch::print_session_stats(&creds, &session_id, "Claude").await;

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
                    "Authorization": format!("Bearer {}", token)
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

2. `addSessionPullRequest` — call this EVERY TIME you open a pull request (e.g., via `gh pr create`). Immediately after the PR is created, call this tool with:
   - sessionId: "{session_id}"
   - pullRequest: the full PR URL returned by `gh pr create`.
   This is required for every PR you open during this session, with no exceptions.

PULL REQUEST DESCRIPTION REQUIREMENT (mandatory, no exceptions):
Whenever you create OR edit a pull request (via `gh pr create`, `gh pr edit`, or any other tool), you MUST include the Edgee session link in the PR description/body. Add the following block at the very top of the PR body, before any other section:

---
Built with [Edgee](https://www.edgee.ai) — [View session]({session_url})

![Edgee session]({session_url}/og)
---

The `/og` path returns an OpenGraph image for the session. When you edit an existing PR to add commits or update the description, preserve this block. If it is missing from a PR you are editing, add it back."#
    );

    if let Some(repo) = repo {
        prompt.push_str(&format!(
            "\n\n3. `setSessionGitHubRepo` — call this EXACTLY ONCE at the start of the session, together with (or right after) `setSessionName`. Arguments:\n   - sessionId: \"{session_id}\"\n   - repo: \"{repo}\"\n   Do not call this tool again during the session."
        ));
    }

    prompt
}
