//! `edgee statusline claude install` — install Edgee's user-level Claude
//! Code integration.
//!
//! Two side effects on `~/.claude/settings.json`:
//!
//! 1. Sets `statusLine.command` to `edgee statusline render` if no statusLine
//!    is configured at user level. (Doesn't touch the user's existing one —
//!    we overlay via `edgee statusline claude fix` per project, never
//!    globally.)
//! 2. Adds a `SessionStart` hook that runs
//!    `edgee statusline claude doctor --warn-only`, so users in projects with
//!    their own statusLine get a one-line warning when Edgee is shadowed.
//!    Idempotent. Legacy hook entries pointing at the previous top-level
//!    `edgee doctor --warn-only` command are rewritten to the new path.
//!
//! Legacy `statusLine.command` values (bare `edgee statusline` or wrapper
//! script paths from the pre-restructure transient install) are migrated to
//! the canonical `edgee statusline render` form on every install.
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

const HOOK_COMMAND: &str = "edgee statusline claude doctor --warn-only";
const HOOK_NEW_MARKER: &str = "edgee statusline claude doctor";
const HOOK_LEGACY_MARKER: &str = "edgee doctor";

pub async fn run(opts: Options) -> Result<()> {
    let path = claude_settings::user_settings_path();
    let mut value = if path.is_file() {
        claude_settings::read_settings(&path)?.value
    } else {
        Value::Object(Default::default())
    };

    let mut changes = Vec::new();

    if !opts.skip_statusline && install_statusline(&mut value)? {
        changes.push("statusLine → edgee statusline render");
    }

    if !opts.skip_hook && install_session_start_hook(&mut value)? {
        changes.push("SessionStart hook → edgee statusline claude doctor --warn-only");
    }

    // Best-effort: clean up wrapper scripts left behind by the old transient
    // install (pre-restructure). Ignore errors — these are stale files in
    // the user's edgee config dir and harmless either way.
    cleanup_legacy_wrapper_scripts();

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
        "  {} Run `edgee statusline claude doctor` in any project to check for statusLine conflicts.",
        style("→").dim(),
    );
    Ok(())
}

fn install_statusline(value: &mut Value) -> Result<bool> {
    if let Some(sl) = value.get("statusLine") {
        if let Some(cmd) = claude_settings::status_line_command(sl) {
            // Heal a legacy form (bare `edgee statusline`, or a wrapper-script
            // path left over from the old transient install): rewrite it to
            // the canonical explicit form so Claude Code calls the renderer
            // directly instead of hitting our help output.
            if is_legacy_statusline(cmd) {
                claude_settings::set_status_line(value, STATUSLINE_COMMAND, Some(10));
                return Ok(true);
            }
        }
        // User has their own statusLine — never override.
        return Ok(false);
    }
    claude_settings::set_status_line(value, STATUSLINE_COMMAND, Some(10));
    Ok(true)
}

const STATUSLINE_COMMAND: &str = "edgee statusline render";

fn is_legacy_statusline(cmd: &str) -> bool {
    // Bare `edgee statusline` (no subcommand) — would now print help, so
    // rewrite to the explicit `render` form.
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    if tokens == ["edgee", "statusline"] {
        return true;
    }
    is_legacy_wrapper(cmd)
}

fn is_legacy_wrapper(cmd: &str) -> bool {
    cmd.contains("statusline-wrapper.sh") || cmd.contains("edgee/statusline.sh")
}

/// Silent heal pass for the launch flow: read `~/.claude/settings.json`,
/// rewrite a legacy `statusLine.command` to the canonical form if needed,
/// and write back. Never adds the SessionStart hook, never prints output.
/// All errors are swallowed.
pub fn heal_legacy_statusline() {
    let path = claude_settings::user_settings_path();
    if !path.is_file() {
        return;
    }
    let Ok(file) = claude_settings::read_settings(&path) else {
        return;
    };
    let mut value = file.value;

    let needs_rewrite = value
        .get("statusLine")
        .and_then(claude_settings::status_line_command)
        .is_some_and(is_legacy_statusline);

    if !needs_rewrite {
        return;
    }

    claude_settings::set_status_line(&mut value, STATUSLINE_COMMAND, Some(10));
    let _ = claude_settings::write_settings(&path, &value);
}

fn cleanup_legacy_wrapper_scripts() {
    let dir = crate::config::global_config_dir();
    let _ = std::fs::remove_file(dir.join("statusline-wrapper.sh"));
    let _ = std::fs::remove_file(dir.join("statusline.sh"));
}

/// Idempotently add a `SessionStart` hook entry that runs the doctor in
/// warn-only mode. Returns `true` if the file changed.
///
/// Migrates legacy entries that point at the old top-level `edgee doctor
/// --warn-only` to the new `edgee statusline claude doctor --warn-only` path.
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

    if migrate_legacy_hook(arr) {
        return Ok(true);
    }

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

/// Walk the `SessionStart` array, find any Edgee-doctor entry whose command
/// isn't already the new path, and rewrite it. Returns true if a rewrite
/// happened.
fn migrate_legacy_hook(arr: &mut [Value]) -> bool {
    let mut changed = false;
    for entry in arr.iter_mut() {
        if let Some(inner) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
            for h in inner.iter_mut() {
                if rewrite_if_legacy(h) {
                    changed = true;
                }
            }
        }
        if rewrite_if_legacy(entry) {
            changed = true;
        }
    }
    changed
}

fn rewrite_if_legacy(v: &mut Value) -> bool {
    let Some(obj) = v.as_object_mut() else {
        return false;
    };
    let Some(cmd) = obj.get("command").and_then(Value::as_str) else {
        return false;
    };
    if cmd.contains(HOOK_NEW_MARKER) {
        return false;
    }
    if !cmd.contains(HOOK_LEGACY_MARKER) {
        return false;
    }
    obj.insert(
        "command".to_string(),
        Value::String(HOOK_COMMAND.to_string()),
    );
    true
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
    cmd.contains(HOOK_NEW_MARKER) || cmd.contains(HOOK_LEGACY_MARKER)
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
        assert_eq!(v["statusLine"]["command"], "edgee statusline render");
        let arr = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(
            arr[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains(HOOK_NEW_MARKER)
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

    #[tokio::test]
    async fn install_migrates_stale_wrapper_path() {
        // A previous launch using the old transient install crashed or
        // otherwise skipped its Drop guard, leaving the wrapper-script
        // path in `~/.claude/settings.json`. The file no longer exists, so
        // the new install must overwrite with the canonical command.
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude").join("settings.json"),
            serde_json::to_string_pretty(&json!({
                "statusLine": {
                    "type": "command",
                    "command": "/Users/x/.config/edgee/statusline-wrapper.sh"
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options::default()).await.unwrap();
        let v = read_user(&home);
        assert_eq!(v["statusLine"]["command"], "edgee statusline render");
    }

    #[tokio::test]
    async fn install_migrates_bare_statusline_to_render() {
        // Pre-restructure installs wrote `edgee statusline` (which now prints
        // help, not the renderer). Next install must rewrite to the explicit
        // `edgee statusline render` form so Claude Code renders properly.
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude").join("settings.json"),
            serde_json::to_string_pretty(&json!({
                "statusLine": {"type": "command", "command": "edgee statusline"}
            }))
            .unwrap(),
        )
        .unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);
        run(Options::default()).await.unwrap();

        let v = read_user(&home);
        assert_eq!(v["statusLine"]["command"], "edgee statusline render");
    }

    #[tokio::test]
    async fn heal_legacy_statusline_silently_rewrites_bare_form() {
        // The launch flow calls heal_legacy_statusline() on every launch.
        // It must rewrite legacy forms without invoking the full installer
        // (no SessionStart hook touched, no print output).
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude").join("settings.json"),
            serde_json::to_string_pretty(&json!({
                "statusLine": {"type": "command", "command": "edgee statusline"},
                "hooks": {}
            }))
            .unwrap(),
        )
        .unwrap();

        let _lock = env_lock();
        let _h = isolate_home(&home);
        super::heal_legacy_statusline();

        let v = read_user(&home);
        assert_eq!(v["statusLine"]["command"], "edgee statusline render");
        // Hook section preserved as-is (heal must not add the SessionStart
        // hook — that's the installer's job).
        assert!(v["hooks"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn heal_legacy_statusline_leaves_third_party_alone() {
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
        super::heal_legacy_statusline();

        let v = read_user(&home);
        assert_eq!(v["statusLine"]["command"], "/path/to/user-custom.sh");
    }

    #[tokio::test]
    async fn install_migrates_legacy_hook_command() {
        // Users who ran the old top-level `edgee install` have a
        // SessionStart hook calling `edgee doctor --warn-only`. After the
        // restructure that command no longer exists, so the new install
        // must rewrite the entry in place rather than treat it as
        // already-present.
        let (_tmp, home) = fresh_home();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(
            home.join(".claude").join("settings.json"),
            serde_json::to_string_pretty(&json!({
                "hooks": {
                    "SessionStart": [
                        {"hooks": [{
                            "type": "command",
                            "command": "edgee doctor --warn-only"
                        }]}
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
        let cmd = v["hooks"]["SessionStart"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert_eq!(cmd, "edgee statusline claude doctor --warn-only");
        // No duplicate entry created.
        assert_eq!(
            v["hooks"]["SessionStart"].as_array().unwrap().len(),
            1,
            "migration must rewrite in place, not append"
        );
    }

}
