//! Helpers for reading and writing a statusLine-bearing JSON settings file
//! (Copilot CLI's `settings.json`), and for recognising Edgee's own commands.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

/// A parsed settings file.
#[derive(Debug, Clone)]
pub struct SettingsFile {
    #[allow(dead_code)]
    pub path: PathBuf,
    pub value: Value,
}

/// Read a settings file and parse it as JSON. A missing file is reported as
/// an error; callers must check existence first.
pub fn read_settings(path: &Path) -> Result<SettingsFile> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let value: Value = if content.trim().is_empty() {
        Value::Object(Default::default())
    } else {
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))?
    };
    Ok(SettingsFile {
        path: path.to_path_buf(),
        value,
    })
}

/// Write a settings file atomically, creating parent directories as needed.
pub fn write_settings(path: &Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(value)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, content).with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("Failed to rename into {}", path.display()))?;
    Ok(())
}

/// Extract the `command` string from a `statusLine` JSON value. Returns
/// `None` if the value is malformed or the command is empty.
pub fn status_line_command(sl: &Value) -> Option<&str> {
    let cmd = sl.get("command")?.as_str()?;
    if cmd.is_empty() {
        None
    } else {
        Some(cmd)
    }
}

/// Escape a command for inclusion inside POSIX single quotes. The result is
/// safe to splice into `... '<escaped>' ...` regardless of what characters
/// the input contains (single quotes, backslashes, `$`, backticks, …).
///
/// The single-quote escape is the canonical "always safe" POSIX shell escape
/// because POSIX single-quoted strings have no escape sequences at all — to
/// embed a single quote you have to close the string, escape the quote, and
/// reopen.
pub fn posix_single_quote_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Set or merge a `statusLine` block into a settings JSON value, preserving
/// all other top-level keys.
pub fn set_status_line(value: &mut Value, command: &str, refresh_interval: Option<u64>) {
    let obj = value.as_object_mut();
    let mut sl = serde_json::Map::new();
    sl.insert("type".into(), Value::String("command".into()));
    sl.insert("command".into(), Value::String(command.into()));
    if let Some(ms) = refresh_interval {
        sl.insert("refreshInterval".into(), Value::from(ms));
    }
    if let Some(obj) = obj {
        obj.insert("statusLine".into(), Value::Object(sl));
    } else {
        let mut new_obj = serde_json::Map::new();
        new_obj.insert("statusLine".into(), Value::Object(sl));
        *value = Value::Object(new_obj);
    }
}

/// Classification of the effective `statusLine` command relative to Edgee's
/// own integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// No `statusLine` configured anywhere — Claude Code shows nothing.
    Absent,
    /// Plain Edgee command (`edgee statusline ...` without `--wrap`) or a
    /// known legacy Edgee wrapper path.
    Edgee,
    /// Edgee overlay (`edgee statusline --wrap ...`).
    EdgeeWrap,
    /// Some third-party command — Edgee is shadowed if it would otherwise be
    /// active.
    ThirdParty,
}

/// Classify a raw `statusLine.command` string. The matching is structural —
/// no hardcoded third-party tool names — so the wrapper works generically.
pub fn classify_command(command: &str) -> CommandKind {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return CommandKind::Absent;
    }
    if is_edgee_wrap(trimmed) {
        return CommandKind::EdgeeWrap;
    }
    if is_edgee_plain(trimmed) {
        return CommandKind::Edgee;
    }
    CommandKind::ThirdParty
}

fn is_edgee_wrap(cmd: &str) -> bool {
    // Accept naming variants we may pick:
    //   - `edgee statusline wrap <cmd>`        (current, written by `fix`)
    //   - `edgee statusline --wrap <cmd>`      (legacy, still in deployed
    //                                          `.claude/settings.local.json`
    //                                          files written before the
    //                                          subcommand restructure)
    //   - `edgee statusline-wrap <cmd>`        (no-dash hyphenation hedge)
    let head = cmd.split_whitespace().take(3).collect::<Vec<_>>();
    matches!(
        head.as_slice(),
        ["edgee", "statusline", "wrap"]
            | ["edgee", "statusline", "--wrap"]
            | ["edgee", "statusline-wrap", ..]
    )
}

fn is_edgee_plain(cmd: &str) -> bool {
    // `edgee statusline` (no --wrap) or any legacy wrapper script we install.
    let head = cmd.split_whitespace().take(2).collect::<Vec<_>>();
    if matches!(head.as_slice(), ["edgee", "statusline"]) {
        return true;
    }
    // Legacy paths written by `edgee launch` in the user's edgee config dir.
    cmd.contains("statusline-wrapper.sh") || cmd.contains("edgee/statusline.sh")
}

/// Test-only mutex shared by every module that exercises the `HOME` /
/// `EDGEE_*` env vars. Required because `cargo test` runs tests in parallel
/// and process-global env mutation is racy.
#[cfg(test)]
pub fn env_test_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classify_basic_cases() {
        assert_eq!(classify_command(""), CommandKind::Absent);
        assert_eq!(classify_command("   "), CommandKind::Absent);
        assert_eq!(classify_command("edgee statusline"), CommandKind::Edgee);
        assert_eq!(
            classify_command("  edgee statusline   "),
            CommandKind::Edgee
        );
        assert_eq!(
            classify_command("edgee statusline --wrap 'foo'"),
            CommandKind::EdgeeWrap
        );
        assert_eq!(
            classify_command("edgee statusline wrap 'foo'"),
            CommandKind::EdgeeWrap
        );
        assert_eq!(
            classify_command("edgee statusline-wrap 'foo'"),
            CommandKind::EdgeeWrap
        );
        assert_eq!(classify_command("/bin/foo"), CommandKind::ThirdParty);
        assert_eq!(
            classify_command("ccusage statusline"),
            CommandKind::ThirdParty
        );
    }

    #[test]
    fn classify_legacy_edgee_paths() {
        assert_eq!(
            classify_command("/Users/me/.config/edgee/statusline-wrapper.sh"),
            CommandKind::Edgee
        );
        assert_eq!(
            classify_command("/Users/me/.config/edgee/statusline.sh"),
            CommandKind::Edgee
        );
    }

    #[test]
    fn posix_escape_preserves_ascii() {
        assert_eq!(posix_single_quote_escape("hello"), "hello");
        assert_eq!(
            posix_single_quote_escape("/bin/foo --bar"),
            "/bin/foo --bar"
        );
    }

    #[test]
    fn posix_escape_handles_single_quote() {
        // `it's` becomes `it'\''s` — close, escape, reopen.
        assert_eq!(posix_single_quote_escape("it's"), "it'\\''s");
        // Wrapped: `'it'\''s'` — well-formed.
        let wrapped = format!("'{}'", posix_single_quote_escape("it's"));
        assert_eq!(wrapped, "'it'\\''s'");
    }

    #[test]
    fn posix_escape_handles_special_chars() {
        // Single quotes are the only thing that needs escaping inside
        // POSIX single-quoted strings — backslashes, $, `, " all pass through.
        assert_eq!(posix_single_quote_escape("a\\b"), "a\\b");
        assert_eq!(posix_single_quote_escape("$VAR"), "$VAR");
        assert_eq!(posix_single_quote_escape("`cmd`"), "`cmd`");
        assert_eq!(posix_single_quote_escape("\"foo\""), "\"foo\"");
    }

    #[test]
    fn set_status_line_preserves_other_keys() {
        let mut v = json!({"hooks": {"foo": "bar"}, "theme": "dark"});
        set_status_line(&mut v, "edgee statusline --wrap 'x'", Some(10));
        assert_eq!(v["hooks"]["foo"], "bar");
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["statusLine"]["type"], "command");
        assert_eq!(v["statusLine"]["command"], "edgee statusline --wrap 'x'");
        assert_eq!(v["statusLine"]["refreshInterval"], 10);
    }

    #[test]
    fn set_status_line_creates_object_from_null() {
        let mut v = Value::Null;
        set_status_line(&mut v, "edgee statusline", None);
        assert_eq!(v["statusLine"]["command"], "edgee statusline");
        assert!(v["statusLine"].get("refreshInterval").is_none());
    }
}
