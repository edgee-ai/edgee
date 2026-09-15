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

pub async fn run(opts: Options) -> Result<()> {
    // Fail before authentication if there is no IDE to launch.
    binary()?;
    crate::commands::relay::run_for_agent_with_args("intellij", &opts.args).await
}

/// Launch the executable directly so the Copilot plugin inherits proxy and CA
/// variables. `open -a` can reuse an instance with the old environment.
pub(crate) fn binary() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("EDGEE_INTELLIJ_BINARY") {
        let path = PathBuf::from(path);
        anyhow::ensure!(path.is_file(), "EDGEE_INTELLIJ_BINARY must point to the IntelliJ IDEA executable.");
        return Ok(path);
    }

    #[cfg(target_os = "macos")]
    {
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Applications"));
        }
        if let Some(path) = macos_binary(&roots) {
            return Ok(path);
        }
    }

    // Toolbox installations can expose their launcher on PATH. Other install
    // locations can be selected explicitly with EDGEE_INTELLIJ_BINARY.
    let names: &[&str] = if cfg!(target_os = "windows") {
        &["idea64.exe", "idea.exe"]
    } else {
        &["idea", "idea.sh"]
    };
    names.iter().find_map(|name| {
        let resolved = super::util::resolve_binary(name);
        if resolved == std::ffi::OsStr::new(name) {
            return None;
        }
        let path = PathBuf::from(resolved);
        path.is_file().then_some(path)
    }).context(
        "IntelliJ IDEA not found. Install it and add its launcher to PATH, or set EDGEE_INTELLIJ_BINARY to its executable path.",
    )
}

#[cfg(any(target_os = "macos", test))]
fn macos_binary(roots: &[PathBuf]) -> Option<PathBuf> {
    roots.iter().flat_map(|root| {
        ["IntelliJ IDEA.app", "IntelliJ IDEA CE.app", "IntelliJ IDEA Ultimate.app"]
            .map(|bundle| root.join(bundle).join("Contents/MacOS/idea"))
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
        assert_eq!(macos_binary(&[system, user]), Some(executable));
    }

    #[test]
    fn does_not_detect_an_empty_app_bundle_as_installed() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("IntelliJ IDEA.app")).unwrap();
        assert_eq!(macos_binary(&[tmp.path().to_path_buf()]), None);
    }
}
