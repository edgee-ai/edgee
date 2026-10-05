//! `edgee statusline copilot enable` / `disable`.
//!
//! `disable` writes `<global_config_dir>/statusline-copilot.disabled`, which
//! stops the launch-time install, and takes Edgee out of Copilot CLI's
//! `statusLine`: removed when it is ours, unwrapped back to the user's own
//! command when `install --wrap` merged the two.

use anyhow::{Context, Result};
use console::style;
use serde_json::Value;

use super::install;
use crate::commands::statusline::settings::{self, CommandKind};

pub fn disabled_marker_path() -> std::path::PathBuf {
    crate::config::global_config_dir().join("statusline-copilot.disabled")
}

pub fn installed_marker_path() -> std::path::PathBuf {
    crate::config::global_config_dir().join("statusline-copilot.installed")
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
    println!("  {} Edgee statusline enabled for Copilot CLI.", style("✓").green());
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

    let path = super::settings_path();
    if path.is_file() {
        let mut value = settings::read_settings(&path)?.value;
        if remove_edgee(&mut value) {
            settings::write_settings(&path, &value)?;
        }
    }

    println!("  {} Edgee statusline disabled for Copilot CLI.", style("✓").green());
    println!(
        "  {} Run `edgee statusline copilot enable` to turn it back on.",
        style("→").dim()
    );
    Ok(())
}

/// Returns true when `value` changed.
fn remove_edgee(value: &mut Value) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    let Some(command) = obj
        .get("statusLine")
        .and_then(settings::status_line_command)
        .map(str::to_string)
    else {
        return false;
    };
    match settings::classify_command(&command) {
        CommandKind::EdgeeWrap => match install::unwrap_command(&command) {
            Some(theirs) => obj["statusLine"]["command"] = Value::String(theirs),
            // A wrap we can't parse back: dropping it would lose the user's
            // command, so leave it.
            None => return false,
        },
        CommandKind::Edgee => {
            obj.remove("statusLine");
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn removes_plain_edgee_statusline() {
        let mut v = json!({"statusLine": {"command": install::STATUSLINE_COMMAND}, "theme": "auto"});
        assert!(remove_edgee(&mut v));
        assert_eq!(v, json!({"theme": "auto"}));
    }

    #[test]
    fn unwraps_back_to_the_users_command() {
        let mut v = json!({"statusLine": {"command": install::wrap_command("~/line.sh"), "padding": 2}});
        assert!(remove_edgee(&mut v));
        assert_eq!(v, json!({"statusLine": {"command": "~/line.sh", "padding": 2}}));
    }

    #[test]
    fn leaves_foreign_statusline_alone() {
        let original = json!({"statusLine": {"command": "~/line.sh"}});
        let mut v = original.clone();
        assert!(!remove_edgee(&mut v));
        assert_eq!(v, original);
    }
}
