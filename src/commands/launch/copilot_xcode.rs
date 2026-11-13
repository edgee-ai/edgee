//! GitHub Copilot in Xcode's coding assistant through the Copilot subscription relay.
//!
//! Xcode 27.3 beta (27C5020n) runs Copilot as a child process: it downloads
//! Copilot CLI into `~/Library/Developer/Xcode/CodingAssistant/Agents/` and spawns
//! `copilot --acp --plugin-dir …/xcode-integration` with its own `COPILOT_HOME`.
//! The child inherits Xcode's environment, so launching the Xcode executable
//! directly with proxy env reaches it — the same shape as `copilot-desktop`.
//! Xcode chooses the CLI's arguments and `COPILOT_HOME` itself, so Edgee cannot
//! add flags (MCP, plugins) to it; BYOK would replace the user's subscription.

use std::path::PathBuf;

use anyhow::Result;

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Project paths and arguments forwarded to Xcode
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

const OVERRIDE_ENV: &str = "EDGEE_XCODE_BINARY";

/// Copilot ships in Xcode 27.3 beta first, so the beta bundle wins over a
/// release Xcode installed alongside it.
#[cfg(any(target_os = "macos", test))]
const BUNDLES: &[&str] = &["Xcode-beta.app", "Xcode.app"];

pub(crate) fn binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(OVERRIDE_ENV) {
        let path = PathBuf::from(path);
        anyhow::ensure!(
            path.is_file(),
            "{OVERRIDE_ENV} must point to the Xcode executable (Xcode-beta.app/Contents/MacOS/Xcode)."
        );
        return Ok(path);
    }
    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
        macos_binary(&roots).ok_or_else(|| {
            anyhow::anyhow!(
                "Xcode not found in /Applications or ~/Applications. Install Xcode 27.3 beta or later there, or set {OVERRIDE_ENV} to its Contents/MacOS/Xcode executable."
            )
        })
    }
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("Xcode is available on macOS only.")
}

#[cfg(any(target_os = "macos", test))]
fn macos_binary(roots: &[PathBuf]) -> Option<PathBuf> {
    BUNDLES
        .iter()
        .flat_map(|bundle| roots.iter().map(move |root| root.join(bundle).join("Contents/MacOS/Xcode")))
        .find(|path| path.is_file())
}

pub async fn run(opts: Options, reroute: &super::reroute::Reroute) -> Result<()> {
    // Resolve before login so missing installs fail early.
    binary()?;
    crate::commands::relay::run_for_agent_with_args("copilot-xcode", &opts.args, reroute).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(root: &std::path::Path, bundle: &str) -> PathBuf {
        let exe = root.join(bundle).join("Contents/MacOS/Xcode");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        exe
    }

    #[test]
    fn prefers_beta_over_release() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path(), "Xcode.app");
        let beta = install(tmp.path(), "Xcode-beta.app");
        assert_eq!(macos_binary(&[tmp.path().to_path_buf()]), Some(beta));
    }

    #[test]
    fn finds_release_in_user_applications() {
        let tmp = tempfile::tempdir().unwrap();
        let system = tmp.path().join("system");
        let release = install(&tmp.path().join("user"), "Xcode.app");
        assert_eq!(macos_binary(&[system, tmp.path().join("user")]), Some(release));
    }

    #[test]
    fn empty_bundle_is_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("Xcode-beta.app")).unwrap();
        assert_eq!(macos_binary(&[tmp.path().to_path_buf()]), None);
    }
}
