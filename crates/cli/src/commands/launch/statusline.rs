use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

const EDGEE_SCRIPT: &str = include_str!("statusline.sh");

pub struct StatuslineGuard {
    previous_status_line: Option<serde_json::Value>,
    settings_path: PathBuf,
    wrapper_script_path: PathBuf,
    cache_file: PathBuf,
}

fn claude_settings_path() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    PathBuf::from(home).join(".claude").join("settings.json")
}

fn edgee_script_path() -> PathBuf {
    crate::config::global_config_dir().join("statusline.sh")
}

fn wrapper_script_path() -> PathBuf {
    crate::config::global_config_dir().join("statusline-wrapper.sh")
}

fn cache_file_path(session_id: &str) -> PathBuf {
    crate::config::global_config_dir()
        .join("cache")
        .join(format!("statusline-{session_id}.json"))
}

/// Returns true if the current statusLine command already points to our wrapper.
fn is_our_statusline(settings: &serde_json::Value) -> bool {
    settings
        .get("statusLine")
        .and_then(|sl| sl.get("command"))
        .and_then(|c| c.as_str())
        .map(|cmd| cmd.contains("statusline-wrapper.sh") || cmd.contains("edgee/statusline.sh"))
        .unwrap_or(false)
}

/// Read `~/.claude/settings.json` as an untyped JSON value, preserving all fields.
fn read_claude_settings(path: &PathBuf) -> Result<serde_json::Value> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))
}

/// Write `~/.claude/settings.json` atomically.
fn write_claude_settings(path: &PathBuf, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(value)?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &content)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Install the Edgee status line into Claude Code's settings.
///
/// If the user already has a `statusLine` configured, a wrapper script is
/// generated that runs the original command first, then appends the Edgee line.
pub fn install(session_id: &str, _api_base_url: &str) -> Result<StatuslineGuard> {
    let edgee_path = edgee_script_path();
    let wrapper_path = wrapper_script_path();
    let settings_path = claude_settings_path();
    let cache_file = cache_file_path(session_id);

    // 1. Write the Edgee script
    if let Some(parent) = edgee_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&edgee_path, EDGEE_SCRIPT)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&edgee_path, fs::Permissions::from_mode(0o755))?;
    }

    // 2. Read existing settings
    let mut settings = read_claude_settings(&settings_path)?;

    // 3. Save previous statusLine (skip if it's already ours — crash recovery)
    let previous_status_line = if is_our_statusline(&settings) {
        None
    } else {
        settings.get("statusLine").cloned()
    };

    // 4. Extract existing command (if any) and build wrapper
    let existing_command = previous_status_line
        .as_ref()
        .and_then(|sl| sl.get("command"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());

    let wrapper_content = if let Some(ref existing_cmd) = existing_command {
        format!(
            "#!/usr/bin/env bash\nINPUT=$(cat)\necho \"$INPUT\" | {} 2>/dev/null\nexport EDGEE_HAS_EXISTING_STATUSLINE=1\necho \"$INPUT\" | {}\n",
            existing_cmd,
            edgee_path.display(),
        )
    } else {
        format!(
            "#!/usr/bin/env bash\nINPUT=$(cat)\necho \"$INPUT\" | {}\n",
            edgee_path.display(),
        )
    };

    fs::write(&wrapper_path, &wrapper_content)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wrapper_path, fs::Permissions::from_mode(0o755))?;
    }

    // 5. Update settings.json
    let status_line_value = serde_json::json!({
        "type": "command",
        "command": wrapper_path.to_string_lossy(),
        "refreshInterval": 10
    });
    settings
        .as_object_mut()
        .context("settings.json is not a JSON object")?
        .insert("statusLine".to_string(), status_line_value);

    write_claude_settings(&settings_path, &settings)?;

    Ok(StatuslineGuard {
        previous_status_line,
        settings_path,
        wrapper_script_path: wrapper_path,
        cache_file,
    })
}

/// Restore the previous `statusLine` setting and clean up.
pub fn uninstall(guard: StatuslineGuard) -> Result<()> {
    let mut settings = read_claude_settings(&guard.settings_path)?;

    if let Some(obj) = settings.as_object_mut() {
        match &guard.previous_status_line {
            Some(prev) => {
                obj.insert("statusLine".to_string(), prev.clone());
            }
            None => {
                obj.remove("statusLine");
            }
        }
    }

    write_claude_settings(&guard.settings_path, &settings)?;

    // Clean up cache file
    let _ = fs::remove_file(&guard.cache_file);

    // Remove wrapper script (leave the main edgee script for reuse)
    let _ = fs::remove_file(&guard.wrapper_script_path);

    Ok(())
}
