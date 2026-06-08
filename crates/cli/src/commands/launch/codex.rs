use anyhow::Result;

use super::util;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Route traffic through a local gateway instead of the hosted Edgee service.
    /// Session tracking is disabled in this mode.
    #[arg(long)]
    pub local_gateway: bool,

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
    if creds
        .codex
        .as_ref()
        .map(|c| c.api_key.is_empty())
        .unwrap_or(true)
    {
        let created = crate::commands::auth::login::ensure_provider_key("codex").await?;
        // First-run onboarding — only when the key was just created.
        if created {
            crate::commands::auth::login::ensure_onboarded("codex").await?;
        }
        creds = crate::config::read()?;
    }

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

    if opts.local_gateway {
        return run_with_local_gateway(opts.args).await;
    }

    util::spawn_cli_version_report(&creds, &session_id);

    let repo_entry = crate::git::detect_origin()
        .map(|url| format!(",\"x-edgee-repo\"=\"{url}\""))
        .unwrap_or_default();
    let base_url = format!("{}/v1", crate::config::gateway_base_url());
    let mut cmd = std::process::Command::new(util::resolve_binary("codex"));
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.args([
        "-c", "model_provider=\"edgee-cli\"",
        "-c", "model_providers.edgee-cli.name=\"EDGEE\"",
        "-c", &format!("model_providers.edgee-cli.base_url=\"{base_url}\""),
        "-c", &format!("model_providers.edgee-cli.http_headers={{\"x-edgee-api-key\"=\"{api_key}\",\"x-edgee-session-id\"=\"{session_id}\"{repo_entry}}}"),
        "-c", "model_providers.edgee-cli.wire_api=\"responses\"",
    ]);
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

/// Launch Codex routed through a local gateway. Session tracking and version
/// reporting are skipped — the backend never sees this traffic.
async fn run_with_local_gateway(args: Vec<String>) -> Result<()> {
    use std::net::Ipv4Addr;

    let log_path = crate::config::local_gateway_log_path();
    crate::local_gateway::init_file_tracing(&log_path)?;
    eprintln!("edgee: gateway logs -> {}", log_path.display());

    let gateway = crate::local_gateway::start((Ipv4Addr::LOCALHOST, 0).into()).await?;
    let addr = gateway.addr;

    let base_url = format!("http://{addr}/v1");
    let mut cmd = tokio::process::Command::new(util::resolve_binary("codex"));
    cmd.args([
        "-c",
        "model_provider=\"edgee-cli\"",
        "-c",
        "model_providers.edgee-cli.name=\"EDGEE\"",
        "-c",
        &format!("model_providers.edgee-cli.base_url=\"{base_url}\""),
        "-c",
        "model_providers.edgee-cli.wire_api=\"responses\"",
    ]);
    cmd.args(&args);

    util::run_with_gateway(
        gateway,
        cmd,
        "Codex CLI is not installed. Install it from https://developers.openai.com/codex/cli",
    )
    .await
}
