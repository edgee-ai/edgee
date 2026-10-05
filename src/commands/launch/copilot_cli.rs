use anyhow::Result;

use crate::commands::relay::AgentExtras;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Extra args passed through to the copilot CLI
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub async fn run(opts: Options, reroute: &super::reroute::Reroute) -> Result<()> {
    crate::commands::relay::run_for_agent_with_args("copilot-cli", &opts.args, reroute).await
}

/// State a Copilot launch keeps alive until [`CopilotSession::finish`].
pub(crate) struct CopilotSession;

impl CopilotSession {
    pub(crate) async fn finish(self, session_id: &str) {
        if let Ok(creds) = crate::config::read() {
            super::print_session_stats(&creds, session_id, "Copilot").await;
        }
    }
}

/// Builds the Copilot-specific part of a relayed launch. Runs after the relay
/// created `session_id`. `interactive` is false under
/// `edgee relay --non-interactive`, which must never prompt.
pub(crate) async fn prepare(
    _session_id: &str,
    _interactive: bool,
) -> Result<(AgentExtras, CopilotSession)> {
    let mut extras = AgentExtras::default();

    // The statusline renderer reads this to reach the active profile's API.
    extras
        .env
        .push(("EDGEE_CONSOLE_API_URL", crate::config::console_api_base_url().into()));

    Ok((extras, CopilotSession))
}
