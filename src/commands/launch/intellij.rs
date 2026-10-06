//! IntelliJ IDEA's GitHub Copilot plugin through the Copilot subscription relay.

use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, clap::Parser)]
#[command(disable_help_flag = true)]
pub struct Options {
    /// Project paths and arguments forwarded to IntelliJ IDEA
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

pub async fn run(opts: Options, reroute: &super::reroute::Reroute) -> Result<()> {
    // Fail before authentication if there is no IDE to launch.
    binary()?;
    crate::commands::relay::run_for_agent_with_args("intellij", &opts.args, reroute).await
}

/// Launch the executable directly so the Copilot plugin inherits proxy and CA
/// variables. `open -a` can reuse an instance with the old environment.
pub(crate) fn binary() -> Result<PathBuf> {
    jetbrains_binary("IntelliJ IDEA", "EDGEE_INTELLIJ_BINARY", &["IntelliJ IDEA.app", "IntelliJ IDEA CE.app", "IntelliJ IDEA Ultimate.app"], "idea")
}

pub(crate) fn jetbrains_binary(product: &str, override_env: &str, bundles: &[&str], launcher: &str) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(override_env) {
        let path = PathBuf::from(path);
        anyhow::ensure!(path.is_file(), "{override_env} must point to the {product} executable.");
        return Ok(path);
    }

    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
        if let Some(path) = macos_binary(&roots, bundles, launcher) {
            return Ok(path);
        }
    }

    // Toolbox installations can expose their launcher on PATH. Other install
    // locations can be selected explicitly with the product override variable.
    #[cfg(not(target_os = "macos"))]
    let _ = bundles;
    let names = if cfg!(target_os = "windows") {
        vec![format!("{launcher}64.exe"), format!("{launcher}.exe")]
    } else {
        vec![launcher.to_string(), format!("{launcher}.sh")]
    };
    names.iter().find_map(|name| {
        let resolved = super::util::resolve_binary(name);
        if resolved == std::ffi::OsStr::new(name.as_str()) {
            return None;
        }
        let path = PathBuf::from(resolved);
        path.is_file().then_some(path)
    }).with_context(|| format!(
        "{product} not found. Install it and add its launcher to PATH, or set {override_env} to its executable path."
    ))
}

#[cfg(any(target_os = "macos", test))]
fn macos_binary(roots: &[PathBuf], bundles: &[&str], launcher: &str) -> Option<PathBuf> {
    roots.iter().flat_map(|root| {
        bundles.iter().map(move |bundle| root.join(bundle).join("Contents/MacOS").join(launcher))
    }).find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_community_installation_in_user_applications() {
        let tmp = tempfile::tempdir().unwrap();
        let system = tmp.path().join("system");
        let user = tmp.path().join("user");
        let executable = user.join("IntelliJ IDEA CE.app/Contents/MacOS/idea");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "").unwrap();
        assert_eq!(macos_binary(&[system, user], &["IntelliJ IDEA CE.app"], "idea"), Some(executable));
    }

    #[test]
    fn finds_phpstorm_installation() {
        let tmp = tempfile::tempdir().unwrap();
        let executable = tmp.path().join("PhpStorm.app/Contents/MacOS/phpstorm");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "").unwrap();
        assert_eq!(macos_binary(&[tmp.path().to_path_buf()], &["PhpStorm.app", "PHPStorm.app"], "phpstorm"), Some(executable));
    }

    #[test]
    fn does_not_detect_an_empty_app_bundle_as_installed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("IntelliJ IDEA.app")).unwrap();
        assert_eq!(macos_binary(&[tmp.path().to_path_buf()], &["IntelliJ IDEA.app"], "idea"), None);
    }
}
