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
    sessions: usize,
    totals: Totals,
    recent: Vec<SessionBrief>,
}

#[derive(Serialize)]
struct Totals {
    requests: u64,
    errors: u64,
    input_tokens: u64,
    output_tokens: u64,
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
        sessions: logs.len(),
        totals: compute_totals(logs),
        recent,
    }
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
        let totals = v.get("totals").expect("totals");
        for key in [
            "requests",
            "errors",
            "input_tokens",
            "output_tokens",
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
}
