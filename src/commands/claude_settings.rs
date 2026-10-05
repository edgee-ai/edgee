//! Upgrade cleanup for the retired Edgee statusline integration.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

/// Run on every Claude launch so projects visited after upgrading are cleaned too.
/// Never edit shared project settings or execute a wrapped command during migration.
pub fn remove_legacy_statusline() {
    let user_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".claude"))
        });
    if let Some(dir) = user_dir {
        clean_file(&dir.join("settings.json"), false);
    }
    if let Ok(cwd) = std::env::current_dir() {
        for dir in cwd.ancestors() {
            clean_file(&dir.join(".claude/settings.local.json"), false);
            clean_file(&dir.join(".claude/settings.json"), true);
        }
    }
}

fn clean_file(path: &Path, shared: bool) {
    if !path.is_file() {
        return;
    }
    if let Err(error) = migrate_file(path, shared) {
        eprintln!("Edgee statusline cleanup: {error:#}. Remove legacy Edgee statusLine and doctor hooks from {}.", path.display());
    }
}

fn migrate_file(path: &Path, shared: bool) -> Result<()> {
    let original = fs::read_to_string(path)?;
    if original.trim().is_empty() {
        return Ok(());
    }
    let mut value: Value = serde_json::from_str(&original)
        .with_context(|| format!("Cannot read {}", path.display()))?;
    if !clean_settings(&mut value) {
        return Ok(());
    }
    if shared {
        eprintln!("Legacy Edgee statusline in {}. Remove Edgee statusLine and doctor hooks; restore any wrapped custom command.", path.display());
        return Ok(());
    }
    write_settings(path, &value)
}

fn write_settings(path: &Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension(format!("json.edgee-{}.tmp", std::process::id()));
    fs::write(&tmp, serde_json::to_string_pretty(value)?)?;
    fs::rename(&tmp, path).with_context(|| format!("Cannot update {}", path.display()))
}

fn clean_settings(value: &mut Value) -> bool {
    let original = value.clone();
    if let Some(command) = value.pointer("/statusLine/command").and_then(Value::as_str) {
        if let Some(inner) = unwrap_command(command) {
            value["statusLine"]["command"] = Value::String(inner);
        } else if is_plain_command(command) {
            if let Some(obj) = value.as_object_mut() {
                obj.remove("statusLine");
            }
        }
    }
    if let Some(entries) = value
        .pointer_mut("/hooks/SessionStart")
        .and_then(Value::as_array_mut)
    {
        entries.retain_mut(|entry| {
            if is_doctor_hook(entry) {
                return false;
            }
            if let Some(hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = hooks.len();
                hooks.retain(|hook| !is_doctor_hook(hook));
                return before == hooks.len() || !hooks.is_empty();
            }
            true
        });
    }
    *value != original
}

fn is_plain_command(command: &str) -> bool {
    let is_edgee_statusline = split_edgee_invocation(command).is_some_and(|(_, args)| {
        matches!(
            args.split_whitespace().collect::<Vec<_>>().as_slice(),
            ["statusline"] | ["statusline", "render"]
        )
    });
    is_edgee_statusline
        || (command.split_whitespace().count() == 1
            && (command.trim().ends_with("/edgee/statusline.sh")
                || command.trim().ends_with("/edgee/statusline-wrapper.sh")))
}

/// Decode only the single-quoted form Edgee wrote, including escaped apostrophes.
/// Refuse shell suffixes and malformed quoting instead of losing custom commands.
fn unwrap_command(command: &str) -> Option<String> {
    let (env, args) = split_edgee_invocation(command)?;
    let quoted = ["statusline wrap ", "statusline --wrap ", "statusline-wrap "]
        .iter()
        .find_map(|prefix| args.trim().strip_prefix(prefix))?
        .trim();
    let body = quoted.strip_prefix('\'')?.strip_suffix('\'')?;
    let mut result = env.iter().map(|var| format!("{var} ")).collect::<String>();
    let mut remaining = body;
    while let Some(index) = remaining.find('\'') {
        result.push_str(&remaining[..index]);
        remaining = remaining[index..].strip_prefix("'\\''")?;
        result.push('\'');
    }
    result.push_str(remaining);
    Some(result)
}

/// Split `[VAR=value ...] [/path/to/]edgee args` as users customised it (e.g.
/// `EDGEE_STATUSLINE_LAYOUT=stacked ~/bin/edgee statusline ...`). Returns the
/// non-Edgee env assignments to keep, and the arguments after the binary.
fn split_edgee_invocation(command: &str) -> Option<(Vec<&str>, &str)> {
    let mut env = Vec::new();
    let mut rest = command.trim_start();
    loop {
        let (token, tail) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        if is_env_assignment(token) {
            if !token.starts_with("EDGEE_") {
                env.push(token);
            }
            rest = tail.trim_start();
            continue;
        }
        let program = token.rsplit(['/', '\\']).next()?;
        return (program == "edgee" || program.eq_ignore_ascii_case("edgee.exe"))
            .then_some((env, tail));
    }
}

fn is_env_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, value)| {
        name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !value.contains(['\'', '"', '`', '$', '\\'])
    })
}

fn is_doctor_hook(value: &Value) -> bool {
    let Some(command) = value.get("command").and_then(Value::as_str) else {
        return false;
    };
    let Some((_, args)) = split_edgee_invocation(command) else {
        return false;
    };
    matches!(
        args.split_whitespace().collect::<Vec<_>>().as_slice(),
        ["statusline", "claude", "doctor"]
            | ["statusline", "claude", "doctor", "--warn-only"]
            | ["doctor"]
            | ["doctor", "--warn-only"]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    #[test]
    fn cleanup_preserves_custom_hooks_in_same_group_and_is_idempotent() {
        let mut value = json!({
            "statusLine": {"type": "command", "command": "edgee statusline render"},
            "hooks": {"SessionStart": [{"matcher": "", "hooks": [
                {"type": "command", "command": "edgee statusline claude doctor --warn-only"},
                {"type": "command", "command": "echo custom"}
            ]}]},
            "env": {"CUSTOM": "1"}
        });
        assert!(clean_settings(&mut value));
        assert!(value.get("statusLine").is_none());
        assert_eq!(
            value["hooks"]["SessionStart"][0]["hooks"],
            json!([{"type": "command", "command": "echo custom"}])
        );
        assert_eq!(value["env"]["CUSTOM"], "1");
        assert!(!clean_settings(&mut value));
    }

    #[test]
    fn restores_wrapped_commands_literally() {
        let original = "printf '%s' \"$HOME\" `whoami` \\ hi";
        for prefix in [
            "edgee statusline wrap",
            "edgee statusline --wrap",
            "edgee statusline-wrap",
        ] {
            let command = format!("{prefix} '{}'", original.replace('\'', "'\\''"));
            let mut value =
                json!({"statusLine": {"type": "command", "command": command, "padding": 2}});
            assert!(clean_settings(&mut value));
            assert_eq!(value["statusLine"]["command"], original);
            assert_eq!(value["statusLine"]["padding"], 2);
        }
    }

    #[test]
    fn unwraps_commands_with_env_prefix_and_binary_path() {
        for (command, expected) in [
            (
                "EDGEE_STATUSLINE_LAYOUT=stacked /home/user/local/bin/edgee statusline wrap 'mise exec -- ccstatusline'",
                "mise exec -- ccstatusline",
            ),
            (
                "FOO=1 EDGEE_X=y ~/.local/bin/edgee statusline wrap 'ccusage statusline'",
                "FOO=1 ccusage statusline",
            ),
        ] {
            let mut value =
                json!({"statusLine": {"type": "command", "command": command, "refreshInterval": 10}});
            assert!(clean_settings(&mut value));
            assert_eq!(value["statusLine"]["command"], expected);
            assert_eq!(value["statusLine"]["refreshInterval"], 10);
        }
        let mut value = json!({
            "statusLine": {"command": "EDGEE_STATUSLINE_LAYOUT=stacked /opt/edgee statusline"},
            "hooks": {"SessionStart": [{"command": "/opt/edgee doctor --warn-only"}]}
        });
        assert!(clean_settings(&mut value));
        assert!(value.get("statusLine").is_none());
        assert_eq!(value["hooks"]["SessionStart"], json!([]));
    }

    #[test]
    fn leaves_custom_commands_and_malformed_wrappers_untouched() {
        for command in [
            "echo 'edgee statusline'",
            "edgee statusline wrap 'foo' && echo hi",
            "edgee statusline wrap 'broken",
            "ccusage statusline",
            "/usr/bin/not-edgee statusline wrap 'foo'",
            "FOO=\"a b\" edgee statusline wrap 'foo'",
        ] {
            let mut value = json!({"statusLine": {"command": command}, "hooks": {"SessionStart": [{"command": "echo 'edgee doctor'"}]}});
            assert!(!clean_settings(&mut value));
        }
    }

    #[test]
    fn cleans_local_files_but_never_shared_or_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let original = r#"{"statusLine":{"command":"edgee statusline"},"keep":true}"#;
        fs::write(&path, original).unwrap();
        migrate_file(&path, true).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        migrate_file(&path, false).unwrap();
        let value: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value, json!({"keep": true}));
        fs::write(&path, "{invalid").unwrap();
        assert!(migrate_file(&path, false).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{invalid");
    }
}
