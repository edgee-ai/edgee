//! `edgee install` — install Edgee's user-level Claude Code integration.
//!
//! Two side effects on `~/.claude/settings.json`:
//!
//! 1. Sets `statusLine.command` to `edgee statusline` if no statusLine is
//!    configured at user level. (Doesn't touch the user's existing one — we
//!    overlay via `edgee fix` per project, never globally.)
//! 2. Adds a `SessionStart` hook that runs `edgee doctor --warn-only`, so
//!    users in projects with their own statusLine get a one-line warning
//!    when Edgee is shadowed. Idempotent.
//!
//! The shared `.claude/settings.json` files in projects are **never**
//! modified by this command — only the user-level file.

use anyhow::{Context, Result};
use console::style;
use serde_json::Value;

use crate::commands::claude_settings;

#[derive(Debug, Default, clap::Parser)]
pub struct Options {
    /// Skip the SessionStart hook; only install the statusLine.
    #[arg(long)]
    pub skip_hook: bool,

    /// Skip the user-level statusLine; only install the SessionStart hook.
    #[arg(long)]
    pub skip_statusline: bool,
}

const HOOK_COMMAND: &str = "edgee doctor --warn-only";
const HOOK_MARKER: &str = "edgee doctor";

pub async fn run(opts: Options) -> Result<()> {
    let path = claude_settings::user_settings_path();
    let mut value = if path.is_file() {
        claude_settings::read_settings(&path)?.value
    } else {
        Value::Object(Default::default())
    };

    let mut changes = Vec::new();

    if !opts.skip_statusline && install_statusline(&mut value)? {
        changes.push("statusLine → edgee statusline");
    }

    if !opts.skip_hook && install_session_start_hook(&mut value)? {
        changes.push("SessionStart hook → edgee doctor --warn-only");
    }

    if changes.is_empty() {
        println!(
            "  {} Edgee is already installed at user level — nothing to do.",
            style("✓").green(),
        );
        return Ok(());
    }

    claude_settings::write_settings(&path, &value)?;
    println!("  {} Wrote {}", style("✓").green(), path.display());
    for c in &changes {
        println!("    • {c}");
    }
    println!();
    println!(
        "  {} Run `edgee doctor` in any project to check for statusLine conflicts.",
        style("→").dim(),
    );
    Ok(())
}

fn install_statusline(value: &mut Value) -> Result<bool> {
    if value.get("statusLine").is_some() {
        // User already has a statusLine — never override.
        return Ok(false);
    }
    claude_settings::set_status_line(value, "edgee statusline", Some(10));
    Ok(true)
}

/// Idempotently add a `SessionStart` hook entry that runs `edgee doctor
/// --warn-only`. Returns `true` if the file changed.
fn install_session_start_hook(value: &mut Value) -> Result<bool> {
    let obj = value
        .as_object_mut()
        .context("user settings root must be a JSON object")?;

    let hooks = obj
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    let hooks_obj = hooks
        .as_object_mut()
        .context("`hooks` in user settings is not a JSON object")?;

    let session_start = hooks_obj
        .entry("SessionStart".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let arr = session_start
        .as_array_mut()
        .context("`hooks.SessionStart` is not an array")?;

    if hook_already_present(arr) {
        return Ok(false);
    }

    arr.push(serde_json::json!({
        "hooks": [
            { "type": "command", "command": HOOK_COMMAND }
        ]
    }));
    Ok(true)
}

fn hook_already_present(arr: &[Value]) -> bool {
    for entry in arr {
        // Support both nested ("hooks": [...]) and flat ({"type", "command"}) forms.
        if let Some(inner) = entry.get("hooks").and_then(Value::as_array) {
            for h in inner {
                if matches_hook(h) {
                    return true;
                }
            }
        }
        if matches_hook(entry) {
            return true;
        }
    }
    false
}

fn matches_hook(v: &Value) -> bool {
    let cmd = v.get("command").and_then(Value::as_str).unwrap_or("");
    cmd.contains(HOOK_MARKER)
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
        let prev = std::env::var_os("HOME");
        unsafe {
            std::env::set_var("HOME", home);
        }
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                match &self.0 {
                    Some(prev) => unsafe { std::env::set_var("HOME", prev) },
                    None => unsafe { std::env::remove_var("HOME") },
                }
            }
        }
        Restore(prev)
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
    async fn install_creates_settings_when_absent() {
        let (_tmp, home) = fresh_home();
        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options {
            skip_hook: false,
            skip_statusline: false,
        })
        .await
        .unwrap();
        let v = read_user(&home);
        assert_eq!(v["statusLine"]["command"], "edgee statusline");
        let arr = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(
            arr[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains(HOOK_MARKER)
        );
    }

    #[tokio::test]
    async fn install_does_not_replace_existing_statusline() {
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
        run(Options {
            skip_hook: true,
            skip_statusline: false,
        })
        .await
        .unwrap();

        let v = read_user(&home);
        assert_eq!(v["statusLine"]["command"], "/path/to/user-custom.sh");
    }

    #[tokio::test]
    async fn install_is_idempotent() {
        let (_tmp, home) = fresh_home();
        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options::default()).await.unwrap();
        let first = read_user(&home);

        run(Options::default()).await.unwrap();
        let second = read_user(&home);

        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn install_preserves_existing_unrelated_hooks() {
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude").join("settings.json"),
            serde_json::to_string_pretty(&json!({
                "hooks": {
                    "SessionStart": [
                        {"hooks": [{"type": "command", "command": "/some/other/script.sh"}]}
                    ],
                    "OnUserPrompt": [
                        {"hooks": [{"type": "command", "command": "/another.sh"}]}
                    ]
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options::default()).await.unwrap();

        let v = read_user(&home);
        let session_start = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(session_start.len(), 2, "must concat, not replace");
        assert!(
            v["hooks"]["OnUserPrompt"].as_array().is_some(),
            "unrelated hook events must survive"
        );
    }

    #[tokio::test]
    async fn install_skip_flags_respected() {
        let (_tmp, home) = fresh_home();
        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options {
            skip_hook: false,
            skip_statusline: true,
        })
        .await
        .unwrap();

        let v = read_user(&home);
        assert!(v.get("statusLine").is_none());
        assert!(v["hooks"]["SessionStart"].is_array());
    }

}
