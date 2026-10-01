//! `edgee statusline kimi enable` / `disable` — toggle the persistent Kimi
//! Code integration without a full uninstall.
//!
//! `disable` writes a marker file at
//! `<global_config_dir>/statusline-kimi.disabled` and clears
//! `status_line.command` in `$KIMI_CODE_HOME/tui.toml`, but only when it's
//! still exactly Edgee's own value — a value the user has since changed is
//! left alone, and `status_line.items` is never touched. `enable` deletes
//! the marker and re-runs the installer.

use anyhow::{Context, Result};
use console::style;
use toml::Value;

use crate::commands::statusline::kimi::install::{self, STATUSLINE_COMMAND};

/// Path of the marker file that records "user explicitly disabled the Kimi
/// statusline integration". Presence is the signal; the file is empty.
pub fn disabled_marker_path() -> std::path::PathBuf {
    crate::config::global_config_dir().join("statusline-kimi.disabled")
}

/// Path of the marker file that records "auto-install on first launch has
/// already happened". Presence is the signal; the file is empty.
pub fn installed_marker_path() -> std::path::PathBuf {
    crate::config::global_config_dir().join("statusline-kimi.installed")
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
    println!("  {} Edgee Kimi statusline enabled.", style("✓").green());
    Ok(())
}

pub async fn disable() -> Result<()> {
    let marker = disabled_marker_path();
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    std::fs::write(&marker, b"").with_context(|| format!("Failed to write {}", marker.display()))?;

    let mut removed = false;
    if let Some(path) = super::tui_toml_path() {
        if path.is_file() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            if let Ok(mut value) = toml::from_str::<Value>(&content) {
                if remove_edgee_status_line(&mut value) {
                    let rendered = toml::to_string_pretty(&value)?;
                    std::fs::write(&path, rendered)
                        .with_context(|| format!("Failed to write {}", path.display()))?;
                    removed = true;
                }
            }
        }
    }

    println!("  {} Edgee Kimi statusline disabled.", style("✓").green());
    if removed {
        println!("    • removed status_line.command");
    }
    println!(
        "  {} Run `edgee statusline kimi enable` to turn it back on.",
        style("→").dim(),
    );
    Ok(())
}

/// Remove `status_line.command` only if it's exactly Edgee's own value.
/// `status_line.items` and every other key survive untouched.
fn remove_edgee_status_line(value: &mut Value) -> bool {
    let Some(table) = value.as_table_mut() else {
        return false;
    };
    let Some(status_line) = table.get_mut("status_line").and_then(Value::as_table_mut) else {
        return false;
    };
    match status_line.get("command").and_then(Value::as_str) {
        Some(cmd) if cmd == STATUSLINE_COMMAND => {
            status_line.remove("command");
            true
        }
        _ => false,
    }
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use crate::commands::claude_settings::env_test_lock as env_lock;
    use std::fs;
    use std::path::PathBuf;

    fn isolate_home(home: &PathBuf) -> impl Drop {
        let prev_home = std::env::var_os("HOME");
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_kimi_home = std::env::var_os("KIMI_CODE_HOME");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::remove_var("KIMI_CODE_HOME");
        }
        struct Restore {
            home: Option<std::ffi::OsString>,
            xdg: Option<std::ffi::OsString>,
            kimi_home: Option<std::ffi::OsString>,
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
                    match &self.kimi_home {
                        Some(p) => std::env::set_var("KIMI_CODE_HOME", p),
                        None => std::env::remove_var("KIMI_CODE_HOME"),
                    }
                }
            }
        }
        Restore { home: prev_home, xdg: prev_xdg, kimi_home: prev_kimi_home }
    }

    fn fresh_home() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        (tmp, home)
    }

    fn read_tui_toml(home: &std::path::Path) -> Value {
        let p = home.join(".kimi-code").join("tui.toml");
        let s = fs::read_to_string(p).unwrap();
        toml::from_str(&s).unwrap()
    }

    #[tokio::test]
    async fn disable_then_enable_round_trips() {
        let (_tmp, home) = fresh_home();
        let _lock = env_lock();
        let _h = isolate_home(&home);

        install::run(install::Options::default()).await.unwrap();
        let installed = read_tui_toml(&home);
        assert_eq!(
            installed["status_line"]["command"].as_str(),
            Some("edgee statusline render")
        );

        disable().await.unwrap();
        assert!(disabled_marker_path().is_file());
        let after_disable = read_tui_toml(&home);
        assert!(after_disable["status_line"].get("command").is_none());

        enable().await.unwrap();
        assert!(!disabled_marker_path().is_file());
        let after_enable = read_tui_toml(&home);
        assert_eq!(
            after_enable["status_line"]["command"].as_str(),
            Some("edgee statusline render")
        );
    }

    #[tokio::test]
    async fn disable_leaves_third_party_command_and_items_alone() {
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".kimi-code")).unwrap();
        fs::write(
            home.join(".kimi-code").join("tui.toml"),
            "[status_line]\nitems = [\"mode\", \"model\"]\ncommand = \"/path/to/user-custom.sh\"\n",
        )
        .unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);

        disable().await.unwrap();

        let v = read_tui_toml(&home);
        assert_eq!(
            v["status_line"]["command"].as_str(),
            Some("/path/to/user-custom.sh")
        );
        assert_eq!(v["status_line"]["items"].as_array().unwrap().len(), 2);
        assert!(disabled_marker_path().is_file());
    }
}
