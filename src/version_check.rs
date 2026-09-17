//! Background "is there a newer release?" check shown on startup.
//!
//! Strategy: the latest known release is cached in a small state file. On every
//! run we compare the current binary version against the cached version, which
//! is instant. The cache is refreshed from GitHub at most once every
//! [`CHECK_INTERVAL_SECS`], with a short timeout so a slow or offline network
//! never delays the CLI noticeably.
//!
//! Interactive launches offer an update before starting the agent. Deferrals
//! and failures snooze the offer for 24 hours. Managed installs keep their own
//! update channel. Disable checks with `EDGEE_NO_UPDATE_CHECK=1`.

use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Input, Select};
use serde::{Deserialize, Serialize};

use crate::commands::update;
use crate::config;

/// How long a cached "latest version" lookup is considered fresh. (24h in seconds)
const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// Hard cap on the GitHub round-trip so startup is never blocked for long.
const FETCH_TIMEOUT: Duration = Duration::from_millis(1500);

const REPO_OWNER: &str = "edgee-ai";
const REPO_NAME: &str = "edgee";
const RESTART_MARKER: &str = "EDGEE_INTERNAL_UPDATE_RESTART";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct CheckState {
    /// Unix timestamp (UTC) of the last network refresh attempt.
    last_check: i64,
    /// Latest release version observed from GitHub, if any.
    latest_version: Option<String>,
    /// Separate from the network cache: checking must not postpone the prompt.
    remind_after: i64,
}

fn state_path() -> std::path::PathBuf {
    config::global_data_dir().join("update-check.toml")
}

fn read_state() -> CheckState {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_state(state: &CheckState) {
    let path = state_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(content) = toml::to_string_pretty(state) {
        let _ = std::fs::write(&path, content);
    }
}

fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// Fetch the latest release version string from GitHub (blocking).
fn fetch_latest_version() -> anyhow::Result<String> {
    use self_update::backends::github::ReleaseList;

    let releases = ReleaseList::configure()
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .build()?
        .fetch()?;

    releases
        .into_iter()
        .next()
        .map(|r| r.version)
        .ok_or_else(|| anyhow::anyhow!("no releases found"))
}

/// Check for a newer release, optionally offering to update before a launch.
/// Network lookup is bounded; an accepted update can take longer.
pub async fn maybe_notify(is_launch: bool) {
    // Consume the marker so launched agents do not inherit it.
    let restarted = std::env::var_os(RESTART_MARKER).is_some();
    std::env::remove_var(RESTART_MARKER);
    if restarted || std::env::var_os("EDGEE_NO_UPDATE_CHECK").is_some() {
        return;
    }
    if !std::io::stderr().is_terminal() || std::env::var_os("CI").is_some() {
        return;
    }

    let mut state = read_state();

    // Refresh the cached latest version at most once per interval.
    if now_unix() - state.last_check >= CHECK_INTERVAL_SECS {
        let fetched = tokio::time::timeout(
            FETCH_TIMEOUT,
            tokio::task::spawn_blocking(fetch_latest_version),
        )
        .await;

        // Record the attempt regardless of outcome so we don't retry every run.
        state.last_check = now_unix();
        if let Ok(Ok(Ok(version))) = fetched {
            state.latest_version = Some(version);
        }
        write_state(&state);
    }

    let Some(latest) = state.latest_version.as_deref() else {
        return;
    };
    let current = self_update::cargo_crate_version!();

    if !self_update::version::bump_is_greater(current, latest).unwrap_or(false) {
        return;
    }
    let interactive = can_prompt(
        is_launch,
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
        std::env::var_os("CI").is_some(),
    );
    if is_launch && now_unix() < state.remind_after {
        return;
    }
    eprintln!(
        "\n{} {} → {}",
        "A new version of edgee is available:".yellow(),
        current.dimmed(),
        latest.green(),
    );

    let managed = update::self_update_disabled();
    let homebrew = update::installed_with_homebrew();
    if managed {
        eprintln!("Updates are managed by your administrator. Contact them to update Edgee.");
    } else if homebrew {
        eprintln!("Run {} to upgrade.", "brew upgrade edgee".cyan());
    } else if !interactive {
        eprintln!("Run {} to upgrade.", "edgee update".cyan());
    }
    if !interactive {
        return;
    }

    // Save before interacting: errors, dismissal and unsuccessful updates must
    // not result in another prompt on the very next launch.
    let latest = latest.to_owned();
    state.remind_after = now_unix() + CHECK_INTERVAL_SECS;
    write_state(&state);
    if managed || homebrew {
        wait_to_continue();
        return;
    }

    let theme = ColorfulTheme::default();
    let selection = Select::with_theme(&theme)
        .with_prompt("Update Edgee before launching?")
        .items(["Update and continue", "Later"])
        .default(1)
        .interact_on_opt(&console::Term::stderr());
    if !matches!(selection, Ok(Some(0))) {
        return;
    }

    // Capture before replacement: /proc/self/exe can point to a deleted file
    // after updating on Linux.
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Cannot locate Edgee: {error}. Run `edgee update` to retry.");
            wait_to_continue();
            return;
        }
    };
    match update::perform_update(true, Some(latest)).await {
        Ok(true) => {
            if let Err(error) = restart(&executable) {
                eprintln!(
                    "Cannot restart updated Edgee: {error}. Relaunch Edgee to use the new version."
                );
                wait_to_continue();
            }
        }
        Ok(false) => {}
        Err(error) => {
            eprintln!("Update failed: {error:#}");
            eprintln!(
                "Run `edgee update` to retry. For permission errors, contact your administrator."
            );
            wait_to_continue();
        }
    }
}

fn can_prompt(is_launch: bool, stdin: bool, stdout: bool, stderr: bool, ci: bool) -> bool {
    is_launch && stdin && stdout && stderr && !ci
}

fn wait_to_continue() {
    // Keep instructions visible until the user is ready for the agent's TUI.
    let _ = Input::<String>::new()
        .with_prompt("Press Enter to continue launching")
        .allow_empty(true)
        .interact_on(&console::Term::stderr());
}

fn restart_command(
    executable: &Path,
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Command {
    let mut command = Command::new(executable);
    command.args(args).env(RESTART_MARKER, "1");
    command
}

fn restart(executable: &Path) -> std::io::Result<()> {
    let mut command = restart_command(executable, std::env::args_os().skip(1));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec())
    }
    #[cfg(not(unix))]
    {
        let status = command.status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_release_is_greater() {
        assert!(self_update::version::bump_is_greater("0.2.6", "0.2.8").unwrap());
        assert!(!self_update::version::bump_is_greater("0.2.6", "0.2.6").unwrap());
        assert!(!self_update::version::bump_is_greater("0.2.6", "0.2.5").unwrap());
    }

    #[test]
    fn state_round_trips_through_toml() {
        let state = CheckState {
            last_check: 1_700_000_000,
            latest_version: Some("0.2.8".to_string()),
            remind_after: 1_700_086_400,
        };
        let encoded = toml::to_string_pretty(&state).unwrap();
        let decoded: CheckState = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.last_check, 1_700_000_000);
        assert_eq!(decoded.latest_version.as_deref(), Some("0.2.8"));
        assert_eq!(decoded.remind_after, state.remind_after);
    }

    #[test]
    fn empty_state_parses_to_default() {
        let state: CheckState = toml::from_str("").unwrap();
        assert_eq!(state.last_check, 0);
        assert!(state.latest_version.is_none());
        assert_eq!(state.remind_after, 0);
    }

    #[test]
    fn old_cache_does_not_snooze_first_offer() {
        let state: CheckState =
            toml::from_str("last_check = 1700000000\nlatest_version = '0.2.8'\n").unwrap();
        assert_eq!(state.remind_after, 0);
    }

    #[test]
    fn prompts_only_for_interactive_launches() {
        assert!(can_prompt(true, true, true, true, false));
        // Ordinary commands, piped input/output, detached launches and CI.
        assert!(!can_prompt(false, true, true, true, false));
        assert!(!can_prompt(true, false, true, true, false));
        assert!(!can_prompt(true, true, false, true, false));
        assert!(!can_prompt(true, true, true, false, false));
        assert!(!can_prompt(true, true, true, true, true));
    }

    #[test]
    fn restart_preserves_profile_separators_and_agent_arguments() {
        let args: Vec<std::ffi::OsString> = [
            "-p",
            "work",
            "launch",
            "claude",
            "--",
            "-p",
            "prompt with spaces",
            "--",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        let command = restart_command(Path::new("/path with spaces/edgee"), args.clone());
        assert_eq!(command.get_program(), "/path with spaces/edgee");
        assert_eq!(command.get_args().collect::<Vec<_>>(), args);
        assert!(command.get_envs().any(|(key, value)| {
            key == RESTART_MARKER && value == Some(std::ffi::OsStr::new("1"))
        }));
    }
}
