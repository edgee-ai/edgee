use anyhow::Result;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Skip installing the Edgee status line
    #[arg(long)]
    pub no_statusline: bool,
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

    // Step 3b: ensure MCP preference is set
    crate::commands::auth::login::ensure_mcp_preference().await?;
    creds = crate::config::read()?;

    // Step 4: launch claude with the correct env vars
    let claude = creds.claude.as_ref().unwrap();
    let api_key = &claude.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();
    crate::commands::launch::spawn_cli_version_report(&creds, &session_id);
    let repo_origin = crate::git::detect_origin();
    let repo_header = repo_origin
        .as_ref()
        .map(|url| format!("\nx-edgee-repo: {}", url))
        .unwrap_or_default();

    // Install Edgee status line (best-effort); guard restores it on drop.
    let _statusline_guard = if opts.no_statusline {
        None
    } else {
        crate::commands::launch::statusline::install(
            &session_id,
            &crate::config::console_api_base_url(),
        )
        .ok()
    };
    let mut cmd = std::process::Command::new(crate::commands::launch::resolve_binary("claude"));

    cmd.env("ANTHROPIC_BASE_URL", crate::config::gateway_base_url());
    cmd.env(
        "ANTHROPIC_CUSTOM_HEADERS",
        format!("x-edgee-api-key: {}\nx-edgee-session-id: {}{}", api_key, session_id, repo_header),
    );
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env("EDGEE_CONSOLE_API_URL", crate::config::console_api_base_url());

    // Step 5: conditionally set up MCP integration
    let use_mcp = creds.enable_mcp.unwrap_or(false);
    if use_mcp {
        let mcp_config_path = write_mcp_config(&creds)?;
        cmd.arg("--mcp-config").arg(&mcp_config_path);
        let session_url = format!("{}/session/{}", crate::config::console_base_url(), session_id);
        cmd.arg("--append-system-prompt").arg(crate::commands::launch::agent_session_prompt(&session_id, repo_origin.as_deref(), &session_url));
        cmd.arg("--allowedTools").arg("mcp__edgee__setSessionName,mcp__edgee__addSessionPullRequest,mcp__edgee__addSessionCommit,mcp__edgee__setSessionGitHubRepo");
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
