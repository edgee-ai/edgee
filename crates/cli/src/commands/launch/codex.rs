use anyhow::Result;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the codex CLI
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

    // Step 2: ensure we have an api_key for Codex
    if creds.codex.as_ref().map(|c| c.api_key.is_empty()).unwrap_or(true) {
        crate::commands::auth::login::ensure_provider_key("codex").await?;
        creds = crate::config::read()?;
    }

    // Step 3: ensure we have a connection choice (default to "plan" for codex)
    if creds.codex.as_ref().and_then(|c| c.connection.as_deref()).is_none() {
        let provider = creds.codex.get_or_insert_with(Default::default);
        provider.connection = Some("plan".to_string());
        crate::config::write(&creds)?;
    }

    // Step 3b: ensure MCP preference is set
    crate::commands::auth::login::ensure_mcp_preference().await?;
    creds = crate::config::read()?;

    // Step 4: launch codex with the correct env vars
    let codex = creds.codex.as_ref().unwrap();
    let api_key = &codex.api_key;
    let session_id = uuid::Uuid::new_v4().to_string();
    crate::commands::launch::spawn_cli_version_report(&creds, &session_id);
    let repo_origin = crate::git::detect_origin();
    let repo_entry = repo_origin
        .as_ref()
        .map(|url| format!(",\"x-edgee-repo\"=\"{}\"", url))
        .unwrap_or_default();
    let base_url = format!("{}/v1", crate::config::gateway_base_url());
    let mut cmd = std::process::Command::new(crate::commands::launch::resolve_binary("codex"));
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.args([
        "-c", "model_provider=\"edgee-cli\"",
        "-c", "model_providers.edgee-cli.name=\"EDGEE\"",
        "-c", &format!("model_providers.edgee-cli.base_url=\"{base_url}\""),
        "-c", &format!("model_providers.edgee-cli.http_headers={{\"x-edgee-api-key\"=\"{api_key}\",\"x-edgee-session-id\"=\"{session_id}\"{repo_entry}}}"),
        "-c", "model_providers.edgee-cli.wire_api=\"responses\"",
    ]);

    // Step 5: conditionally set up MCP integration
    let use_mcp = creds.enable_mcp.unwrap_or(false);
    let user_token = creds.user_token.as_deref().unwrap_or("");
    if use_mcp && !user_token.is_empty() {
        cmd.env("EDGEE_USER_TOKEN", user_token);
        let session_url = format!("{}/session/{}", crate::config::console_base_url(), session_id);
        let prompt = crate::commands::launch::agent_session_prompt(
            &session_id,
            repo_origin.as_deref(),
            &session_url,
        );
        let escaped_prompt = crate::commands::launch::toml_escape_string(&prompt);
        cmd.args([
            "-c",
            &format!(
                "mcp_servers.edgee.url=\"{}\"",
                crate::config::mcp_base_url()
            ),
            "-c",
            "mcp_servers.edgee.bearer_token_env_var=\"EDGEE_USER_TOKEN\"",
            "-c",
            &format!("developer_instructions={escaped_prompt}"),
        ]);
    }

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

    crate::commands::launch::print_session_stats(&creds, &session_id, "Codex").await;

    if let Some(code) = status.code() {
        std::process::exit(code);
    }

    Ok(())
}
