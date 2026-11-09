//! `edgee statusline copilot`: keep Edgee in the `statusLine` of Copilot CLI's
//! user `settings.json` (`$COPILOT_HOME`, default `~/.copilot`).
//!
//! `edgee launch copilot-cli` runs [`ensure_on_launch`] every time. It is
//! idempotent and only writes when something changes, so a stale entry from an
//! older build gets upgraded and a settings file wiped by the user is repaired.
//! Two things are never touched: a statusLine of the user's own (unless
//! `install --wrap` is asked to merge with it) and anything after `uninstall`,
//! which leaves an opt-out marker.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use console::style;
use serde_json::{json, Map, Value};

/// `EDGEE_BIN` is the `edgee` that launched the session, so the statusline never
/// depends on which `edgee` (if any, or which version) is first on Copilot's
/// `PATH`. Copilot and `/bin/sh` both expand `${VAR:-default}`.
pub const COMMAND: &str = "\"${EDGEE_BIN:-edgee}\" statusline";
pub const BIN_ENV: &str = "EDGEE_BIN";

const WRAP_PREFIX: &str = "\"${EDGEE_BIN:-edgee}\" statusline --wrap ";
/// Written by earlier builds of this integration, upgraded in place.
const LEGACY_PLAIN: [&str; 2] = ["edgee statusline", "edgee statusline render"];
const LEGACY_WRAP_PREFIX: &str = "edgee statusline wrap ";

/// Copilot re-runs the command on session events anyway; the interval only
/// picks up gateway-side totals that land after the last event.
const REFRESH_INTERVAL_SECS: u64 = 10;

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Set Edgee as Copilot CLI's statusline (`edgee launch copilot-cli` does it
    /// for you).
    Install {
        /// If a statusLine of your own is configured, show Edgee's segment next
        /// to it instead of leaving it untouched.
        #[arg(long)]
        wrap: bool,
    },
    /// Remove Edgee's statusline and stop `edgee launch copilot-cli` from
    /// installing it again.
    Uninstall,
}

pub fn run(opts: Options) -> Result<()> {
    match opts.command {
        Command::Install { wrap } => install(wrap),
        Command::Uninstall => uninstall(),
    }
}

fn settings_path() -> PathBuf {
    let root = match std::env::var_os("COPILOT_HOME").filter(|v| !v.is_empty()) {
        Some(home) => PathBuf::from(home),
        None => {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .unwrap_or_default();
            PathBuf::from(home).join(".copilot")
        }
    };
    root.join("settings.json")
}

fn opt_out_marker() -> PathBuf {
    crate::config::global_config_dir().join("statusline-copilot.disabled")
}

/// Best-effort: a failure warns and never stops Copilot from starting.
pub fn ensure_on_launch() {
    if opt_out_marker().is_file() {
        return;
    }
    match sync(false) {
        Ok((_, Outcome::Changed)) => println!(
            "  {} Edgee statusline set up for Copilot CLI {}",
            style("✓").green(),
            style("(`edgee statusline copilot uninstall` removes it)").dim()
        ),
        Ok(_) => {}
        Err(e) => eprintln!(
            "  {} edgee: skipped the Copilot CLI statusline setup: {e:#}",
            style("⚠").yellow()
        ),
    }
}

fn install(wrap: bool) -> Result<()> {
    match fs::remove_file(opt_out_marker()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).context("Failed to remove the statusline opt-out marker");
        }
        _ => {}
    }
    let (path, outcome) = sync(wrap)?;
    match outcome {
        Outcome::Changed => println!("  {} Wrote {}", style("✓").green(), path.display()),
        Outcome::Unchanged => println!(
            "  {} Copilot CLI statusline already set up in {}",
            style("✓").green(),
            path.display()
        ),
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

fn uninstall() -> Result<()> {
    let marker = opt_out_marker();
    if let Some(dir) = marker.parent() {
        fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    }
    fs::write(&marker, b"").with_context(|| format!("Failed to write {}", marker.display()))?;

    let path = settings_path();
    let mut settings = load(&path)?;
    if remove(&mut settings) {
        store(&path, &settings)?;
    }
    println!("  {} Edgee statusline removed from Copilot CLI.", style("✓").green());
    println!(
        "  {} Run `edgee statusline copilot install` to bring it back.",
        style("→").dim()
    );
    Ok(())
}

fn sync(wrap: bool) -> Result<(PathBuf, Outcome)> {
    let path = settings_path();
    let mut settings = load(&path)?;
    let outcome = apply(&mut settings, wrap);
    if outcome == Outcome::Changed {
        store(&path, &settings)?;
    }
    Ok((path, outcome))
}

/// A missing or empty file is an empty settings object. Anything that is not a
/// JSON object is an error rather than something to overwrite.
fn load(path: &Path) -> Result<Map<String, Value>> {
    let content = match fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => content,
        Ok(_) => return Ok(Map::new()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
    };
    serde_json::from_str(&content)
        .with_context(|| format!("{} is not a valid JSON object", path.display()))
}

/// Atomic, so Copilot never reads a half-written file.
fn store(path: &Path, settings: &Map<String, Value>) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(settings)?)
        .with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("Failed to replace {}", path.display()))
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Changed,
    Unchanged,
    /// The user's own statusLine, left alone. `None` when it has no command.
    Foreign(Option<String>),
}

enum Kind {
    /// Our renderer, possibly in an older spelling.
    Plain,
    /// Our wrapper around the user's command, `None` if it does not parse back.
    Wrapped(Option<String>),
    Foreign,
}

fn classify(command: &str) -> Kind {
    let command = command.trim();
    if command == COMMAND || LEGACY_PLAIN.contains(&command) {
        return Kind::Plain;
    }
    match [WRAP_PREFIX, LEGACY_WRAP_PREFIX]
        .into_iter()
        .find_map(|prefix| command.strip_prefix(prefix))
    {
        Some(quoted) => Kind::Wrapped(unquote(quoted)),
        None => Kind::Foreign,
    }
}

/// Single quotes keep their content literal in POSIX shells (Copilot runs the
/// command through `/bin/sh`); a literal `'` is closed, escaped and reopened.
fn wrap_command(theirs: &str) -> String {
    format!("{WRAP_PREFIX}'{}'", theirs.replace('\'', "'\\''"))
}

fn unquote(quoted: &str) -> Option<String> {
    let inner = quoted.trim().strip_prefix('\'')?.strip_suffix('\'')?;
    Some(inner.replace("'\\''", "'"))
}

fn status_line_command(settings: &Map<String, Value>) -> Option<String> {
    let command = settings.get("statusLine")?.get("command")?.as_str()?;
    (!command.is_empty()).then(|| command.to_string())
}

fn apply(settings: &mut Map<String, Value>, wrap: bool) -> Outcome {
    if !settings.contains_key("statusLine") {
        settings.insert(
            "statusLine".into(),
            json!({"type": "command", "command": COMMAND, "refreshInterval": REFRESH_INTERVAL_SECS}),
        );
        return Outcome::Changed;
    }
    let Some(command) = status_line_command(settings) else {
        return Outcome::Foreign(None);
    };
    let wanted = match classify(&command) {
        Kind::Plain => COMMAND.to_string(),
        Kind::Wrapped(Some(theirs)) => wrap_command(&theirs),
        Kind::Foreign if wrap => wrap_command(&command),
        Kind::Wrapped(None) | Kind::Foreign => return Outcome::Foreign(Some(command)),
    };
    if wanted == command {
        return Outcome::Unchanged;
    }
    settings["statusLine"]["command"] = Value::String(wanted);
    Outcome::Changed
}

/// Takes Edgee out: drops our statusLine, or hands a wrapped one back to the
/// user's own command. Returns true when `settings` changed.
fn remove(settings: &mut Map<String, Value>) -> bool {
    let Some(command) = status_line_command(settings) else {
        return false;
    };
    match classify(&command) {
        Kind::Plain => {
            settings.remove("statusLine");
        }
        Kind::Wrapped(Some(theirs)) => {
            settings["statusLine"]["command"] = Value::String(theirs);
        }
        // A wrap that does not parse back: dropping it would lose their command.
        Kind::Wrapped(None) | Kind::Foreign => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            other => panic!("not an object: {other}"),
        }
    }

    #[test]
    fn command_uses_the_env_var_name_launch_sets() {
        assert!(COMMAND.contains(BIN_ENV));
        assert!(WRAP_PREFIX.starts_with(COMMAND));
    }

    #[test]
    fn installs_when_absent_and_keeps_other_keys() {
        let mut s = settings(json!({"theme": "auto"}));
        assert_eq!(apply(&mut s, false), Outcome::Changed);
        assert_eq!(s["statusLine"]["command"], COMMAND);
        assert_eq!(s["statusLine"]["type"], "command");
        assert_eq!(s["statusLine"]["refreshInterval"], REFRESH_INTERVAL_SECS);
        assert_eq!(s["theme"], "auto");
    }

    #[test]
    fn current_entry_is_left_as_is() {
        let original = settings(json!({"statusLine": {"command": COMMAND, "padding": 10}}));
        let mut s = original.clone();
        assert_eq!(apply(&mut s, true), Outcome::Unchanged);
        assert_eq!(s, original);
    }

    #[test]
    fn legacy_entries_are_upgraded_in_place() {
        for legacy in LEGACY_PLAIN {
            let mut s = settings(json!({"statusLine": {"command": legacy, "padding": 10}}));
            assert_eq!(apply(&mut s, false), Outcome::Changed, "{legacy}");
            assert_eq!(s["statusLine"], json!({"command": COMMAND, "padding": 10}));
        }
        let mut s = settings(json!({"statusLine": {"command": "edgee statusline wrap '~/l.sh'"}}));
        assert_eq!(apply(&mut s, false), Outcome::Changed);
        assert_eq!(s["statusLine"]["command"], wrap_command("~/l.sh"));
    }

    #[test]
    fn foreign_statusline_is_never_overridden_without_wrap() {
        let original = settings(json!({"statusLine": {"command": "~/my-line.sh"}}));
        let mut s = original.clone();
        assert_eq!(
            apply(&mut s, false),
            Outcome::Foreign(Some("~/my-line.sh".into()))
        );
        assert_eq!(s, original);
    }

    #[test]
    fn a_statusline_without_command_is_foreign_even_with_wrap() {
        let original = settings(json!({"statusLine": {"type": "command"}}));
        let mut s = original.clone();
        assert_eq!(apply(&mut s, true), Outcome::Foreign(None));
        assert_eq!(s, original);
    }

    #[test]
    fn wrap_keeps_padding_and_round_trips_quotes() {
        let theirs = "echo 'it''s' $HOME";
        let mut s = settings(json!({"statusLine": {"command": theirs, "padding": 4}}));
        assert_eq!(apply(&mut s, true), Outcome::Changed);
        let wrapped = s["statusLine"]["command"].as_str().unwrap().to_string();
        assert!(wrapped.starts_with(WRAP_PREFIX));
        assert_eq!(s["statusLine"]["padding"], 4);
        // A second pass, with or without --wrap, finds nothing to do.
        assert_eq!(apply(&mut s, true), Outcome::Unchanged);
        assert_eq!(apply(&mut s, false), Outcome::Unchanged);
        assert!(remove(&mut s));
        assert_eq!(s["statusLine"]["command"], theirs);
    }

    #[test]
    fn remove_drops_only_our_own_statusline() {
        let mut s = settings(json!({"statusLine": {"command": COMMAND}, "theme": "auto"}));
        assert!(remove(&mut s));
        assert_eq!(s, settings(json!({"theme": "auto"})));

        let original = settings(json!({"statusLine": {"command": "~/line.sh"}}));
        let mut s = original.clone();
        assert!(!remove(&mut s));
        assert_eq!(s, original);
    }

    #[test]
    fn a_wrap_that_does_not_parse_back_is_kept() {
        let original = settings(json!({"statusLine": {"command": format!("{WRAP_PREFIX}unquoted")}}));
        let mut s = original.clone();
        assert!(!remove(&mut s));
        assert_eq!(s, original);
    }

    #[test]
    fn load_treats_missing_and_empty_as_empty_and_rejects_non_objects() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert!(load(&path).unwrap().is_empty());
        fs::write(&path, "  \n").unwrap();
        assert!(load(&path).unwrap().is_empty());
        fs::write(&path, "[1, 2]").unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn store_creates_the_directory_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("copilot").join("settings.json");
        let mut s = Map::new();
        apply(&mut s, false);
        store(&path, &s).unwrap();
        assert_eq!(load(&path).unwrap(), s);
        assert!(!path.with_extension("json.tmp").exists());
    }
}
