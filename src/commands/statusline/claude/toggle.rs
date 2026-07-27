//! `edgee statusline claude enable` / `disable` — toggle the persistent
//! Claude Code integration without requiring a full uninstall.
//!
//! `disable` writes a marker file at `<global_config_dir>/statusline-claude.disabled`
//! and strips the Edgee `statusLine` + `SessionStart` hook from
//! `~/.claude/settings.json`. The marker is read by the launch flow to skip
//! the per-launch transient install too. `enable` deletes the marker and
//! re-runs the installer.
//!
//! `disable` only removes a `statusLine` we recognize as ours
//! ([`CommandKind::Edgee`] / [`EdgeeWrap`]) and only removes hook entries
//! whose command matches our marker — third-party config is left untouched.

use anyhow::{Context, Result};
use console::style;
use serde_json::Value;

use crate::commands::claude_settings::{self, CommandKind};
use crate::commands::statusline::claude::install;

const HOOK_NEW_MARKER: &str = "edgee statusline claude doctor";
const HOOK_LEGACY_MARKER: &str = "edgee doctor";

/// Path of the marker file that records "user explicitly disabled the
/// statusline integration". Presence is the signal; the file is empty.
pub fn disabled_marker_path() -> std::path::PathBuf {
    crate::config::global_config_dir().join("statusline-claude.disabled")
}

/// Path of the marker file that records "auto-install on first launch has
/// already happened". Presence is the signal; the file is empty.
pub fn installed_marker_path() -> std::path::PathBuf {
    crate::config::global_config_dir().join("statusline-claude.installed")
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
    println!(
        "  {} Edgee statusline enabled.",
        style("✓").green(),
    );
    Ok(())
}

pub async fn disable() -> Result<()> {
    let marker = disabled_marker_path();
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    std::fs::write(&marker, b"")
        .with_context(|| format!("Failed to write {}", marker.display()))?;

    let path = claude_settings::user_settings_path();
    let mut changes: Vec<&str> = Vec::new();
    if path.is_file() {
        let mut value = claude_settings::read_settings(&path)?.value;
        let mut dirty = false;
        if remove_edgee_status_line(&mut value) {
            dirty = true;
            changes.push("removed Edgee statusLine");
        }
        if remove_edgee_session_start_hook(&mut value)? {
            dirty = true;
            changes.push("removed SessionStart hook");
        }
        if dirty {
            claude_settings::write_settings(&path, &value)?;
        }
    }

    println!(
        "  {} Edgee statusline disabled.",
        style("✓").green(),
    );
    for c in &changes {
        println!("    • {c}");
    }
    println!(
        "  {} Run `edgee statusline claude enable` to turn it back on.",
        style("→").dim(),
    );
    Ok(())
}

/// Remove the top-level `statusLine` block if and only if its command
/// classifies as our own (plain or wrapped Edgee). Returns true on change.
fn remove_edgee_status_line(value: &mut Value) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    let Some(sl) = obj.get("statusLine") else {
        return false;
    };
    let cmd = match claude_settings::status_line_command(sl) {
        Some(c) => c,
        None => return false,
    };
    let kind = claude_settings::classify_command(cmd);
    if !matches!(kind, CommandKind::Edgee | CommandKind::EdgeeWrap) {
        return false;
    }
    obj.remove("statusLine");
    true
}

/// Remove any `SessionStart` hook entry whose command mentions
/// `edgee doctor` (covers both the new and legacy paths). Returns true on
/// change. Empties out wrapper objects whose `hooks` arrays end up empty.
fn remove_edgee_session_start_hook(value: &mut Value) -> Result<bool> {
    let Some(obj) = value.as_object_mut() else {
        return Ok(false);
    };
    let Some(hooks) = obj.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(false);
    };
    let Some(arr) = hooks
        .get_mut("SessionStart")
        .and_then(Value::as_array_mut)
    else {
        return Ok(false);
    };

    let original_len = arr.len();
    arr.retain_mut(|entry| {
        if let Some(inner) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
            inner.retain(|h| !is_edgee_hook(h));
            if inner.is_empty() {
                return false;
            }
        }
        !is_edgee_hook(entry)
    });
    Ok(arr.len() != original_len)
}

fn is_edgee_hook(v: &Value) -> bool {
    v.get("command")
        .and_then(Value::as_str)
        .is_some_and(|cmd| cmd.contains(HOOK_NEW_MARKER) || cmd.contains(HOOK_LEGACY_MARKER))
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
            // global_config_dir uses HOME on Unix; clearing XDG_CONFIG_HOME
            // would only matter if the helper consulted it, but be safe.
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
        Restore {
            home: prev_home,
            xdg: prev_xdg,
        }
    }

    fn fresh_home() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        (tmp, home)
    }

    fn read_user(home: &std::path::Path) -> Value {
        let p = home.join(".claude").join("settings.json");
        let s = fs::read_to_string(p).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[tokio::test]
    async fn disable_then_enable_round_trips() {
        let (_tmp, home) = fresh_home();
        let _lock = env_lock();
        let _h = isolate_home(&home);

        // Start from a clean install.
        install::run(install::Options::default()).await.unwrap();
        let installed = read_user(&home);
        assert_eq!(installed["statusLine"]["command"], "edgee statusline render");

        // Disable.
        disable().await.unwrap();
        assert!(disabled_marker_path().is_file());
        let after_disable = read_user(&home);
        assert!(after_disable.get("statusLine").is_none());

        // Enable round-trips.
        enable().await.unwrap();
        assert!(!disabled_marker_path().is_file());
        let after_enable = read_user(&home);
        assert_eq!(after_enable["statusLine"]["command"], "edgee statusline render");
    }

    #[tokio::test]
    async fn disable_leaves_third_party_statusline_alone() {
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude").join("settings.json"),
            serde_json::to_string_pretty(&json!({
                "statusLine": {"type": "command", "command": "/path/to/user-custom.sh"}
            }))
            .unwrap(),
        )
        .unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);

        disable().await.unwrap();

        let v = read_user(&home);
        assert_eq!(v["statusLine"]["command"], "/path/to/user-custom.sh");
        assert!(disabled_marker_path().is_file());
    }

    #[tokio::test]
    async fn enable_clears_disable_marker_when_only_marker_exists() {
        let (_tmp, home) = fresh_home();
        let _lock = env_lock();
        let _h = isolate_home(&home);

        // Pre-create the marker, no settings file yet.
        let marker = disabled_marker_path();
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, b"").unwrap();
        assert!(marker.is_file());

        enable().await.unwrap();

        assert!(!marker.is_file(), "marker should be cleared");
        let v = read_user(&home);
        assert_eq!(v["statusLine"]["command"], "edgee statusline render");
    }
}
