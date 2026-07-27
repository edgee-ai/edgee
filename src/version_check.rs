//! Background "is there a newer release?" check shown on startup.
//!
//! Strategy: the latest known release is cached in a small state file. On every
//! run we compare the current binary version against the cached version, which
//! is instant. The cache is refreshed from GitHub at most once every
//! [`CHECK_INTERVAL_SECS`], with a short timeout so a slow or offline network
//! never delays the CLI noticeably.
//!
//! Disable entirely with `EDGEE_NO_UPDATE_CHECK=1`. The notice is only printed
//! when stderr is a terminal, so piped / CI usage stays quiet.

use std::io::IsTerminal;
use std::time::Duration;

use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::config;

/// How long a cached "latest version" lookup is considered fresh. (24h in seconds)
const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// Hard cap on the GitHub round-trip so startup is never blocked for long.
const FETCH_TIMEOUT: Duration = Duration::from_millis(1500);

const REPO_OWNER: &str = "edgee-ai";
const REPO_NAME: &str = "edgee";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct CheckState {
    /// Unix timestamp (UTC) of the last network refresh attempt.
    last_check: i64,
    /// Latest release version observed from GitHub, if any.
    latest_version: Option<String>,
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

/// Check for a newer release and, if one exists, print an upgrade hint to
/// stderr. Never errors and never blocks for longer than [`FETCH_TIMEOUT`].
pub async fn maybe_notify() {
    if std::env::var_os("EDGEE_NO_UPDATE_CHECK").is_some() {
        return;
    }
    // Only nudge interactive users; stay silent when output is piped or in CI.
    if !std::io::stderr().is_terminal() {
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

    if self_update::version::bump_is_greater(current, latest).unwrap_or(false) {
        eprintln!(
            "\n{} {} → {}\nRun {} to upgrade.\n",
            "A new version of edgee is available:".yellow(),
            current.dimmed(),
            latest.green(),
            "edgee self-update".cyan(),
        );
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
        };
        let encoded = toml::to_string_pretty(&state).unwrap();
        let decoded: CheckState = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.last_check, 1_700_000_000);
        assert_eq!(decoded.latest_version.as_deref(), Some("0.2.8"));
    }

    #[test]
    fn empty_state_parses_to_default() {
        let state: CheckState = toml::from_str("").unwrap();
        assert_eq!(state.last_check, 0);
        assert!(state.latest_version.is_none());
    }
}
