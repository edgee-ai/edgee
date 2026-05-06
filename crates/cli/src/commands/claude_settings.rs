//! Shared utilities for reading and writing Claude Code settings files.
//!
//! Claude Code's `statusLine` and other settings live in three layers:
//!
//! 1. `~/.claude/settings.json`           — user level
//! 2. `<repo>/.claude/settings.json`      — project shared (often committed)
//! 3. `<repo>/.claude/settings.local.json` — project local (per-user, gitignored)
//!
//! Precedence: local > shared > user. Only one `statusLine` ever runs.
//!
//! `edgee fix` overlays into the **local** file only, so it never modifies the
//! shared file (which is typically managed by another tool).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

pub const SHARED_FILE: &str = "settings.json";
pub const LOCAL_FILE: &str = "settings.local.json";

/// One Claude Code settings file (either a `.claude/settings.json` or a
/// `.claude/settings.local.json`).
#[derive(Debug, Clone)]
pub struct SettingsFile {
    /// Absolute path to the file on disk. Recorded so callers can write
    /// back into the same location if needed.
    #[allow(dead_code)]
    pub path: PathBuf,
    pub value: Value,
}

/// Resolved Claude Code project context: where the project root is, plus the
/// contents of the shared and local settings files (if any).
#[derive(Debug, Clone, Default)]
pub struct ProjectSettings {
    pub project_root: Option<PathBuf>,
    pub shared: Option<SettingsFile>,
    pub local: Option<SettingsFile>,
}

/// Where a particular `statusLine` value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLineSource {
    User,
    ProjectShared,
    ProjectLocal,
}

/// Walk up from `start` looking for the closest directory that contains a
/// `.claude` directory with at least one of `settings.json` or
/// `settings.local.json`. When one is found, both files are read (a missing
/// file becomes `None`). When none is found anywhere up to the filesystem
/// root, all fields are `None`.
pub fn discover_project<P: AsRef<Path>>(start: P) -> Result<ProjectSettings> {
    let mut current = Some(start.as_ref().to_path_buf());
    while let Some(dir) = current {
        let claude_dir = dir.join(".claude");
        if claude_dir.is_dir() {
            let shared_path = claude_dir.join(SHARED_FILE);
            let local_path = claude_dir.join(LOCAL_FILE);
            let shared_exists = shared_path.is_file();
            let local_exists = local_path.is_file();
            if shared_exists || local_exists {
                return Ok(ProjectSettings {
                    project_root: Some(dir),
                    shared: if shared_exists {
                        Some(read_settings(&shared_path)?)
                    } else {
                        None
                    },
                    local: if local_exists {
                        Some(read_settings(&local_path)?)
                    } else {
                        None
                    },
                });
            }
        }
        current = dir.parent().map(Path::to_path_buf);
    }
    Ok(ProjectSettings::default())
}

/// Read the user-level `~/.claude/settings.json` file (if it exists).
pub fn read_user_settings() -> Result<Option<SettingsFile>> {
    let path = user_settings_path();
    if !path.is_file() {
        return Ok(None);
    }
    Ok(Some(read_settings(&path)?))
}

/// Path to `~/.claude/settings.json` (or its Windows equivalent).
pub fn user_settings_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".claude").join(SHARED_FILE)
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

/// Resolve the effective `statusLine.command` according to Claude Code's
/// precedence rules: local > shared > user.
///
/// Returns `(source, value)` where `value` is the full `statusLine` object
/// (not just `command`) and `source` indicates which layer it came from. None
/// when no layer has a `statusLine`.
pub fn effective_status_line<'a>(
    project: &'a ProjectSettings,
    user: Option<&'a SettingsFile>,
) -> Option<(StatusLineSource, &'a Value)> {
    if let Some(local) = &project.local {
        if let Some(sl) = local.value.get("statusLine") {
            return Some((StatusLineSource::ProjectLocal, sl));
        }
    }
    if let Some(shared) = &project.shared {
        if let Some(sl) = shared.value.get("statusLine") {
            return Some((StatusLineSource::ProjectShared, sl));
        }
    }
    if let Some(user) = user {
        if let Some(sl) = user.value.get("statusLine") {
            return Some((StatusLineSource::User, sl));
        }
    }
    None
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
    // Accept naming variants we may pick: `edgee statusline --wrap` and
    // `edgee statusline-wrap` (no-dash hyphenation as a hedge).
    let head = cmd.split_whitespace().take(3).collect::<Vec<_>>();
    matches!(
        head.as_slice(),
        ["edgee", "statusline", "--wrap"] | ["edgee", "statusline-wrap", ..]
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
    fn effective_precedence_local_wins() {
        let project = ProjectSettings {
            project_root: Some("/tmp/proj".into()),
            shared: Some(SettingsFile {
                path: "/tmp/proj/.claude/settings.json".into(),
                value: json!({"statusLine": {"command": "shared"}}),
            }),
            local: Some(SettingsFile {
                path: "/tmp/proj/.claude/settings.local.json".into(),
                value: json!({"statusLine": {"command": "local"}}),
            }),
        };
        let user = SettingsFile {
            path: "/tmp/user/settings.json".into(),
            value: json!({"statusLine": {"command": "user"}}),
        };
        let (src, sl) = effective_status_line(&project, Some(&user)).unwrap();
        assert_eq!(src, StatusLineSource::ProjectLocal);
        assert_eq!(status_line_command(sl), Some("local"));
    }

    #[test]
    fn effective_precedence_shared_then_user() {
        let project = ProjectSettings {
            project_root: Some("/tmp/proj".into()),
            shared: Some(SettingsFile {
                path: "/tmp/proj/.claude/settings.json".into(),
                value: json!({"statusLine": {"command": "shared"}}),
            }),
            local: None,
        };
        let user = SettingsFile {
            path: "/tmp/user/settings.json".into(),
            value: json!({"statusLine": {"command": "user"}}),
        };
        let (src, sl) = effective_status_line(&project, Some(&user)).unwrap();
        assert_eq!(src, StatusLineSource::ProjectShared);
        assert_eq!(status_line_command(sl), Some("shared"));
    }

    #[test]
    fn effective_precedence_user_only() {
        let project = ProjectSettings::default();
        let user = SettingsFile {
            path: "/tmp/user/settings.json".into(),
            value: json!({"statusLine": {"command": "user"}}),
        };
        let (src, sl) = effective_status_line(&project, Some(&user)).unwrap();
        assert_eq!(src, StatusLineSource::User);
        assert_eq!(status_line_command(sl), Some("user"));
    }

    #[test]
    fn effective_returns_none_when_nothing_set() {
        let project = ProjectSettings::default();
        assert!(effective_status_line(&project, None).is_none());
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

    #[test]
    fn discover_project_walks_up() {
        let tmp = tempfile::tempdir().unwrap();
        let proj = tmp.path().join("repo");
        let nested = proj.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(proj.join(".claude")).unwrap();
        std::fs::write(
            proj.join(".claude").join("settings.json"),
            r#"{"statusLine": {"command": "x"}}"#,
        )
        .unwrap();

        let resolved = discover_project(&nested).unwrap();
        assert_eq!(resolved.project_root.as_deref(), Some(proj.as_path()));
        assert!(resolved.shared.is_some());
        assert!(resolved.local.is_none());
    }

    #[test]
    fn discover_project_returns_empty_when_no_claude_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = discover_project(tmp.path()).unwrap();
        assert!(resolved.project_root.is_none());
        assert!(resolved.shared.is_none());
        assert!(resolved.local.is_none());
    }
}
