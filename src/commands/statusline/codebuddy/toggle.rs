//! `edgee statusline codebuddy enable` / `disable` — toggle the persistent
//! CodeBuddy integration without a full uninstall.
//!
//! `disable` writes a marker file at
//! `<global_config_dir>/statusline-codebuddy.disabled` and strips the Edgee
//! `statusLine` from `~/.codebuddy/settings.json`, but only when it's still
//! exactly Edgee's own command — a `statusLine` the user has since changed
//! to something else is left alone. `enable` deletes the marker and re-runs
//! the installer.

use anyhow::{Context, Result};
use console::style;
use serde_json::Value;

use crate::commands::claude_settings;
use crate::commands::statusline::codebuddy::install::{self, STATUSLINE_COMMAND};

/// Path of the marker file that records "user explicitly disabled the
/// CodeBuddy statusline integration". Presence is the signal; the file is
/// empty.
pub fn disabled_marker_path() -> std::path::PathBuf {
    crate::config::global_config_dir().join("statusline-codebuddy.disabled")
}

/// Path of the marker file that records "auto-install on first launch has
/// already happened". Presence is the signal; the file is empty.
pub fn installed_marker_path() -> std::path::PathBuf {
    crate::config::global_config_dir().join("statusline-codebuddy.installed")
}

pub fn is_disabled() -> bool {
    disabled_marker_path().is_file()
}

pub async fn enable() -> Result<()> {
    let marker = disabled_marker_path();
    if marker.exists() {
        std::fs::remove_file(&marker)
            .with_context(|| format!("Failed to remove {}", marker.display()))?;
    }
    install::run(install::Options::default()).await?;
    println!("  {} Edgee CodeBuddy statusline enabled.", style("✓").green());
    Ok(())
}

pub async fn disable() -> Result<()> {
    let marker = disabled_marker_path();
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    std::fs::write(&marker, b"").with_context(|| format!("Failed to write {}", marker.display()))?;

    let path = install::settings_path();
    let mut removed = false;
    if path.is_file() {
        let mut value = claude_settings::read_settings(&path)?.value;
        if remove_edgee_status_line(&mut value) {
            claude_settings::write_settings(&path, &value)?;
            removed = true;
        }
    }

    println!("  {} Edgee CodeBuddy statusline disabled.", style("✓").green());
    if removed {
        println!("    • removed Edgee statusLine");
    }
    println!(
        "  {} Run `edgee statusline codebuddy enable` to turn it back on.",
        style("→").dim(),
    );
    Ok(())
}

/// Remove the top-level `statusLine` block only if its command is exactly
/// Edgee's own — a user-modified or third-party value is left untouched.
fn remove_edgee_status_line(value: &mut Value) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    let Some(sl) = obj.get("statusLine") else {
        return false;
    };
    if claude_settings::status_line_command(sl) != Some(STATUSLINE_COMMAND) {
        return false;
    }
    obj.remove("statusLine");
    true
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::commands::claude_settings::env_test_lock as env_lock;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    fn isolate_home(home: &PathBuf) -> impl Drop {
        let prev_home = std::env::var_os("HOME");
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        struct Restore {
            home: Option<std::ffi::OsString>,
            xdg: Option<std::ffi::OsString>,
        }
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    match &self.home {
                        Some(p) => std::env::set_var("HOME", p),
                        None => std::env::remove_var("HOME"),
                    }
                    match &self.xdg {
                        Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
                        None => std::env::remove_var("XDG_CONFIG_HOME"),
                    }
                }
            }
        }
        Restore { home: prev_home, xdg: prev_xdg }
    }

    fn fresh_home() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        (tmp, home)
    }

    fn read_settings_file(home: &std::path::Path) -> Value {
        let p = home.join(".codebuddy").join("settings.json");
        let s = fs::read_to_string(p).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[tokio::test]
    async fn disable_then_enable_round_trips() {
        let (_tmp, home) = fresh_home();
        let _lock = env_lock();
        let _h = isolate_home(&home);

        install::run(install::Options::default()).await.unwrap();
        let installed = read_settings_file(&home);
        assert_eq!(installed["statusLine"]["command"], "edgee statusline render");

        disable().await.unwrap();
        assert!(disabled_marker_path().is_file());
        let after_disable = read_settings_file(&home);
        assert!(after_disable.get("statusLine").is_none());

        enable().await.unwrap();
        assert!(!disabled_marker_path().is_file());
        let after_enable = read_settings_file(&home);
        assert_eq!(after_enable["statusLine"]["command"], "edgee statusline render");
    }

    #[tokio::test]
    async fn disable_leaves_third_party_statusline_alone() {
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".codebuddy")).unwrap();
        fs::write(
            home.join(".codebuddy").join("settings.json"),
            serde_json::to_string_pretty(&json!({
                "statusLine": {"type": "command", "command": "/path/to/user-custom.sh"}
            }))
            .unwrap(),
        )
        .unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);

        disable().await.unwrap();

        let v = read_settings_file(&home);
        assert_eq!(v["statusLine"]["command"], "/path/to/user-custom.sh");
        assert!(disabled_marker_path().is_file());
    }
}
