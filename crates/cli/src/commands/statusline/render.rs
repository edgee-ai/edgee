//! Rust port of the legacy `statusline.sh` renderer.
//!
//! Reads the Claude Code session JSON from stdin (currently ignored — we use
//! `EDGEE_SESSION_ID` from the environment), fetches a per-session summary
//! from the Edgee API (with an on-disk cache), and prints a single line of
//! ANSI-colored text. When `EDGEE_SESSION_ID` is missing — e.g. Claude was
//! launched without `edgee launch` — the renderer emits no output so Claude
//! Code hides the statusline entirely. When the session ID is present but the
//! network call fails and no cached value is available, the bare Edgee marker
//! is shown. The renderer must never crash and must always exit 0.
//!
//! Also fetches the caller's org spend-limit status (cached separately, same
//! TTL) and appends a warning segment once usage crosses [`USAGE_WARN_PCT`].
//! Org id / user token come from the local credentials file
//! (`crate::config::read()`), not the environment.

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::api::{ApiClient, UsageLimitStatus};

const CACHE_MAX_AGE_SECS: u64 = 8;
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const USAGE_WARN_PCT: f64 = 80.0;

const PURPLE: &str = "\x1b[38;5;128m";
const BOLD_PURPLE: &str = "\x1b[1;38;5;128m";
const DIM: &str = "\x1b[2m";
const YELLOW: &str = "\x1b[38;5;220m";
const RED: &str = "\x1b[38;5;196m";
const RESET: &str = "\x1b[0m";

#[derive(Debug, Default, Deserialize)]
struct SessionSummary {
    #[serde(default)]
    total_uncompressed_tools_tokens: u64,
    #[serde(default)]
    total_compressed_tools_tokens: u64,
    #[serde(default)]
    total_requests: u64,
}

/// Run as the `edgee statusline` subcommand without `--wrap`.
pub async fn run() -> anyhow::Result<()> {
    // Drain stdin so the upstream invoker doesn't block on an unread pipe.
    let _ = drain_stdin();

    let line = render_with_separator(env_separator()).await;
    if !line.is_empty() {
        println!("{line}");
    }
    Ok(())
}

/// Render the Edgee statusline as a single line. Used both by the standalone
/// `edgee statusline` command and by the `--wrap` path.
///
/// Never blocks longer than [`HTTP_TIMEOUT`] on a network call. Falls back to
/// a minimal output if anything goes wrong.
pub async fn render_line() -> String {
    render_with_separator("").await
}

/// Internal entrypoint for tests and the standalone command.
async fn render_with_separator(prefix: &str) -> String {
    let session_id = std::env::var("EDGEE_SESSION_ID").unwrap_or_default();
    if session_id.is_empty() {
        // No Edgee session in scope (Claude launched outside `edgee launch`).
        // Emit nothing so Claude Code hides the statusline entirely.
        return String::new();
    }

    let stats = fetch_or_cache(&session_id).await;
    let usage = fetch_or_cache_usage().await;
    format_line(prefix, stats.as_ref(), usage.as_ref())
}

fn drain_stdin() -> std::io::Result<()> {
    let mut buf = Vec::new();
    std::io::stdin().lock().read_to_end(&mut buf)?;
    Ok(())
}

fn env_separator() -> &'static str {
    if std::env::var_os("EDGEE_HAS_EXISTING_STATUSLINE").is_some() {
        "| "
    } else {
        ""
    }
}

fn cache_path(session_id: &str) -> PathBuf {
    crate::config::global_config_dir()
        .join("cache")
        .join(format!("statusline-{session_id}.json"))
}

fn usage_cache_path(org_id: &str) -> PathBuf {
    crate::config::global_config_dir()
        .join("cache")
        .join(format!("usage-limit-{org_id}.json"))
}

async fn fetch_or_cache(session_id: &str) -> Option<SessionSummary> {
    let cache_file = cache_path(session_id);
    let cache_fresh = cache_age(&cache_file)
        .map(|age| age < Duration::from_secs(CACHE_MAX_AGE_SECS))
        .unwrap_or(false);

    if cache_fresh {
        if let Some(s) = read_cache(&cache_file) {
            return Some(s);
        }
    }

    if let Some(stats) = fetch_summary(session_id).await {
        if let Some(parent) = cache_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec(&serde_json::json!({
            "total_uncompressed_tools_tokens": stats.total_uncompressed_tools_tokens,
            "total_compressed_tools_tokens": stats.total_compressed_tools_tokens,
            "total_requests": stats.total_requests,
        })) {
            let _ = fs::write(&cache_file, json);
        }
        return Some(stats);
    }

    read_cache(&cache_file)
}

fn cache_age(path: &PathBuf) -> Option<Duration> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

fn read_cache(path: &PathBuf) -> Option<SessionSummary> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Fetches the org's spend-limit status, cached on disk with the same TTL as
/// session stats. Reads org id / token from the local credentials file
/// (`crate::config::read()`) — no env var needed. Silent no-op (falls back to
/// cache, then `None`) on any missing auth, network, or parse failure.
async fn fetch_or_cache_usage() -> Option<UsageLimitStatus> {
    let creds = crate::config::read().ok()?;
    let org_id = creds.org_id.as_deref().filter(|o| !o.is_empty())?;
    let user_token = creds.user_token.as_deref().filter(|t| !t.is_empty())?;

    let cache_file = usage_cache_path(org_id);
    let cache_fresh = cache_age(&cache_file)
        .map(|age| age < Duration::from_secs(CACHE_MAX_AGE_SECS))
        .unwrap_or(false);

    if cache_fresh {
        if let Some(u) = read_usage_cache(&cache_file) {
            return Some(u);
        }
    }

    if let Some(status) = fetch_usage_limit_status(user_token, org_id).await {
        if let Some(parent) = cache_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec(&status) {
            let _ = fs::write(&cache_file, json);
        }
        return Some(status);
    }

    read_usage_cache(&cache_file)
}

fn read_usage_cache(path: &PathBuf) -> Option<UsageLimitStatus> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

async fn fetch_usage_limit_status(user_token: &str, org_id: &str) -> Option<UsageLimitStatus> {
    let client = ApiClient::new(user_token).ok()?;
    tokio::time::timeout(HTTP_TIMEOUT, client.get_usage_limit_status(org_id))
        .await
        .ok()?
        .ok()
}

async fn fetch_summary(session_id: &str) -> Option<SessionSummary> {
    let api_base = std::env::var("EDGEE_CONSOLE_API_URL")
        .unwrap_or_else(|_| "https://api.edgee.app".to_string());
    let url = format!("{api_base}/v1/sessions/{session_id}/summary");

    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .ok()?;

    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<SessionSummary>().await.ok()
}

fn format_line(prefix: &str, stats: Option<&SessionSummary>, usage: Option<&UsageLimitStatus>) -> String {
    let warning = format_usage_warning(usage);

    let Some(stats) = stats else {
        return format!("{prefix}{PURPLE}三 Edgee{RESET}{warning}");
    };

    let before = stats.total_uncompressed_tools_tokens;
    let after = stats.total_compressed_tools_tokens;
    let requests = stats.total_requests;

    if before > 0 && after < before {
        let pct = (before - after) * 100 / before;
        let filled = (pct as usize) / 10;
        let mut bar = String::new();
        for _ in 0..filled.min(10) {
            bar.push('█');
        }
        for _ in filled.min(10)..10 {
            bar.push('░');
        }
        format!(
            "{prefix}{PURPLE}三 Edgee{RESET}  {PURPLE}{bar}{RESET} {BOLD_PURPLE}{pct}%{RESET} tool compression  {DIM}{requests} reqs{RESET}{warning}"
        )
    } else if requests > 0 {
        format!("{prefix}{PURPLE}三 Edgee{RESET}  {DIM}{requests} reqs{RESET}{warning}")
    } else {
        format!("{prefix}{PURPLE}三 Edgee{RESET}{warning}")
    }
}

/// Renders a trailing " ⚠ NN% of $limit used" segment once usage crosses
/// [`USAGE_WARN_PCT`]. Empty string when unmetered, unknown, or below the
/// threshold — never adds visual noise for a healthy account.
fn format_usage_warning(usage: Option<&UsageLimitStatus>) -> String {
    let Some(usage) = usage else {
        return String::new();
    };
    if !usage.has_limit {
        return String::new();
    }
    let Some(pct) = usage.percent_used else {
        return String::new();
    };
    if pct < USAGE_WARN_PCT {
        return String::new();
    }
    let color = if pct >= 100.0 { RED } else { YELLOW };
    let max_usage = usage.max_usage.unwrap_or_default();
    format!("  {color}⚠ {pct:.0}% of ${max_usage:.0} used{RESET}")
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;

    #[test]
    fn format_no_stats() {
        let s = format_line("", None, None);
        assert!(s.contains("三 Edgee"));
        assert!(!s.contains("reqs"));
    }

    #[test]
    fn format_no_compression_with_requests() {
        let stats = SessionSummary {
            total_uncompressed_tools_tokens: 0,
            total_compressed_tools_tokens: 0,
            total_requests: 12,
        };
        let s = format_line("", Some(&stats), None);
        assert!(s.contains("12 reqs"));
        assert!(!s.contains("compression"));
    }

    #[test]
    fn format_with_compression() {
        let stats = SessionSummary {
            total_uncompressed_tools_tokens: 1000,
            total_compressed_tools_tokens: 600,
            total_requests: 7,
        };
        let s = format_line("", Some(&stats), None);
        assert!(s.contains("40%"));
        assert!(s.contains("compression"));
        assert!(s.contains("7 reqs"));
    }

    #[test]
    fn format_with_separator_prefix() {
        let s = format_line("| ", None, None);
        assert!(s.starts_with("| "));
    }

    #[test]
    fn format_no_usage_warning_below_threshold() {
        let usage = UsageLimitStatus {
            has_limit: true,
            max_usage: Some(100.0),
            used_credits: Some(50_000_000_000),
            period: Some("monthly".to_string()),
            percent_used: Some(50.0),
        };
        let s = format_line("", None, Some(&usage));
        assert!(!s.contains('⚠'));
    }

    #[test]
    fn format_usage_warning_above_threshold() {
        let usage = UsageLimitStatus {
            has_limit: true,
            max_usage: Some(100.0),
            used_credits: Some(85_000_000_000),
            period: Some("monthly".to_string()),
            percent_used: Some(85.0),
        };
        let s = format_line("", None, Some(&usage));
        assert!(s.contains('⚠'));
        assert!(s.contains("85%"));
        assert!(s.contains("$100"));
    }

    #[test]
    fn format_no_warning_when_no_limit_configured() {
        let usage = UsageLimitStatus {
            has_limit: false,
            ..Default::default()
        };
        let s = format_line("", None, Some(&usage));
        assert!(!s.contains('⚠'));
    }

    #[tokio::test]
    async fn render_without_session_id_is_empty() {
        let _lock = crate::commands::claude_settings::env_test_lock();
        unsafe {
            std::env::remove_var("EDGEE_SESSION_ID");
        }
        let s = render_with_separator("").await;
        assert!(
            s.is_empty(),
            "expected empty render with no session, got {s:?}"
        );
    }
}
