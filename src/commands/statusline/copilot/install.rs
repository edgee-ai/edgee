//! `edgee statusline copilot install`: set Copilot CLI's `statusLine` to
//! Edgee's renderer.
//!
//! Never overrides a statusLine the user configured. `--wrap` opts into merging
//! with it instead, by rewriting its command to `edgee statusline wrap '<theirs>'`
//! (Copilot runs the command through `/bin/sh`), keeping its other keys such as
//! `padding`.

use std::fs;

use anyhow::Result;
use console::style;
use serde_json::Value;

use crate::commands::statusline::settings::{self, CommandKind};

pub const STATUSLINE_COMMAND: &str = "edgee statusline render";

/// Copilot re-runs the command on session events anyway; the interval only
/// picks up gateway-side totals that land after the last event.
const REFRESH_INTERVAL_SECS: u64 = 10;

#[derive(Debug, Default, clap::Parser)]
pub struct Options {
    /// If a statusLine of your own is configured, show Edgee's segment next to
    /// it instead of leaving it untouched.
    #[arg(long)]
    pub wrap: bool,

    /// Set when `edgee launch copilot-cli` runs the install, so a no-op stays
    /// silent right before the agent starts.
    #[arg(skip)]
    pub implicit: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Installed,
    Wrapped,
    AlreadyEdgee,
    /// The user's own statusLine, left alone. `None` when it has no command.
    Foreign(Option<String>),
}

pub async fn run(opts: Options) -> Result<()> {
    let path = super::settings_path();
    let mut value = if path.is_file() {
        settings::read_settings(&path)?.value
    } else {
        Value::Object(Default::default())
    };

    match apply(&mut value, opts.wrap) {
        Outcome::Installed | Outcome::Wrapped => {
            settings::write_settings(&path, &value)?;
            let command = value["statusLine"]["command"].as_str().unwrap_or_default();
            println!("  {} Wrote {}", style("✓").green(), path.display());
            println!("    • statusLine → {command}");
        }
        Outcome::AlreadyEdgee if !opts.implicit => {
            println!(
                "  {} Copilot CLI statusline already set up in {}",
                style("✓").green(),
                path.display()
            );
        }
        Outcome::AlreadyEdgee => {}
        Outcome::Foreign(command) => {
            let yours = command.map(|c| format!(" ({c})")).unwrap_or_default();
            println!(
                "  {} Copilot CLI already has a statusLine{yours}; Edgee left it untouched.",
                style("→").dim()
            );
            println!(
                "    Run `edgee statusline copilot install --wrap` to show Edgee's segment next to it."
            );
        }
    }
    Ok(())
}

/// Launch-time install: once, unless disabled. Best-effort, a failure warns and
/// never stops Copilot from starting.
pub async fn ensure_installed_on_launch() {
    use super::toggle;

    if toggle::is_disabled() {
        return;
    }
    let marker = toggle::installed_marker_path();
    if marker.is_file() {
        return;
    }
    let opts = Options {
        implicit: true,
        ..Default::default()
    };
    if let Err(e) = run(opts).await {
        eprintln!(
            "  {} edgee: skipped Copilot CLI statusline install: {e}",
            style("⚠").yellow()
        );
        return;
    }
    if let Some(parent) = marker.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&marker, b"");
}

fn apply(value: &mut Value, wrap: bool) -> Outcome {
    let Some(sl) = value.get_mut("statusLine") else {
        settings::set_status_line(value, STATUSLINE_COMMAND, Some(REFRESH_INTERVAL_SECS));
        return Outcome::Installed;
    };
    let Some(command) = settings::status_line_command(sl).map(str::to_string) else {
        return Outcome::Foreign(None);
    };
    match settings::classify_command(&command) {
        CommandKind::Edgee | CommandKind::EdgeeWrap => Outcome::AlreadyEdgee,
        _ if wrap => {
            sl["command"] = Value::String(wrap_command(&command));
            Outcome::Wrapped
        }
        _ => Outcome::Foreign(Some(command)),
    }
}

pub fn wrap_command(command: &str) -> String {
    format!(
        "edgee statusline wrap '{}'",
        settings::posix_single_quote_escape(command)
    )
}

/// Inverse of [`wrap_command`], for `disable` to hand the user's command back.
pub fn unwrap_command(command: &str) -> Option<String> {
    let quoted = command.trim().strip_prefix("edgee statusline wrap ")?.trim();
    let inner = quoted.strip_prefix('\'')?.strip_suffix('\'')?;
    Some(inner.replace("'\\''", "'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn installs_when_absent_and_keeps_other_keys() {
        let mut v = json!({"theme": "auto"});
        assert_eq!(apply(&mut v, false), Outcome::Installed);
        assert_eq!(v["statusLine"]["command"], STATUSLINE_COMMAND);
        assert_eq!(v["statusLine"]["type"], "command");
        assert_eq!(v["statusLine"]["refreshInterval"], REFRESH_INTERVAL_SECS);
        assert_eq!(v["theme"], "auto");
    }

    #[test]
    fn existing_edgee_statusline_is_left_as_is() {
        let original = json!({"statusLine": {"type": "command", "command": STATUSLINE_COMMAND, "padding": 10}});
        let mut v = original.clone();
        assert_eq!(apply(&mut v, true), Outcome::AlreadyEdgee);
        assert_eq!(v, original);
    }

    #[test]
    fn foreign_statusline_is_never_overridden_without_wrap() {
        let original = json!({"statusLine": {"command": "~/my-line.sh"}});
        let mut v = original.clone();
        assert_eq!(apply(&mut v, false), Outcome::Foreign(Some("~/my-line.sh".into())));
        assert_eq!(v, original);
    }

    #[test]
    fn wrap_keeps_padding_and_round_trips_quotes() {
        let theirs = "echo 'it''s' $HOME";
        let mut v = json!({"statusLine": {"type": "command", "command": theirs, "padding": 4}});
        assert_eq!(apply(&mut v, true), Outcome::Wrapped);
        let wrapped = v["statusLine"]["command"].as_str().unwrap();
        assert!(wrapped.starts_with("edgee statusline wrap '"));
        assert_eq!(v["statusLine"]["padding"], 4);
        assert_eq!(unwrap_command(wrapped).as_deref(), Some(theirs));
    }

    #[test]
    fn unwrap_rejects_other_commands() {
        assert_eq!(unwrap_command(STATUSLINE_COMMAND), None);
        assert_eq!(unwrap_command("edgee statusline wrap unquoted"), None);
    }
}
