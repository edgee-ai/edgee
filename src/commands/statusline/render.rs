//! `edgee statusline`: one ANSI line of live session totals.
//!
//! Copilot CLI pipes its own session JSON to stdin; it is ignored, since the
//! session is identified by the `EDGEE_*` env that `edgee launch` sets. Without
//! that env (a plain `copilot`) nothing is printed, so no segment shows up. The
//! renderer never fails: a slow or unreachable API falls back to the last cached
//! summary, then to the bare marker.
//!
//! The line mirrors the minimized band of the Claude Code mod (`mods/edgee/`),
//! plus cost and savings, which only the gateway knows.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::api::{ApiClient, SessionSummary};

/// A cached summary younger than this is served without calling the API.
const CACHE_TTL: Duration = Duration::from_secs(8);
const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

const ACCENT: &str = "\x1b[1;38;5;141m";
const YELLOW: &str = "\x1b[33m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

pub async fn run() {
    // Drain stdin so the invoker never blocks on an unread pipe.
    let _ = std::io::stdin().lock().read_to_end(&mut Vec::new());
    let line = render().await;
    if !line.is_empty() {
        println!("{line}");
    }
}

/// The statusline, or an empty string outside an Edgee session. Also used by
/// `--wrap`.
pub async fn render() -> String {
    let (Some(session_id), Some(org)) = (
        env("EDGEE_SESSION_ID"),
        env("EDGEE_ORG_ID").or_else(|| env("EDGEE_ORG_SLUG")),
    ) else {
        return String::new();
    };
    format_line(summary(&session_id, &org).await.as_ref())
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

async fn summary(session_id: &str, org: &str) -> Option<SessionSummary> {
    let cache = crate::config::global_config_dir()
        .join("cache")
        .join(format!("statusline-{session_id}.json"));
    if let Some(fresh) = read_cache(&cache, Some(CACHE_TTL)) {
        return Some(fresh);
    }
    match fetch(session_id, org).await {
        Some(summary) => {
            write_cache(&cache, &summary);
            Some(summary)
        }
        None => read_cache(&cache, None),
    }
}

async fn fetch(session_id: &str, org: &str) -> Option<SessionSummary> {
    let token = crate::config::read().ok()?.user_token.filter(|t| !t.is_empty())?;
    let client = ApiClient::new(&token).ok()?;
    tokio::time::timeout(FETCH_TIMEOUT, client.get_session_summary(org, session_id))
        .await
        .ok()?
        .ok()
}

/// `max_age: None` accepts a cache of any age (the API is unreachable).
fn read_cache(path: &Path, max_age: Option<Duration>) -> Option<SessionSummary> {
    if let Some(max_age) = max_age {
        let modified = fs::metadata(path).ok()?.modified().ok()?;
        if SystemTime::now().duration_since(modified).ok()? >= max_age {
            return None;
        }
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

fn write_cache(path: &Path, summary: &SessionSummary) {
    let Ok(json) = serde_json::to_vec(summary) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, json);
}

/// `◆ Edgee  claude-sonnet-4.6 · 142 req · 4.1M↑ 197k↓ · 92% cache · $1.24 · $0.31 saved`
///
/// `↑` counts every input token, cached or not, and the cache share is of that
/// same total, as in the Claude mod's band.
fn format_line(summary: Option<&SessionSummary>) -> String {
    let marker = format!("{ACCENT}◆ Edgee{RESET}");
    let Some(s) = summary.filter(|s| s.total_requests > 0) else {
        return marker;
    };

    let input = s.total_input_tokens + s.total_cached_input_tokens;
    let mut facts = Vec::new();
    if !s.last_request_model.is_empty() {
        facts.push(s.last_request_model.clone());
    }
    facts.push(format!("{} req", s.total_requests));
    facts.push(format!(
        "{}↑ {}↓",
        short(input),
        short(s.total_output_tokens)
    ));
    if input > 0 {
        let hit = s.total_cached_input_tokens as f64 / input as f64;
        facts.push(format!("{:.0}% cache", hit * 100.0));
    }
    facts.push(format!("${}", dollars(s.total_cost)));
    if s.total_savings() > 0 {
        facts.push(format!("${} saved", dollars(s.total_savings())));
    }

    let mut line = format!("{marker}  {DIM}{}{RESET}", facts.join(" · "));
    if s.last_request_is_fallback {
        line.push_str(&format!("{DIM} · {RESET}{YELLOW}⚠ fallback"));
        if s.total_fallback_requests > 1 {
            line.push_str(&format!(" ({}×)", s.total_fallback_requests));
        }
        line.push_str(RESET);
    }
    line
}

fn short(n: u64) -> String {
    let (value, suffix) = match n {
        1_000_000.. => (n as f64 / 1e6, "M"),
        1_000.. => (n as f64 / 1e3, "k"),
        _ => return n.to_string(),
    };
    format!("{value:.1}{suffix}").replace(".0", "")
}

/// Nanodollars to a dollar amount: cents under $100, whole dollars above.
fn dollars(nanodollars: u64) -> String {
    let d = nanodollars as f64 / 1e9;
    if d < 100.0 {
        format!("{d:.2}")
    } else {
        format!("{d:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> SessionSummary {
        SessionSummary {
            total_requests: 142,
            total_cost: 1_240_000_000,
            total_input_tokens: 328_000,
            total_cached_input_tokens: 3_772_000,
            total_output_tokens: 197_000,
            last_request_model: "claude-sonnet-4.6".into(),
            ..Default::default()
        }
    }

    fn plain(line: &str) -> String {
        console::strip_ansi_codes(line).into_owned()
    }

    #[test]
    fn bare_marker_until_the_first_request() {
        assert_eq!(plain(&format_line(None)), "◆ Edgee");
        assert_eq!(
            plain(&format_line(Some(&SessionSummary::default()))),
            "◆ Edgee"
        );
    }

    #[test]
    fn line_follows_the_claude_band() {
        assert_eq!(
            plain(&format_line(Some(&summary()))),
            "◆ Edgee  claude-sonnet-4.6 · 142 req · 4.1M↑ 197k↓ · 92% cache · $1.24"
        );
    }

    #[test]
    fn savings_are_shown_only_when_there_are_some() {
        let mut s = summary();
        s.total_tool_compression_cost_savings = 200_000_000;
        s.total_output_cost_savings = 110_000_000;
        assert!(plain(&format_line(Some(&s))).ends_with("· $1.24 · $0.31 saved"));
    }

    #[test]
    fn fallback_is_flagged_with_its_count() {
        let mut s = summary();
        s.last_request_is_fallback = true;
        assert!(plain(&format_line(Some(&s))).ends_with("$1.24 · ⚠ fallback"));
        s.total_fallback_requests = 3;
        assert!(plain(&format_line(Some(&s))).ends_with("⚠ fallback (3×)"));
    }

    #[test]
    fn cache_share_is_skipped_without_input() {
        let s = SessionSummary {
            total_requests: 1,
            ..Default::default()
        };
        assert_eq!(plain(&format_line(Some(&s))), "◆ Edgee  1 req · 0↑ 0↓ · $0.00");
    }

    #[test]
    fn short_counts() {
        for (n, expected) in [
            (0, "0"),
            (999, "999"),
            (1_000, "1k"),
            (1_500, "1.5k"),
            (4_100_000, "4.1M"),
            (2_000_000, "2M"),
        ] {
            assert_eq!(short(n), expected);
        }
    }

    #[test]
    fn dollar_amounts() {
        assert_eq!(dollars(0), "0.00");
        assert_eq!(dollars(1_240_000_000), "1.24");
        assert_eq!(dollars(99_994_000_000), "99.99");
        assert_eq!(dollars(150_000_000_000), "150");
    }

    #[test]
    fn cache_honours_its_max_age() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache").join("s.json");
        write_cache(&path, &summary());

        assert_eq!(read_cache(&path, Some(CACHE_TTL)).unwrap().total_requests, 142);
        assert!(read_cache(&path, Some(Duration::ZERO)).is_none());
        assert_eq!(read_cache(&path, None).unwrap().total_requests, 142);
        assert!(read_cache(&dir.path().join("missing.json"), None).is_none());
    }
}
