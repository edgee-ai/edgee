//! Rust port of the legacy `statusline.sh` renderer.
//!
//! Reads the Claude Code session JSON from stdin (currently ignored — we use
//! `EDGEE_SESSION_ID` from the environment), fetches a per-session summary
//! from the Edgee API (with an on-disk cache), and prints a single line of
//! ANSI-colored text. When the session ID is missing, the network call fails,
//! and no cached value is available, the renderer prints the bare Edgee
//! marker. It must never crash and must always exit 0.

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

const CACHE_MAX_AGE_SECS: u64 = 8;
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

const PURPLE: &str = "\x1b[38;5;128m";
const BOLD_PURPLE: &str = "\x1b[1;38;5;128m";
const DIM: &str = "\x1b[2m";
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
    println!("{line}");
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
        return format!("{prefix}{PURPLE}三 Edgee{RESET}");
    }

    let stats = fetch_or_cache(&session_id).await;
    format_line(prefix, stats.as_ref())
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

fn format_line(prefix: &str, stats: Option<&SessionSummary>) -> String {
    let Some(stats) = stats else {
        return format!("{prefix}{PURPLE}三 Edgee{RESET}");
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
            "{prefix}{PURPLE}三 Edgee{RESET}  {PURPLE}{bar}{RESET} {BOLD_PURPLE}{pct}%{RESET} tool compression  {DIM}{requests} reqs{RESET}"
        )
    } else if requests > 0 {
        format!("{prefix}{PURPLE}三 Edgee{RESET}  {DIM}{requests} reqs{RESET}")
    } else {
        format!("{prefix}{PURPLE}三 Edgee{RESET}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_no_stats() {
        let s = format_line("", None);
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
        let s = format_line("", Some(&stats));
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
        let s = format_line("", Some(&stats));
        assert!(s.contains("40%"));
        assert!(s.contains("compression"));
        assert!(s.contains("7 reqs"));
    }

    #[test]
    fn format_with_separator_prefix() {
        let s = format_line("| ", None);
        assert!(s.starts_with("| "));
    }
}
