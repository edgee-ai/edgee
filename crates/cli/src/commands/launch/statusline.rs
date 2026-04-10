use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// The Edgee status-line bash script that calls the public session API
/// and displays compression percentage with a visual bar.
const EDGEE_SCRIPT: &str = r#"#!/usr/bin/env bash
set -euo pipefail

# Read stdin (Claude Code passes session JSON)
INPUT=$(cat)

# Env vars set by "edgee launch claude"
SESSION_ID="${EDGEE_SESSION_ID:-}"
API_URL="${EDGEE_CONSOLE_API_URL:-https://api.edgee.app}"

if [ -z "$SESSION_ID" ]; then
    exit 0
fi

# ── cache ────────────────────────────────────────────────────────────
CACHE_DIR="${HOME}/.config/edgee/cache"
mkdir -p "$CACHE_DIR"
CACHE_FILE="${CACHE_DIR}/statusline-${SESSION_ID}.json"
CACHE_MAX_AGE=8

USE_CACHE=false
if [ -f "$CACHE_FILE" ]; then
    if [ "$(uname)" = "Darwin" ]; then
        FILE_AGE=$(( $(date +%s) - $(stat -f %m "$CACHE_FILE") ))
    else
        FILE_AGE=$(( $(date +%s) - $(stat -c %Y "$CACHE_FILE") ))
    fi
    [ "$FILE_AGE" -lt "$CACHE_MAX_AGE" ] && USE_CACHE=true
fi

if [ "$USE_CACHE" = true ]; then
    STATS=$(cat "$CACHE_FILE")
else
    STATS=$(curl -sf --max-time 5 \
        "${API_URL}/v1/sessions/${SESSION_ID}/summary" 2>/dev/null) || STATS=""
    if [ -n "$STATS" ]; then
        echo "$STATS" > "$CACHE_FILE"
    elif [ -f "$CACHE_FILE" ]; then
        STATS=$(cat "$CACHE_FILE")
    fi
fi

# ── render ───────────────────────────────────────────────────────────
SEP=""
[ -n "${EDGEE_HAS_EXISTING_STATUSLINE:-}" ] && SEP="| "

if [ -z "$STATS" ] || ! command -v jq &>/dev/null; then
    echo -e "${SEP}\033[38;5;128m三 Edgee\033[0m"
    exit 0
fi

BEFORE=$(echo "$STATS" | jq -r '.total_uncompressed_tools_tokens // 0')
AFTER=$(echo "$STATS"  | jq -r '.total_compressed_tools_tokens // 0')
REQUESTS=$(echo "$STATS" | jq -r '.total_requests // 0')

PURPLE='\033[38;5;128m'
BOLD_PURPLE='\033[1;38;5;128m'
DIM='\033[2m'
RESET='\033[0m'

if [ "$BEFORE" -gt 0 ] && [ "$AFTER" -lt "$BEFORE" ]; then
    PCT=$(( (BEFORE - AFTER) * 100 / BEFORE ))
    FILLED=$(( PCT * 10 / 100 ))
    BAR=""
    for ((i=0; i<FILLED; i++)); do BAR+="█"; done
    for ((i=FILLED; i<10; i++)); do BAR+="░"; done

    echo -e "${SEP}${PURPLE}三 Edgee${RESET}  ${PURPLE}${BAR}${RESET} ${BOLD_PURPLE}${PCT}%${RESET} tool compression  ${DIM}${REQUESTS} reqs${RESET}"
else
    if [ "$REQUESTS" -gt 0 ]; then
        echo -e "${SEP}${PURPLE}三 Edgee${RESET}  ${DIM}${REQUESTS} reqs${RESET}"
    else
        echo -e "${SEP}${PURPLE}三 Edgee${RESET}"
    fi
fi
"#;

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
