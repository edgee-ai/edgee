use anyhow::Result;

use super::util;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the codebuddy CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// Launch CodeBuddy routed through a local Edgee gateway.
///
/// CodeBuddy only supports the local-gateway flow today: its traffic is
/// Anthropic Messages API, so the gateway's `/v1/messages` route handles it
/// directly. Hosted mode is not yet supported because there is no documented
/// way to inject Edgee's `x-edgee-api-key` / `x-edgee-session-id` headers
/// into CodeBuddy's outbound requests.
pub async fn run(opts: Options) -> Result<()> {
    use std::net::Ipv4Addr;

    // First-run: install the persistent user-level statusline integration
    // exactly once. CodeBuddy itself doesn't render an Edgee statusline today,
    // but users typically also use Claude Code in the same shell — running
    // the installer on the first `edgee launch` of any agent matches the
    // "set it up once" flow we want.
    util::ensure_first_run_installed().await;

    let log_path = crate::config::local_gateway_log_path();
    crate::local_gateway::init_file_tracing(&log_path)?;
    eprintln!("edgee: gateway logs -> {}", log_path.display());

    let gateway = crate::local_gateway::start((Ipv4Addr::LOCALHOST, 0).into()).await?;
    let addr = gateway.addr;

    let mut cmd = tokio::process::Command::new(util::resolve_binary("codebuddy"));
    cmd.env("CODEBUDDY_BASE_URL", format!("http://{addr}"));
    cmd.args(&opts.args);

    util::run_with_gateway(
        gateway,
        cmd,
        "CodeBuddy is not installed. Install it from https://cnb.cool/codebuddy/codebuddy-code",
    )
    .await
}
