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

    // Step 3: launch claude with the correct env vars
    let claude = creds.claude.as_ref().unwrap();
    let api_key = &claude.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();
    let repo_header = crate::git::detect_origin()
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
