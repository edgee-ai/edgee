//! `edgee statusline codebuddy install` — install Edgee's user-level
//! CodeBuddy integration.
//!
//! Sets `statusLine` in `~/.codebuddy/settings.json` to
//! `{"type":"command","command":"edgee statusline render","padding":10}` if
//! no `statusLine` is configured there yet. Never overrides a `statusLine`
//! the user already has.
//!
//! CodeBuddy also has project (`.codebuddy/settings.json`) and local
//! (`.codebuddy/settings.local.json`) settings layers above the user one —
//! same precedence shape as Claude Code — but unlike Claude's
//! `install`/`doctor`/`fix` set, this integration only manages the
//! user-level file. There's no reported user-facing conflict to resolve yet
//! (no equivalent of Claude's shadowing complaints), so the extra
//! doctor/fix machinery isn't warranted until one surfaces.

use anyhow::Result;
use console::style;
use serde_json::Value;

use crate::commands::claude_settings;

#[derive(Debug, Default, clap::Parser)]
pub struct Options {
    /// Set when we run ourselves (first-run auto-install inside `edgee
    /// launch codebuddy`) rather than because the user typed `install`.
    #[arg(skip)]
    pub implicit: bool,
}

pub(super) const STATUSLINE_COMMAND: &str = "edgee statusline render";

/// Path to `~/.codebuddy/settings.json` (or its Windows equivalent).
pub(super) fn settings_path() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    std::path::PathBuf::from(home)
        .join(".codebuddy")
        .join("settings.json")
}

pub async fn run(opts: Options) -> Result<()> {
    let path = settings_path();
    let mut value = if path.is_file() {
        claude_settings::read_settings(&path)?.value
    } else {
        Value::Object(Default::default())
    };

    if !install_statusline(&mut value) {
        if !opts.implicit {
            report_no_op(&value, &path);
        }
        return Ok(());
    }

    claude_settings::write_settings(&path, &value)?;
    println!("  {} Wrote {}", style("✓").green(), path.display());
    println!("    • statusLine → {STATUSLINE_COMMAND}");
    Ok(())
}

fn report_no_op(value: &Value, path: &std::path::Path) {
    println!(
        "  {} No changes needed in {}",
        style("✓").green(),
        path.display(),
    );
    match value
        .get("statusLine")
        .map(|sl| claude_settings::status_line_command(sl))
    {
        Some(Some(cmd)) if cmd == STATUSLINE_COMMAND => {
            println!("    • statusLine → {STATUSLINE_COMMAND}")
        }
        Some(Some(cmd)) => {
            println!("    • statusLine → {cmd} (yours — Edgee never overrides it)")
        }
        _ => println!("    • statusLine → your own config (Edgee never overrides it)"),
    }
}

/// Sets `statusLine` if none is configured yet. Never overrides an existing
/// one. Returns `true` if the file changed.
fn install_statusline(value: &mut Value) -> bool {
    if value.get("statusLine").is_some() {
        return false;
    }
    set_status_line(value);
    true
}

fn set_status_line(value: &mut Value) {
    let sl = serde_json::json!({
        "type": "command",
        "command": STATUSLINE_COMMAND,
        "padding": 10,
    });
    match value.as_object_mut() {
        Some(obj) => {
            obj.insert("statusLine".to_string(), sl);
        }
        None => {
            let mut obj = serde_json::Map::new();
            obj.insert("statusLine".to_string(), sl);
            *value = Value::Object(obj);
        }
    }
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

    fn read_settings_file(home: &std::path::Path) -> Value {
        let p = home.join(".codebuddy").join("settings.json");
        let s = fs::read_to_string(p).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[tokio::test]
    async fn install_creates_settings_when_absent() {
        let (_tmp, home) = fresh_home();
        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options::default()).await.unwrap();

        let v = read_settings_file(&home);
        assert_eq!(v["statusLine"]["command"], "edgee statusline render");
        assert_eq!(v["statusLine"]["padding"], 10);
    }

    #[tokio::test]
    async fn install_does_not_replace_existing_statusline() {
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
        run(Options::default()).await.unwrap();

        let v = read_settings_file(&home);
        assert_eq!(v["statusLine"]["command"], "/path/to/user-custom.sh");
    }

    #[tokio::test]
    async fn install_preserves_unrelated_settings_keys() {
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".codebuddy")).unwrap();
        fs::write(
            home.join(".codebuddy").join("settings.json"),
            serde_json::to_string_pretty(&json!({ "theme": "dark" })).unwrap(),
        )
        .unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options::default()).await.unwrap();

        let v = read_settings_file(&home);
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["statusLine"]["command"], "edgee statusline render");
    }

    #[tokio::test]
    async fn install_is_idempotent() {
        let (_tmp, home) = fresh_home();
        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options::default()).await.unwrap();
        let first = read_settings_file(&home);

        run(Options::default()).await.unwrap();
        let second = read_settings_file(&home);

        assert_eq!(first, second);
    }
}
