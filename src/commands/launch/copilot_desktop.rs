//! GitHub Copilot app through the existing Copilot subscription relay.
//!
//! Verified with macOS app 1.1.20 / bundled Copilot CLI 1.0.84-5:
//! proxy env and NODE_EXTRA_CA_CERTS reach the bundled runtime, including native
//! /responses requests and a successful bash tool round trip. BYOK would replace
//! the user's Copilot subscription; COPILOT_CLI_PATH is ignored in shipped apps.
//! Only local sessions are covered; remote runtimes do not inherit this env.

use std::path::PathBuf;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Arguments forwarded to the GitHub Copilot app
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub(crate) fn binary() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut candidates = vec![PathBuf::from(
            "/Applications/GitHub Copilot.app/Contents/MacOS/github",
        )];
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join(
                "Applications/GitHub Copilot.app/Contents/MacOS/github",
            ));
        }
        candidates.into_iter().find(|path| path.is_file()).ok_or_else(|| {
            anyhow::anyhow!("GitHub Copilot app not found. Install it from https://github.com/features/ai/github-app in /Applications or ~/Applications.")
        })
    }
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("GitHub Copilot desktop launch is currently supported on macOS only. Use `edgee launch copilot-cli` or `edgee launch copilot-vscode` on this platform.")
}

pub async fn run(opts: Options) -> Result<()> {
    // Resolve before login so unsupported platforms and missing installs fail early.
    binary()?;
    crate::commands::relay::run_for_agent_with_args("copilot-desktop", &opts.args).await
}
