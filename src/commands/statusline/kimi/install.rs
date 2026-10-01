//! `edgee statusline kimi install` — install Edgee's user-level Kimi Code
//! integration.
//!
//! Sets `status_line.command` in `$KIMI_CODE_HOME/tui.toml` (default
//! `~/.kimi-code/tui.toml`) to `edgee statusline render` if no `command` is
//! configured there yet. Never overrides one the user already has, and never
//! touches `status_line.items` or any other key in the file.

use anyhow::{Context, Result};
use console::style;
use toml::Value;

#[derive(Debug, Default, clap::Parser)]
pub struct Options {
    /// Set when we run ourselves (first-run auto-install inside `edgee
    /// launch kimi`) rather than because the user typed `install`.
    #[arg(skip)]
    pub implicit: bool,
}

pub(super) const STATUSLINE_COMMAND: &str = "edgee statusline render";

pub async fn run(opts: Options) -> Result<()> {
    let path = super::tui_toml_path().context("Could not determine your home directory")?;
    let mut value = read_toml(&path)?;

    if !install_statusline(&mut value) {
        if !opts.implicit {
            report_no_op(&value, &path);
        }
        return Ok(());
    }

    write_toml(&path, &value)?;
    println!("  {} Wrote {}", style("✓").green(), path.display());
    println!("    • status_line.command → {STATUSLINE_COMMAND}");
    Ok(())
}

fn report_no_op(value: &Value, path: &std::path::Path) {
    println!(
        "  {} No changes needed in {}",
        style("✓").green(),
        path.display(),
    );
    match status_line_command(value) {
        Some(cmd) if cmd == STATUSLINE_COMMAND => {
            println!("    • status_line.command → {STATUSLINE_COMMAND}")
        }
        Some(cmd) => {
            println!("    • status_line.command → {cmd} (yours — Edgee never overrides it)")
        }
        None => println!("    • status_line.command → not set, but the file couldn't be read as a table"),
    }
}

fn status_line_command(value: &Value) -> Option<&str> {
    value.get("status_line")?.get("command")?.as_str()
}

/// A file we cannot parse as TOML is *not* overwritten — same refuse-to-
/// clobber discipline as the MCP config writers in this batch. A missing
/// file reads as an empty table.
fn read_toml(path: &std::path::Path) -> Result<Value> {
    if !path.is_file() {
        return Ok(Value::Table(Default::default()));
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    if content.trim().is_empty() {
        return Ok(Value::Table(Default::default()));
    }
    toml::from_str(&content).with_context(|| {
        format!(
            "{} exists but is not valid TOML.\nFix or remove it, then launch kimi again.",
            path.display()
        )
    })
}

fn write_toml(path: &std::path::Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let content = toml::to_string_pretty(value)?;
    std::fs::write(path, content).with_context(|| format!("Failed to write {}", path.display()))
}

/// Sets `status_line.command` if none is configured yet. Never overrides an
/// existing one. Returns `true` if the value changed.
fn install_statusline(value: &mut Value) -> bool {
    let Some(table) = value.as_table_mut() else {
        return false;
    };
    let status_line = table
        .entry("status_line".to_string())
        .or_insert_with(|| Value::Table(Default::default()));
    let Some(status_line_table) = status_line.as_table_mut() else {
        return false;
    };
    match status_line_table.get("command").and_then(Value::as_str) {
        Some(cmd) if !cmd.is_empty() => false,
        _ => {
            status_line_table.insert(
                "command".to_string(),
                Value::String(STATUSLINE_COMMAND.to_string()),
            );
            true
        }
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
        let prev = std::env::var_os("HOME");
        let prev_kimi_home = std::env::var_os("KIMI_CODE_HOME");
        unsafe {
            std::env::set_var("HOME", home);
            std::env::remove_var("KIMI_CODE_HOME");
        }
        struct Restore(Option<std::ffi::OsString>, Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    match &self.0 {
                        Some(prev) => std::env::set_var("HOME", prev),
                        None => std::env::remove_var("HOME"),
                    }
                    match &self.1 {
                        Some(prev) => std::env::set_var("KIMI_CODE_HOME", prev),
                        None => std::env::remove_var("KIMI_CODE_HOME"),
                    }
                }
            }
        }
        Restore(prev, prev_kimi_home)
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
    async fn install_creates_tui_toml_when_absent() {
        let (_tmp, home) = fresh_home();
        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options::default()).await.unwrap();

        let v = read_tui_toml(&home);
        assert_eq!(
            v["status_line"]["command"].as_str(),
            Some("edgee statusline render")
        );
    }

    #[tokio::test]
    async fn install_does_not_replace_existing_command() {
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".kimi-code")).unwrap();
        fs::write(
            home.join(".kimi-code").join("tui.toml"),
            "[status_line]\ncommand = \"~/.kimi-code/statusline.sh\"\n",
        )
        .unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options::default()).await.unwrap();

        let v = read_tui_toml(&home);
        assert_eq!(
            v["status_line"]["command"].as_str(),
            Some("~/.kimi-code/statusline.sh")
        );
    }

    #[tokio::test]
    async fn install_preserves_status_line_items_and_other_keys() {
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".kimi-code")).unwrap();
        fs::write(
            home.join(".kimi-code").join("tui.toml"),
            "theme = \"dark\"\n\n[status_line]\nitems = [\"mode\", \"model\", \"git\"]\n",
        )
        .unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options::default()).await.unwrap();

        let v = read_tui_toml(&home);
        assert_eq!(v["theme"].as_str(), Some("dark"));
        assert_eq!(
            v["status_line"]["items"].as_array().unwrap().len(),
            3
        );
        assert_eq!(
            v["status_line"]["command"].as_str(),
            Some("edgee statusline render")
        );
    }

    #[tokio::test]
    async fn install_is_idempotent() {
        let (_tmp, home) = fresh_home();
        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options::default()).await.unwrap();
        let first = read_tui_toml(&home);

        run(Options::default()).await.unwrap();
        let second = read_tui_toml(&home);

        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn install_refuses_to_clobber_unparseable_toml() {
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".kimi-code")).unwrap();
        let original = "this is [ not valid toml";
        fs::write(home.join(".kimi-code").join("tui.toml"), original).unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);
        assert!(run(Options::default()).await.is_err());

        let content =
            fs::read_to_string(home.join(".kimi-code").join("tui.toml")).unwrap();
        assert_eq!(content, original);
    }
}
