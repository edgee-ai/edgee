use anyhow::Result;

use super::util;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the codebuddy CLI
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

    let repo_entry = crate::git::detect_origin()
        .map(|url| format!(",\"x-edgee-repo\"=\"{url}\""))
        .unwrap_or_default();
    let base_url = format!("{}/v1", super::resolve_gateway_base_url(&creds).await);
    let mut cmd = std::process::Command::new(util::resolve_binary("codebuddy"));
    cmd.env("EDGEE_SESSION_ID", &session_id);
    cmd.env("CODEBUDDY_BASE_URL", &base_url);
    cmd.env(
        "CODEBUDDY_CUSTOM_HEADERS",
        format!(
            "x-edgee-api-key: {api_key}\nx-edgee-session-id: {session_id}{repo_entry}"
        ),
    );
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
