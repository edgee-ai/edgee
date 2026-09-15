//! Rust port of the legacy `statusline.sh` renderer.
//!
//! Reads the Claude Code session JSON from stdin (currently ignored — we use
//! `EDGEE_SESSION_ID`/`EDGEE_ORG_SLUG` from the environment), fetches a
//! per-session summary from the Edgee API (with an on-disk cache), and prints
//! a single line of ANSI-colored text. Missing either env var, or a failed
//! network call with no cache, degrades gracefully (no output, or the bare
//! Edgee marker). The renderer must never crash and must always exit 0.

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

const CACHE_MAX_AGE_SECS: u64 = 8;
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

const PURPLE: &str = "\x1b[38;5;128m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

#[derive(Debug, Default, Deserialize)]
struct SessionSummary {
    #[serde(default)]
    total_input_tokens: u64,
    #[serde(default)]
    total_cached_input_tokens: u64,
    #[serde(default)]
    total_cache_creation_input_tokens: u64,
    #[serde(default)]
    total_output_tokens: u64,
    #[serde(default)]
    total_reasoning_output_tokens: u64,
    #[serde(default)]
    total_cost: u64,
    #[serde(default)]
    total_requests: u64,
    #[serde(default)]
    total_fallback_requests: u64,
    #[serde(default)]
    last_request_is_fallback: bool,
    #[serde(default)]
    last_request_model: String,
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
    let org_slug = std::env::var("EDGEE_ORG_SLUG").unwrap_or_default();
    if org_slug.is_empty() {
        // Endpoint is org-scoped; no slug means it's unreachable.
        return String::new();
    }

    let stats = fetch_or_cache(&session_id, &org_slug).await;
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

async fn fetch_or_cache(session_id: &str, org_slug: &str) -> Option<SessionSummary> {
    let cache_file = cache_path(session_id);
    let cache_fresh = cache_age(&cache_file)
        .map(|age| age < Duration::from_secs(CACHE_MAX_AGE_SECS))
        .unwrap_or(false);

    if cache_fresh {
        if let Some(s) = read_cache(&cache_file) {
            return Some(s);
        }
    }

    if let Some(stats) = fetch_summary(session_id, org_slug).await {
        if let Some(parent) = cache_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec(&serde_json::json!({
            "total_input_tokens": stats.total_input_tokens,
            "total_cached_input_tokens": stats.total_cached_input_tokens,
            "total_cache_creation_input_tokens": stats.total_cache_creation_input_tokens,
            "total_output_tokens": stats.total_output_tokens,
            "total_reasoning_output_tokens": stats.total_reasoning_output_tokens,
            "total_cost": stats.total_cost,
            "total_requests": stats.total_requests,
            "total_fallback_requests": stats.total_fallback_requests,
            "last_request_is_fallback": stats.last_request_is_fallback,
            "last_request_model": stats.last_request_model,
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

async fn fetch_summary(session_id: &str, org_slug: &str) -> Option<SessionSummary> {
    let api_base = std::env::var("EDGEE_CONSOLE_API_URL")
        .unwrap_or_else(|_| "https://api.edgee.app".to_string());
    let url = format!("{api_base}/v1/sessions/{org_slug}/{session_id}/summary");

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

    let mut line = format!(
        "{prefix}{PURPLE}三 Edgee{RESET}  {DIM}in {}  cr {}  cw {}  out {}  rsn {}  ${}  {} reqs{RESET}",
        format_tokens(stats.total_input_tokens),
        format_tokens(stats.total_cached_input_tokens),
        format_tokens(stats.total_cache_creation_input_tokens),
        format_tokens(stats.total_output_tokens),
        format_tokens(stats.total_reasoning_output_tokens),
        format_cost(stats.total_cost),
        stats.total_requests,
    );

    if stats.last_request_is_fallback {
        line.push_str(&format!(
            "  {YELLOW}⚠ fallback: {}{RESET}",
            stats.last_request_model
        ));
        if stats.total_fallback_requests > 1 {
            line.push_str(&format!(
                " {DIM}({} this session){RESET}",
                stats.total_fallback_requests
            ));
        }
    }

    line
}

fn format_tokens(tokens: u64) -> String {
    for (scale, suffix) in [
        (1_000_000_000_000_000_000, "E"),
        (1_000_000_000_000_000, "P"),
        (1_000_000_000_000, "T"),
        (1_000_000_000, "G"),
        (1_000_000, "M"),
        (1_000, "k"),
    ] {
        if tokens >= scale {
            return format!("{:.1}{suffix}", tokens as f64 / scale as f64);
        }
    }
    tokens.to_string()
}

fn format_cost(nanodollars: u64) -> String {
    let dollars = nanodollars as f64 / 1_000_000_000.0;
    if nanodollars < 1_000_000_000 {
        format!("{dollars:.2}")
    } else if nanodollars < 10_000_000_000 {
        format!("{dollars:.1}")
    } else {
        format!("{dollars:.0}")
    }
}

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;

    #[test]
    fn format_no_stats() {
        let s = format_line("", None);
        assert!(s.contains("三 Edgee"));
        assert!(!s.contains("reqs"));
    }

    #[test]
    fn format_large_session_compactly() {
        let stats = SessionSummary {
            total_input_tokens: 24_127,
            total_cached_input_tokens: 40_865_520,
            total_cache_creation_input_tokens: 2_594_168,
            total_output_tokens: 196_796,
            total_reasoning_output_tokens: 113_765,
            total_cost: 19_841_300_000,
            total_requests: 571,
            ..Default::default()
        };
        assert_eq!(
            format_line("", Some(&stats)),
            format!("{PURPLE}三 Edgee{RESET}  {DIM}in 24.1k  cr 40.9M  cw 2.6M  out 196.8k  rsn 113.8k  $20  571 reqs{RESET}")
        );
    }

    #[test]
    fn format_token_magnitudes() {
        for (tokens, expected) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1.0k"),
            (1_000_000, "1.0M"),
            (1_000_000_000, "1.0G"),
        ] {
            assert_eq!(format_tokens(tokens), expected);
        }
    }

    #[test]
    fn format_cost_precision_by_magnitude() {
        for (nanodollars, expected) in [
            (0, "0.00"),
            (990_000_000, "0.99"),
            (1_000_000_000, "1.0"),
            (4_123_000_000, "4.1"),
            (10_000_000_000, "10"),
            (19_841_300_000, "20"),
        ] {
            assert_eq!(format_cost(nanodollars), expected);
        }
    }

    #[test]
    fn format_includes_all_token_types_and_cost() {
        let stats = SessionSummary {
            total_input_tokens: 1_234,
            total_cached_input_tokens: 2_345,
            total_cache_creation_input_tokens: 345,
            total_output_tokens: 678,
            total_reasoning_output_tokens: 90,
            total_cost: 12_345_678,
            total_requests: 12,
            ..Default::default()
        };
        let s = format_line("", Some(&stats));
        assert!(s.contains("in 1.2k"));
        assert!(s.contains("cr 2.3k"));
        assert!(s.contains("cw 345"));
        assert!(s.contains("out 678"));
        assert!(s.contains("rsn 90"));
        assert!(s.contains("$0.01"));
        assert!(s.contains("12 reqs"));
        assert!(!s.contains("compression"));
        assert!(!s.contains("fallback"));
    }

    #[test]
    fn format_keeps_zero_value_token_types_visible() {
        let s = format_line("", Some(&SessionSummary::default()));
        assert!(s.contains("in 0"));
        assert!(s.contains("cr 0"));
        assert!(s.contains("cw 0"));
        assert!(s.contains("out 0"));
        assert!(s.contains("rsn 0"));
        assert!(s.contains("$0.00"));
        assert!(s.contains("0 reqs"));
    }

    #[test]
    fn format_with_fallback_warning() {
        let stats = SessionSummary {
            total_requests: 5,
            total_fallback_requests: 3,
            last_request_is_fallback: true,
            last_request_model: "claude-sonnet-5".to_string(),
            ..Default::default()
        };
        let s = format_line("", Some(&stats));
        assert!(s.contains("5 reqs"));
        assert!(s.contains("⚠ fallback: claude-sonnet-5"));
        assert!(s.contains("3 this session"));
        assert!(!s.contains("compression"));
    }

    #[test]
    fn format_with_separator_prefix() {
        let s = format_line("| ", None);
        assert!(s.starts_with("| "));
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

    #[tokio::test]
    async fn render_without_org_slug_is_empty() {
        let _lock = crate::commands::claude_settings::env_test_lock();
        unsafe {
            std::env::set_var("EDGEE_SESSION_ID", "test-session");
            std::env::remove_var("EDGEE_ORG_SLUG");
        }
        let s = render_with_separator("").await;
        unsafe {
            std::env::remove_var("EDGEE_SESSION_ID");
        }
        assert!(
            s.is_empty(),
            "expected empty render with no org slug, got {s:?}"
        );
    }
}
