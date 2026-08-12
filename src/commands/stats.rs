use anyhow::{Result, bail};
use console::style;
use serde::Serialize;

use super::util;

setup_command! {
    /// Limit the number of sessions listed below the latest-session report
    #[arg(long)]
    pub limit: Option<usize>,
    /// Emit machine-readable JSON instead of the human-readable report.
    #[arg(long)]
    pub json: bool,
    /// Org usage window when logged in (JSON): 1h, 3h, 6h, 24h, 7d, 30d.
    #[arg(long, default_value = "24h")]
    pub period: String,
    /// Use local session logs even when logged in (JSON).
    #[arg(long)]
    pub local: bool,
}

/// Compression percentage from before/after tool-token totals, or `None` when
/// there's nothing to compare (matches the human report's blank cell).
fn compression_pct(before: u64, after: u64) -> Option<u64> {
    if before == 0 || after >= before {
        None
    } else {
        Some((before - after) * 100 / before)
    }
}

/// Machine-readable shape of `edgee stats --json`. Consumed by front-ends (the
/// macOS menubar app) so they don't scrape the human report.
#[derive(Serialize)]
struct StatsJson {
    /// "api" (org-wide, windowed) or "local" (this machine's session logs).
    source: &'static str,
    /// The time window when `source == "api"` (e.g. "24h").
    #[serde(skip_serializing_if = "Option::is_none")]
    window: Option<String>,
    sessions: usize,
    /// Live online-session count when `source == "api"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    active_sessions: Option<u64>,
    totals: Totals,
    recent: Vec<SessionBrief>,
}

#[derive(Serialize)]
struct Totals {
    requests: u64,
    errors: u64,
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
    token_cost_savings: u64,
    uncompressed_tools_tokens: u64,
    compressed_tools_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    compression_pct: Option<u64>,
}

#[derive(Serialize)]
struct SessionBrief {
    session_id: String,
    tool_name: String,
    ended_at: String,
    ended_at_unix: i64,
    requests: u64,
    input_tokens: u64,
    output_tokens: u64,
    errors: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    compression_pct: Option<u64>,
    logs_url: String,
}

/// Aggregate totals across all sessions. Computed once and shared by both the
/// JSON and human renderers so they can't drift.
fn compute_totals(logs: &[util::SessionLogEntry]) -> Totals {
    let uncompressed: u64 = logs
        .iter()
        .map(|e| e.stats.total_uncompressed_tools_tokens)
        .sum();
    let compressed: u64 = logs
        .iter()
        .map(|e| e.stats.total_compressed_tools_tokens)
        .sum();
    Totals {
        requests: logs.iter().map(|e| e.stats.total_requests).sum(),
        errors: logs.iter().map(|e| e.stats.total_errors).sum(),
        input_tokens: logs.iter().map(|e| e.stats.total_input_tokens).sum(),
        output_tokens: logs.iter().map(|e| e.stats.total_output_tokens).sum(),
        cached_input_tokens: logs.iter().map(|e| e.stats.total_cached_input_tokens).sum(),
        token_cost_savings: logs.iter().map(|e| e.stats.total_token_cost_savings).sum(),
        uncompressed_tools_tokens: uncompressed,
        compressed_tools_tokens: compressed,
        compression_pct: compression_pct(uncompressed, compressed),
    }
}

fn build_stats_json(logs: &[util::SessionLogEntry], limit: Option<usize>) -> StatsJson {
    let recent = logs
        .iter()
        .take(limit.unwrap_or(logs.len()))
        .map(|e| SessionBrief {
            session_id: e.session_id.clone(),
            tool_name: e.tool_name.clone(),
            ended_at: e.ended_at.clone(),
            ended_at_unix: e.ended_at_unix,
            requests: e.stats.total_requests,
            input_tokens: e.stats.total_input_tokens,
            output_tokens: e.stats.total_output_tokens,
            errors: e.stats.total_errors,
            compression_pct: compression_pct(
                e.stats.total_uncompressed_tools_tokens,
                e.stats.total_compressed_tools_tokens,
            ),
            logs_url: e.logs_url.clone(),
        })
        .collect();

    StatsJson {
        source: "local",
        window: None,
        sessions: logs.len(),
        active_sessions: None,
        totals: compute_totals(logs),
        recent,
    }
}

/// Build the JSON shape from an org-wide API usage summary. The per-session
/// `recent` list is omitted (front-ends don't use it in remote mode).
fn stats_json_from_summary(
    summary: &crate::api::OrgUsageSummary,
    period: &str,
    active: Option<u64>,
) -> StatsJson {
    StatsJson {
        source: "api",
        window: Some(period.to_string()),
        sessions: summary.distinct_sessions as usize,
        active_sessions: active,
        totals: Totals {
            requests: summary.total_requests,
            errors: summary.error_requests,
            input_tokens: summary.input_tokens,
            output_tokens: summary.output_tokens,
            cached_input_tokens: summary.cached_input_tokens,
            token_cost_savings: summary.token_cost_savings,
            uncompressed_tools_tokens: summary.uncompressed_tools_tokens,
            compressed_tools_tokens: summary.compressed_tools_tokens,
            compression_pct: compression_pct(
                summary.uncompressed_tools_tokens,
                summary.compressed_tools_tokens,
            ),
        },
        recent: Vec::new(),
    }
}

/// Fetch org-wide usage from the console API. Returns `None` when not logged in,
/// no org is selected, or any API/network error — so the caller falls back to
/// local session logs.
async fn fetch_remote_stats(period: &str) -> Option<StatsJson> {
    let creds = crate::config::read().ok()?;
    let token = creds.user_token.as_deref().filter(|t| !t.is_empty())?;
    let org = creds.org_id.as_deref().filter(|o| !o.is_empty())?;
    let client = crate::api::ApiClient::new(token).ok()?;
    let summary = client.get_org_usage(org, period).await.ok()?;
    // Online count is best-effort; a failure just leaves `active_sessions` unset.
    let active = client.get_online_sessions_count(org).await.ok();
    Some(stats_json_from_summary(&summary, period, active))
}

fn fmt_compression_cell(before: u64, after: u64) -> (String, bool) {
    let Some(pct) = compression_pct(before, after) else {
        return (format!("{}  -", "░".repeat(8)), false);
    };
    let filled = (pct as usize * 8 / 100).min(8);
    let cell = format!("{}{} {:>2}%", "█".repeat(filled), "░".repeat(8 - filled), pct);
    (cell, true)
}

pub async fn run(opts: Options) -> Result<()> {
    let logs = util::read_all_session_logs()?;

    if opts.json {
        // Prefer org-wide, windowed API usage when logged in; fall back to local
        // session logs when logged out, offline, or `--local`.
        if !opts.local {
            if let Some(remote) = fetch_remote_stats(&opts.period).await {
                return util::emit_json(&remote);
            }
        }
        return util::emit_json(&build_stats_json(&logs, opts.limit));
    }

    if logs.is_empty() {
        bail!(
            "No stored session stats found in {}",
            util::session_logs_dir().display()
        );
    }

    let latest = &logs[0];
    let totals = compute_totals(&logs);

    println!();
    println!(
        "  {}  ·  {} sessions",
        style("Edgee stats").bold(),
        style(logs.len()).cyan()
    );
    println!();
    println!(
        "  {}  {}",
        style("Requests").bold().underlined(),
        style(totals.requests).cyan(),
    );
    println!(
        "  {}     {}    {}  {}    {}  {}",
        style("In").bold().underlined(),
        style(util::fmt_tokens(totals.input_tokens)).cyan(),
        style("Out").bold().underlined(),
        style(util::fmt_tokens(totals.output_tokens)).cyan(),
        style("Errors").bold().underlined(),
        if totals.errors > 0 {
            style(totals.errors.to_string()).red()
        } else {
            style(totals.errors.to_string()).dim()
        },
    );

    println!();
    util::render_session_stats(latest, Some("Latest session"));

    println!("  {}", style("All sessions").bold());
    println!();
    let limit = opts.limit.unwrap_or(logs.len()).max(1);
    let visible_logs: Vec<_> = logs.iter().take(limit).collect();
    let tool_width = visible_logs
        .iter()
        .map(|entry| entry.tool_name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let req_width = visible_logs
        .iter()
        .map(|entry| entry.stats.total_requests.to_string().len())
        .max()
        .unwrap_or(3)
        .max(3);
    let in_width = visible_logs
        .iter()
        .map(|entry| util::fmt_tokens(entry.stats.total_input_tokens).len())
        .max()
        .unwrap_or(2)
        .max(2);
    let out_width = visible_logs
        .iter()
        .map(|entry| util::fmt_tokens(entry.stats.total_output_tokens).len())
        .max()
        .unwrap_or(3)
        .max(3);
    let err_width = visible_logs
        .iter()
        .map(|entry| entry.stats.total_errors.to_string().len())
        .max()
        .unwrap_or(3)
        .max(3);

    println!(
        "  {}  {}  {}  {}  {}  {}  {}",
        style(format!("{:<16}", "ended")).dim().bold(),
        style(format!("{:<tool_width$}", "tool")).dim().bold(),
        style(format!("{:>req_width$}", "req")).dim().bold(),
        style(format!("{:>in_width$}", "in")).dim().bold(),
        style(format!("{:>out_width$}", "out")).dim().bold(),
        style(format!("{:<12}", "compression")).dim().bold(),
        style(format!("{:>err_width$}", "err")).dim().bold(),
    );

    for entry in visible_logs {
        let stats = &entry.stats;
        let (compression, has_compression) = fmt_compression_cell(
            stats.total_uncompressed_tools_tokens,
            stats.total_compressed_tools_tokens,
        );
        let errors = stats.total_errors.to_string();

        println!(
            "  {}  {}  {}  {}  {}  {}  {}",
            style(util::fmt_timestamp(&entry.ended_at)).dim(),
            style(format!("{:<tool_width$}", entry.tool_name)).cyan(),
            style(format!("{:>req_width$}", stats.total_requests)).cyan(),
            style(format!("{:>in_width$}", util::fmt_tokens(stats.total_input_tokens))).cyan(),
            style(format!("{:>out_width$}", util::fmt_tokens(stats.total_output_tokens))).cyan(),
            if has_compression {
                style(format!("{:<12}", compression)).green()
            } else {
                style(format!("{:<12}", compression)).dim()
            },
            if stats.total_errors > 0 {
                style(format!("{:>err_width$}", errors)).red()
            } else {
                style(format!("{:>err_width$}", errors)).dim()
            },
        );
    }
    println!();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compression_pct_guards() {
        assert_eq!(compression_pct(0, 0), None);
        assert_eq!(compression_pct(100, 100), None); // after >= before
        assert_eq!(compression_pct(100, 75), Some(25));
    }

    // Guards the field names the menubar app (Stats.swift) decodes.
    #[test]
    fn json_shape_is_stable() {
        let out = build_stats_json(&[], None);
        let v = serde_json::to_value(&out).unwrap();
        assert!(v.get("sessions").is_some());
        assert!(v.get("recent").is_some());
        assert_eq!(v.get("source").and_then(|s| s.as_str()), Some("local"));
        let totals = v.get("totals").expect("totals");
        for key in [
            "requests",
            "errors",
            "input_tokens",
            "output_tokens",
            "cached_input_tokens",
            "token_cost_savings",
            "uncompressed_tools_tokens",
            "compressed_tools_tokens",
        ] {
            assert!(totals.get(key).is_some(), "missing totals.{key}");
        }

        let brief = SessionBrief {
            session_id: "s".into(),
            tool_name: "Claude".into(),
            ended_at: "2026-01-01T00:00:00Z".into(),
            ended_at_unix: 0,
            requests: 1,
            input_tokens: 2,
            output_tokens: 3,
            errors: 0,
            compression_pct: Some(10),
            logs_url: "https://x".into(),
        };
        let bv = serde_json::to_value(&brief).unwrap();
        for key in [
            "session_id",
            "tool_name",
            "ended_at",
            "ended_at_unix",
            "requests",
            "input_tokens",
            "output_tokens",
            "errors",
            "compression_pct",
            "logs_url",
        ] {
            assert!(bv.get(key).is_some(), "missing session.{key}");
        }
    }

    // The API `/usage` summary envelope decodes and maps onto the JSON shape.
    #[test]
    fn api_usage_maps_to_stats_json() {
        let envelope = r#"{
            "summary": {
                "total_requests": 241,
                "distinct_sessions": 12,
                "error_requests": 3,
                "input_tokens": 189000,
                "cached_input_tokens": 3300000,
                "output_tokens": 34000,
                "token_cost_savings": 42,
                "uncompressed_tools_tokens": 100,
                "compressed_tools_tokens": 60
            },
            "stats_by_time": {},
            "delta": {}
        }"#;
        // Decode just the summary the way the API client does.
        #[derive(serde::Deserialize)]
        struct Env {
            summary: crate::api::OrgUsageSummary,
        }
        let env: Env = serde_json::from_str(envelope).unwrap();
        let out = stats_json_from_summary(&env.summary, "1h", Some(2));
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["source"], "api");
        assert_eq!(v["window"], "1h");
        assert_eq!(v["sessions"], 12);
        assert_eq!(v["active_sessions"], 2);
        assert_eq!(v["totals"]["requests"], 241);
        assert_eq!(v["totals"]["errors"], 3);
        assert_eq!(v["totals"]["cached_input_tokens"], 3_300_000u64);
        assert_eq!(v["totals"]["compression_pct"], 40); // (100-60)/100
        assert_eq!(v["recent"].as_array().unwrap().len(), 0);
    }
}
