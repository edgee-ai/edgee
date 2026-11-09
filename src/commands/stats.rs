use anyhow::{Context, Result, bail};
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
    /// Usage window when logged in: 1h, 3h, 6h, 24h, 7d, 30d.
    /// Defaults to 7d (the console dashboard's default), or 24h with --json.
    #[arg(long, value_parser = ["1h", "3h", "6h", "24h", "7d", "30d"])]
    pub period: Option<String>,
    /// Use this machine's session logs even when logged in.
    #[arg(long)]
    pub local: bool,
    /// Read live savings for one session (JSON, nano-USD).
    #[arg(long, requires = "json", conflicts_with_all = ["local", "limit"])]
    pub session: Option<String>,
    /// Read live savings using an explicit Edgee launch alias.
    #[arg(long, hide = true, requires = "json", conflicts_with_all = ["session", "local", "limit"])]
    pub launch_session: Option<String>,
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
    /// "api" (your usage across devices, windowed) or "local" (this machine's
    /// session logs).
    source: &'static str,
    /// The time window when `source == "api"` (e.g. "24h").
    #[serde(skip_serializing_if = "Option::is_none")]
    window: Option<String>,
    sessions: usize,
    /// Live online-session count when `source == "api"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    active_sessions: Option<u64>,
    /// True only when the latest-request lookup succeeded (including an empty result).
    last_request_checked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_request: Option<crate::api::LastRequest>,
    totals: Totals,
    recent: Vec<SessionBrief>,
    /// Dashboard-equivalent figures, present only when `source == "api"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    insights: Option<Insights>,
}

/// The console Overview's derived figures, computed with the console's formulas
/// so `edgee stats` and the dashboard show the same numbers.
#[derive(Serialize)]
struct Insights {
    savings: Savings,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_hit_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avg_latency_ms: Option<f64>,
    total_tokens: u64,
    input_cost_usd: f64,
    output_cost_usd: f64,
    routed_requests: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    delta: Option<crate::api::UsageDelta>,
    top_models: Vec<Breakdown>,
    coding_agents: Vec<Breakdown>,
}

#[derive(Serialize, Debug, PartialEq)]
struct Breakdown {
    label: String,
    cost_usd: f64,
    requests: u64,
    tokens: u64,
    /// Share of total cost, 0–100.
    pct: f64,
}

/// Mirrors the console's `dashboardSavings` (`dashboard-savings.utils.ts`).
/// All amounts nano-USD.
#[derive(Serialize, Debug, PartialEq)]
struct Savings {
    /// Tool result trimming + tool surface reduction + output brevity.
    compression: u64,
    /// Signed routing estimate; `None` when no request carried a priced estimate.
    routing: Option<i64>,
    /// compression + max(0, routing).
    saved: u64,
    /// What the window would have cost without Edgee: total_cost + saved.
    baseline: u64,
    /// saved / baseline, 0–100, one decimal; `None` with no usage.
    pct: Option<f64>,
}

/// Nano-USD (integer, additive-safe) to dollars.
fn nano_usd_to_dollars(nano: u64) -> f64 {
    nano as f64 / 1_000_000_000.0
}

#[derive(Serialize)]
struct Totals {
    requests: u64,
    errors: u64,
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_input_tokens: u64,
    reasoning_output_tokens: u64,
    cost_usd: f64,
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
    cost_usd: f64,
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
        cache_creation_input_tokens: logs.iter().map(|e| e.stats.total_cache_creation_input_tokens).sum(),
        reasoning_output_tokens: logs.iter().map(|e| e.stats.total_reasoning_output_tokens).sum(),
        cost_usd: nano_usd_to_dollars(logs.iter().map(|e| e.stats.total_cost).sum()),
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
            session_id: e.visible_session_id().to_string(),
            tool_name: e.tool_name.clone(),
            ended_at: e.ended_at.clone(),
            ended_at_unix: e.ended_at_unix,
            requests: e.stats.total_requests,
            input_tokens: e.stats.total_input_tokens,
            output_tokens: e.stats.total_output_tokens,
            errors: e.stats.total_errors,
            cost_usd: nano_usd_to_dollars(e.stats.total_cost),
            compression_pct: compression_pct(
                e.stats.total_uncompressed_tools_tokens,
                e.stats.total_compressed_tools_tokens,
            ),
            logs_url: e.logs_url.clone(),
        })
        .collect();

    StatsJson {
        source: "local",
        last_request_checked: false,
        last_request: None,
        window: None,
        sessions: logs.len(),
        active_sessions: None,
        totals: compute_totals(logs),
        recent,
        insights: None,
    }
}

fn clamp_pct(v: f64) -> f64 {
    (v.clamp(0.0, 100.0) * 10.0).round() / 10.0
}

fn compute_savings(
    summary: &crate::api::OrgUsageSummary,
    fallback: Option<&crate::api::FallbackSummary>,
) -> Savings {
    let compression = summary.tool_compression_cost_savings
        + summary.mcp_surface_cost_savings
        + summary.output_cost_savings;
    let routing = fallback
        .filter(|f| f.routing_savings_estimate_count > 0)
        .and_then(|f| f.estimated_routing_savings);
    let saved = compression + routing.unwrap_or(0).max(0) as u64;
    let baseline = summary.total_cost + saved;
    let pct = (baseline > 0).then(|| clamp_pct(saved as f64 * 100.0 / baseline as f64));
    Savings { compression, routing, saved, baseline, pct }
}

/// `input_tokens` excludes the cached and cache-write buckets, so the
/// denominator adds them back (same as the console's cache hit rate).
fn cache_hit_pct(s: &crate::api::OrgUsageSummary) -> Option<f64> {
    let all_input = s.input_tokens + s.cached_input_tokens + s.cache_creation_input_tokens;
    (all_input > 0).then(|| clamp_pct(s.cached_input_tokens as f64 * 100.0 / all_input as f64))
}

/// Group usage rows by `key`, largest cost first, with each row's share of `total_cost`.
fn breakdown<'a>(
    rows: impl Iterator<Item = (String, &'a crate::api::UsageByModel)>,
    total_cost: u64,
) -> Vec<Breakdown> {
    let mut acc: Vec<(String, u64, u64, u64)> = Vec::new();
    for (label, row) in rows {
        match acc.iter_mut().find(|(l, ..)| *l == label) {
            Some(e) => {
                e.1 += row.total_cost;
                e.2 += row.total_requests;
                e.3 += row.total_tokens;
            }
            None => acc.push((label, row.total_cost, row.total_requests, row.total_tokens)),
        }
    }
    acc.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    acc.into_iter()
        .map(|(label, cost, requests, tokens)| Breakdown {
            label,
            cost_usd: nano_usd_to_dollars(cost),
            requests,
            tokens,
            pct: if total_cost > 0 {
                clamp_pct(cost as f64 * 100.0 / total_cost as f64)
            } else {
                0.0
            },
        })
        .collect()
}

/// Display name for a console `coding_assistant` value (console's `CODING_AGENTS`).
fn coding_agent_label(value: &str) -> String {
    match value {
        "api" => "API",
        "claude_code" => "Claude Code",
        "claude_desktop" => "Claude Desktop",
        "codebuddy" => "CodeBuddy",
        "codex" => "Codex",
        "codex_desktop" => "Codex Desktop",
        "opencode" => "OpenCode",
        "cursor" => "Cursor",
        "copilot" => "GitHub Copilot",
        "crush" => "Crush",
        "pi" => "Pi",
        "kimi" => "Kimi Code",
        "kilo" => "Kilo Code",
        "deepseek" => "DeepSeek Harness",
        other => other,
    }
    .to_string()
}

/// Coding agent for a key, or `None` when it isn't a coding-agent key.
fn key_agent(usage: &crate::api::OrgUsage, key_uuid: &str) -> Option<String> {
    usage
        .stats_by_api_key
        .iter()
        .find(|k| k.id == key_uuid)
        .filter(|k| k.key_type.as_deref() == Some("coding_agent"))
        .and_then(|k| k.coding_assistant.as_deref())
        .map(coding_agent_label)
}

fn build_insights(usage: &crate::api::OrgUsage) -> Insights {
    let s = &usage.summary;
    let top_models = breakdown(
        usage.stats_by_model.iter().map(|r| (r.model.clone(), r)),
        s.total_cost,
    );
    let coding_agents = breakdown(
        usage
            .stats_by_model
            .iter()
            .map(|r| (key_agent(usage, &r.key_uuid).unwrap_or_else(|| "Other".into()), r)),
        s.total_cost,
    );
    Insights {
        savings: compute_savings(s, usage.fallback.as_ref()),
        cache_hit_pct: cache_hit_pct(s),
        avg_latency_ms: s.avg_duration_ms.filter(|ms| *ms > 0.0),
        total_tokens: s.total_tokens,
        input_cost_usd: nano_usd_to_dollars(
            s.input_cost + s.cached_input_cost + s.cache_creation_input_cost,
        ),
        output_cost_usd: nano_usd_to_dollars(s.output_cost + s.reasoning_output_cost),
        routed_requests: usage.fallback.as_ref().map_or(0, |f| f.routed_requests),
        delta: usage.delta.clone(),
        top_models,
        coding_agents,
    }
}

/// Build the JSON shape from an API usage payload. The per-session `recent`
/// list stays empty (front-ends don't use it in remote mode).
fn stats_json_from_usage(
    usage: &crate::api::OrgUsage,
    period: &str,
    active: Option<u64>,
) -> StatsJson {
    let summary = &usage.summary;
    StatsJson {
        source: "api",
        last_request_checked: false,
        last_request: None,
        window: Some(period.to_string()),
        sessions: summary.distinct_sessions as usize,
        active_sessions: active,
        totals: Totals {
            requests: summary.total_requests,
            errors: summary.error_requests,
            input_tokens: summary.input_tokens,
            output_tokens: summary.output_tokens,
            cached_input_tokens: summary.cached_input_tokens,
            cache_creation_input_tokens: summary.cache_creation_input_tokens,
            reasoning_output_tokens: summary.reasoning_output_tokens,
            cost_usd: nano_usd_to_dollars(summary.total_cost),
            token_cost_savings: summary.token_cost_savings,
            uncompressed_tools_tokens: summary.uncompressed_tools_tokens,
            compressed_tools_tokens: summary.compressed_tools_tokens,
            compression_pct: compression_pct(
                summary.uncompressed_tools_tokens,
                summary.compressed_tools_tokens,
            ),
        },
        recent: Vec::new(),
        insights: Some(build_insights(usage)),
    }
}

/// Everything the API-backed report needs, fetched once.
struct RemoteStats {
    json: StatsJson,
    sessions: Vec<crate::api::ApiSession>,
    /// key id -> coding agent label, for the sessions table.
    agents: Vec<(String, String)>,
    org_slug: Option<String>,
}

const RECENT_SESSIONS: usize = 8;

/// Fetch the signed-in user's usage within their org from the console API.
/// `Ok(None)` when not logged in or no org is selected; `Err` on API/network
/// failure, so callers can say why they fell back to local logs.
async fn fetch_remote_stats(period: &str, sessions_limit: usize) -> Result<Option<RemoteStats>> {
    let Ok(creds) = crate::config::read() else {
        return Ok(None);
    };
    let Some(token) = creds.user_token.as_deref().filter(|t| !t.is_empty()) else {
        return Ok(None);
    };
    let Some(org) = creds.org_id.as_deref().filter(|o| !o.is_empty()) else {
        return Ok(None);
    };
    // Scope to this account: without it an org admin gets the whole org's numbers,
    // which is not what "my stats" means. Credentials predating `user_id` leave it
    // unset — the API still pins regular members to their own rows.
    let user = creds.user_id.as_deref().filter(|u| !u.is_empty());
    let client = crate::api::ApiClient::new(token)?;

    let keys_fut = async {
        match user {
            Some(user) => client.get_owned_key_ids(org, user).await.map(Some),
            None => Ok(None),
        }
    };
    let (usage, active, keys) = tokio::join!(
        client.get_org_usage(org, period, user),
        // Online count is best-effort; a failure just leaves `active_sessions` unset.
        client.get_online_sessions_count(org, user),
        keys_fut,
    );
    let usage = usage?;
    let mut json = stats_json_from_usage(&usage, period, active.ok());

    // Requests and sessions ignore `user_id` for admins, so both are filtered by
    // the user's own keys and skipped when those can't be resolved.
    let mut sessions = Vec::new();
    if let Ok(Some(key_ids)) = keys {
        let sessions_fut = async {
            if sessions_limit > 0 {
                client.get_sessions(org, period, &key_ids, sessions_limit).await
            } else {
                Ok(Vec::new())
            }
        };
        let (last, recent) = tokio::join!(
            client.get_last_request(org, period, &key_ids),
            sessions_fut,
        );
        if let Ok(request) = last {
            json.last_request_checked = true;
            json.last_request = request;
        }
        sessions = recent.unwrap_or_default();
    }

    let agents = usage
        .stats_by_api_key
        .iter()
        .filter_map(|k| key_agent(&usage, &k.id).map(|a| (k.id.clone(), a)))
        .collect();
    Ok(Some(RemoteStats {
        json,
        sessions,
        agents,
        org_slug: creds.org_slug.clone().filter(|s| !s.is_empty()),
    }))
}

pub async fn run(opts: Options) -> Result<()> {
    if let Some(session_id) = opts.session.as_ref().or(opts.launch_session.as_ref()) {
        let creds = crate::config::read()?;
        let token = creds.user_token.as_deref().filter(|s| !s.is_empty())
            .context("Log in with `edgee auth login` to read session savings")?;
        let org = creds.org_id.as_deref().filter(|s| !s.is_empty())
            .context("Select an organization with `edgee auth login` to read session savings")?;
        let client = crate::api::ApiClient::new(token)?;
        let savings = if opts.launch_session.is_some() {
            client.get_launch_session_savings(org, session_id).await?
        } else {
            client.get_session_savings(org, session_id).await?
        };
        return util::emit_json(&savings);
    }

    if opts.json {
        // Prefer the account's windowed API usage when logged in; fall back to local
        // session logs when logged out, offline, or `--local`. The JSON default
        // window stays 24h: the menubar app relies on it.
        let period = opts.period.as_deref().unwrap_or("24h");
        if !opts.local {
            if let Ok(Some(remote)) = fetch_remote_stats(period, 0).await {
                return util::emit_json(&remote.json);
            }
        }
        let logs = util::read_all_session_logs()?;
        return util::emit_json(&build_stats_json(&logs, opts.limit));
    }

    // Human report: API first, like the console dashboard, so both show the same
    // numbers. Say why when falling back, instead of silently switching source.
    let period = opts.period.as_deref().unwrap_or("7d");
    let fallback_note = if opts.local {
        None
    } else {
        match fetch_remote_stats(period, opts.limit.unwrap_or(RECENT_SESSIONS).clamp(1, 200)).await {
            Ok(Some(remote)) => {
                render_remote(&remote, period, opts.limit);
                return Ok(());
            }
            Ok(None) => Some(
                "Not logged in · showing this machine only. Run `edgee auth login` for usage across all devices."
                    .to_string(),
            ),
            Err(err) => Some(format!("Edgee API unavailable: {err:#} · showing this machine only.")),
        }
    };

    let logs = util::read_all_session_logs()?;
    if logs.is_empty() {
        if let Some(note) = fallback_note {
            eprintln!("  {}", style(note).dim());
        }
        bail!(
            "No stored session stats found in {}",
            util::session_logs_dir().display()
        );
    }
    render_local(&logs, opts.limit, fallback_note.as_deref());
    Ok(())
}

fn fmt_usd(dollars: f64) -> String {
    let abs = dollars.abs();
    let sign = if dollars < 0.0 { "-" } else { "" };
    if abs >= 1_000_000.0 {
        format!("{sign}${:.2}M", abs / 1_000_000.0)
    } else if abs >= 10_000.0 {
        format!("{sign}${:.1}k", abs / 1_000.0)
    } else {
        format!("{sign}${abs:.2}")
    }
}

fn fmt_nano_usd(nano: u64) -> String {
    fmt_usd(nano_usd_to_dollars(nano))
}

/// 1234 -> "1.2k", 3_400_000 -> "3.4M" (the console's compact numbers).
fn fmt_compact(n: u64) -> String {
    match n {
        n if n >= 1_000_000_000 => format!("{:.1}B", n as f64 / 1e9),
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

/// "▲ 12%" vs the previous window. `up_is_bad` colors a rise red (cost),
/// matching the console's inverted cost delta.
fn fmt_delta(pct: Option<f64>, up_is_bad: bool) -> String {
    let Some(p) = pct.filter(|p| p.is_finite()) else {
        return String::new();
    };
    if p.abs() < 0.5 {
        return style("  = 0%".to_string()).dim().to_string();
    }
    let arrow = if p > 0.0 { "▲" } else { "▼" };
    let text = format!("  {arrow} {:.0}%", p.abs());
    if (p > 0.0) == up_is_bad {
        style(text).red().to_string()
    } else {
        style(text).green().to_string()
    }
}

fn period_label(period: &str) -> String {
    match period {
        "1h" => "last hour".into(),
        "3h" => "last 3 hours".into(),
        "6h" => "last 6 hours".into(),
        "24h" => "last 24 hours".into(),
        "7d" => "last 7 days".into(),
        "30d" => "last 30 days".into(),
        other => format!("last {other}"),
    }
}

// Palette shared with the Claude mod (`mods/edgee/hooks/edgee.ts`), so the
// terminal report and the in-agent pane read as one product.
const ACCENT: &str = "#A78BFA";
const COMPRESSION_COLOR: &str = "#55B98A";
const ROUTING_COLOR: &str = "#B12ACB";
const REMAINING_COLOR: &str = "#60566B";

fn provider_color(model: &str) -> &'static str {
    let provider = model.split('/').next().unwrap_or(model).to_ascii_lowercase();
    let guess = |m: &str| model.to_ascii_lowercase().contains(m);
    match provider.as_str() {
        "anthropic" => "#D97757",
        "openai" => "#10A37F",
        "google" => "#4285F4",
        "qwen" => "#8B5CF6",
        "deepseek" => "#4D6BFE",
        "mistral" | "mistralai" => "#FA520F",
        "meta" | "meta-llama" => "#3B82F6",
        "xai" | "x-ai" => "#E5E7EB",
        "zai" | "z-ai" => "#22D3EE",
        "moonshotai" => "#F472B6",
        "minimax" => "#F43F5E",
        "nvidia" => "#76B900",
        "amazon" => "#FF9900",
        _ if guess("claude") => "#D97757",
        _ if guess("gpt") => "#10A37F",
        _ if guess("gemini") => "#4285F4",
        _ if guess("kimi") => "#F472B6",
        _ => ACCENT,
    }
}

fn hex_rgb(hex: &str) -> (u8, u8, u8) {
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0xA78BFA);
    ((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Nearest xterm-256 color-cube entry, for terminals without truecolor.
fn rgb_to_256((r, g, b): (u8, u8, u8)) -> u8 {
    let level = |c: u8| -> u8 {
        match c {
            0..=47 => 0,
            48..=114 => 1,
            _ => (c - 35) / 40,
        }
    };
    16 + 36 * level(r) + 6 * level(g) + level(b)
}

/// Paint `text` in a hex color: 24-bit when the terminal advertises it,
/// 256-color otherwise, plain when colors are off (pipes, NO_COLOR).
fn paint(text: impl std::fmt::Display, hex: &str, bold: bool) -> String {
    let text = text.to_string();
    if !console::colors_enabled() {
        return text;
    }
    let rgb = hex_rgb(hex);
    let truecolor = std::env::var("COLORTERM")
        .map(|v| v.contains("truecolor") || v.contains("24bit"))
        .unwrap_or(false);
    let fg = if truecolor {
        format!("38;2;{};{};{}", rgb.0, rgb.1, rgb.2)
    } else {
        format!("38;5;{}", rgb_to_256(rgb))
    };
    let weight = if bold { "1;" } else { "" };
    format!("\x1b[{weight}{fg}m{text}\x1b[0m")
}

fn report_width() -> usize {
    let cols = console::Term::stdout().size_checked().map_or(80, |(_, c)| c as usize);
    cols.saturating_sub(4).clamp(40, 76)
}

/// `── TITLE ─────`, the mod's section rule.
fn rule(title: &str, width: usize) {
    let tail = width.saturating_sub(title.len() + 4);
    println!();
    println!(
        "  {}{}{}",
        style(paint("── ", ACCENT, false)).dim(),
        paint(title.to_uppercase(), ACCENT, true),
        style(paint(format!(" {}", "─".repeat(tail)), ACCENT, false)).dim(),
    );
}

/// Share bar: `filled` cells of `▰` in `color`, the rest dim `▱`.
fn share_bar(pct: f64, width: usize, color: &str) -> String {
    let filled = ((pct / 100.0) * width as f64).round().clamp(0.0, width as f64) as usize;
    let filled = if pct > 0.0 { filled.max(1) } else { 0 };
    format!(
        "{}{}",
        paint("▰".repeat(filled), color, false),
        style("▱".repeat(width - filled)).dim()
    )
}

fn render_breakdown(title: &str, items: &[Breakdown], limit: usize, width: usize, color_of: impl Fn(&str) -> &'static str) {
    if items.is_empty() {
        return;
    }
    rule(title, width);
    let name_width = items.iter().take(limit).map(|i| i.label.chars().count()).max().unwrap_or(0).min(32);
    let bar = width.saturating_sub(name_width + 24).clamp(6, 20);
    for item in items.iter().take(limit) {
        let color = color_of(&item.label);
        let label: String = item.label.chars().take(name_width).collect();
        println!(
            "  {} {}  {}  {}  {}",
            paint("●", color, false),
            paint(format!("{label:<name_width$}"), color, true),
            share_bar(item.pct, bar, color),
            style(format!("{:>9}", fmt_usd(item.cost_usd))).bold(),
            style(format!("{:>5.1}%", item.pct)).dim(),
        );
    }
}

fn render_remote(remote: &RemoteStats, period: &str, limit: Option<usize>) {
    let json = &remote.json;
    let Some(insights) = json.insights.as_ref() else {
        return;
    };
    let totals = &json.totals;
    let delta = insights.delta.as_ref();
    let savings = &insights.savings;
    let width = report_width();
    let label = |s: &str| style(format!("{s:<11}")).dim();

    println!();
    let mut header = format!(
        "  {}  {}",
        paint("◆ EDGEE", ACCENT, true),
        style(format!("{} · all devices", period_label(period))).dim(),
    );
    if let Some(active) = json.active_sessions.filter(|n| *n > 0) {
        header.push_str(&format!(
            "  {} {}",
            style("●").green(),
            style(format!("{active} online")).green()
        ));
    }
    println!("{header}");

    if totals.requests == 0 {
        println!();
        println!(
            "  {}",
            style("No gateway usage in this window. Try a longer one: `edgee stats --period 30d`.").dim()
        );
        println!();
        return;
    }

    // ── Savings: the mod's savings card, with a stacked bar for the donut.
    rule("Cost savings", width);
    let pct_of = |nano: u64| -> f64 {
        if savings.baseline == 0 {
            0.0
        } else {
            nano as f64 * 100.0 / savings.baseline as f64
        }
    };
    let routing_saved = savings.routing.unwrap_or(0).max(0) as u64;
    let paid = savings.baseline.saturating_sub(savings.saved);
    println!(
        "  {}  {}",
        paint(fmt_nano_usd(savings.saved), COMPRESSION_COLOR, true),
        style(format!(
            "saved · {} cost reduction",
            savings.pct.map_or("—".into(), |p| format!("{p:.1}%"))
        ))
        .dim(),
    );
    let bar = width.saturating_sub(2);
    let cells = |nano: u64| ((pct_of(nano) / 100.0) * bar as f64).round() as usize;
    let (c, r) = (cells(savings.compression), cells(routing_saved));
    let r = if routing_saved > 0 { r.max(1) } else { r };
    let rest = bar.saturating_sub(c + r);
    println!(
        "  {}{}{}",
        paint("█".repeat(c), COMPRESSION_COLOR, false),
        paint("█".repeat(r), ROUTING_COLOR, false),
        paint("░".repeat(rest), REMAINING_COLOR, false),
    );
    let legend = |color: &str, name: &str, amount: String, share: Option<f64>| {
        println!(
            "  {} {}{}{}",
            paint("●", color, false),
            style(format!("{name:<20}")).dim(),
            style(format!("{amount:>9}")).bold(),
            share.map_or(String::new(), |p| style(format!("  ({p:.1}%)")).dim().to_string()),
        );
    };
    let routing_amount = match savings.routing {
        Some(r) if r < 0 => format!("+{}", fmt_nano_usd(r.unsigned_abs())),
        Some(r) => fmt_nano_usd(r as u64),
        None => "—".to_string(),
    };
    legend(ROUTING_COLOR, "Routing savings", routing_amount, Some(pct_of(routing_saved)));
    legend(COMPRESSION_COLOR, "Compression savings", fmt_nano_usd(savings.compression), Some(pct_of(savings.compression)));
    legend(REMAINING_COLOR, "Remaining cost", fmt_nano_usd(paid), None);
    println!(
        "  {}",
        style(format!("Without Edgee {} · estimates · USD", fmt_nano_usd(savings.baseline))).dim()
    );

    // ── Usage: the dashboard's KPI row.
    rule("Usage", width);
    println!(
        "  {}{}{}   {}",
        label("cost"),
        paint(fmt_usd(totals.cost_usd), ACCENT, true),
        fmt_delta(delta.and_then(|d| d.total_cost_percent), true),
        style(format!(
            "in {} · out {}",
            fmt_usd(insights.input_cost_usd),
            fmt_usd(insights.output_cost_usd)
        ))
        .dim(),
    );
    let errors = if totals.errors > 0 {
        style(format!("{} error{}", totals.errors, if totals.errors == 1 { "" } else { "s" })).red().to_string()
    } else {
        style("0 errors").dim().to_string()
    };
    let rerouted = if insights.routed_requests > 0 {
        format!(
            " {} {}",
            style("·").dim(),
            paint(format!("⇄ {} rerouted", fmt_compact(insights.routed_requests)), ROUTING_COLOR, false)
        )
    } else {
        String::new()
    };
    println!(
        "  {}{}{}   {}{}",
        label("requests"),
        paint(fmt_compact(totals.requests), ACCENT, true),
        fmt_delta(delta.and_then(|d| d.total_requests_percent), false),
        errors,
        rerouted,
    );
    println!(
        "  {}{}{}   {} {}  {} {}  {} {}",
        label("tokens"),
        style(fmt_compact(insights.total_tokens)).bold(),
        fmt_delta(delta.and_then(|d| d.total_tokens_percent), false),
        style("in").dim(),
        style(fmt_compact(totals.input_tokens)).cyan(),
        style("cached").dim(),
        style(fmt_compact(totals.cached_input_tokens)).green(),
        style("out").dim(),
        style(fmt_compact(totals.output_tokens)).yellow(),
    );
    if let Some(hit) = insights.cache_hit_pct {
        let w = 20;
        let filled = ((hit / 100.0) * w as f64).round() as usize;
        println!(
            "  {}{}{} {}",
            label("cache hit"),
            style("█".repeat(filled)).green(),
            style("░".repeat(w - filled)).dim(),
            style(format!("{hit:.0}%")).green().bold(),
        );
    }
    let latency = match insights.avg_latency_ms {
        Some(ms) => {
            let secs = ms / 1000.0;
            let text = format!("{secs:.1}s");
            if secs < 3.0 {
                style(text).green()
            } else if secs < 10.0 {
                style(text).yellow()
            } else {
                style(text).red()
            }
            .to_string()
        }
        None => style("—").dim().to_string(),
    };
    println!(
        "  {}{}   {} {}",
        label("latency"),
        latency,
        style("· sessions").dim(),
        style(json.sessions).bold(),
    );

    render_breakdown("Top models", &insights.top_models, 5, width, provider_color);
    if insights.coding_agents.iter().any(|a| a.label != "Other") {
        render_breakdown("Coding agents", &insights.coding_agents, 5, width, |l| {
            if l == "Other" { REMAINING_COLOR } else { ACCENT }
        });
    }

    let sessions: Vec<_> = remote
        .sessions
        .iter()
        .take(limit.unwrap_or(RECENT_SESSIONS).clamp(1, 200))
        .collect();
    if !sessions.is_empty() {
        rule("Recent sessions", width);
        let agent_of = |key: &str| {
            remote
                .agents
                .iter()
                .find(|(id, _)| id == key)
                .map_or("—", |(_, a)| a.as_str())
        };
        let agent_width = sessions.iter().map(|s| agent_of(&s.key_uuid).len()).max().unwrap_or(5).max(5);
        println!(
            "  {}  {}  {}  {}  {}  {}",
            style(format!("{:<16}", "last req (UTC)")).dim(),
            style(format!("{:<agent_width$}", "agent")).dim(),
            style(format!("{:>5}", "req")).dim(),
            style(format!("{:>9}", "cost")).dim(),
            style(format!("{:>9}", "saved")).dim(),
            style(format!("{:>3}", "err")).dim(),
        );
        for s in sessions {
            let saved = s.total_tool_compression_cost_savings
                + s.total_mcp_surface_cost_savings
                + s.total_output_cost_savings
                + s.estimated_routing_savings.unwrap_or(0).max(0) as u64;
            println!(
                "  {}  {}  {:>5}  {}  {}  {}",
                style(util::fmt_timestamp(&s.last_request)).dim(),
                paint(format!("{:<agent_width$}", agent_of(&s.key_uuid)), ACCENT, false),
                fmt_compact(s.total_requests),
                style(format!("{:>9}", fmt_nano_usd(s.total_cost))).bold(),
                paint(format!("{:>9}", fmt_nano_usd(saved)), COMPRESSION_COLOR, false),
                if s.total_errors > 0 {
                    style(format!("{:>3}", s.total_errors)).red()
                } else {
                    style(format!("{:>3}", s.total_errors)).dim()
                },
            );
        }
    }

    let dashboard = match remote.org_slug.as_deref() {
        Some(slug) => format!("{}/~/{slug}", crate::config::console_base_url()),
        None => crate::config::console_base_url(),
    };
    println!();
    println!(
        "  {} {}   {} {}",
        paint("↗", ACCENT, true),
        style(dashboard).underlined(),
        paint("--local", ACCENT, true),
        style("this machine's logs").dim(),
    );
    println!();
}

/// Before/after token figures for the three compression strategies, summed
/// over one or many local session logs.
#[derive(Default)]
struct LocalCompression {
    tools_before: u64,
    tools_after: u64,
    surface_before: u64,
    surface_after: u64,
    brevity_requests: u64,
    brevity_rate: f64,
}

impl LocalCompression {
    fn add(&mut self, s: &crate::api::SessionStats) {
        self.tools_before += s.total_uncompressed_tools_tokens;
        self.tools_after += s.total_compressed_tools_tokens;
        self.surface_before += s.total_mcp_surface_tokens_before;
        self.surface_after += s.total_mcp_surface_tokens_after;
        self.brevity_requests += s.total_brevity_requests;
        self.brevity_rate += s.total_brevity_rate;
    }

    fn of<'a>(stats: impl IntoIterator<Item = &'a crate::api::SessionStats>) -> Self {
        let mut c = Self::default();
        stats.into_iter().for_each(|s| c.add(s));
        c
    }

    /// Prints one line per strategy that did something; returns whether any did.
    fn render(&self, width: usize) -> bool {
        let bar = width.saturating_sub(48).clamp(8, 20);
        let mut any = false;
        let mut line = |name: &str, before: u64, after: u64| {
            let Some(pct) = compression_pct(before, after) else {
                return;
            };
            any = true;
            println!(
                "  {} {}{:>7} → {:<7} {} {}",
                paint("●", COMPRESSION_COLOR, false),
                style(format!("{name:<15}")).dim(),
                style(fmt_compact(before)).dim(),
                paint(fmt_compact(after), COMPRESSION_COLOR, false),
                share_bar(pct as f64, bar, COMPRESSION_COLOR),
                paint(format!("{pct}% saved"), COMPRESSION_COLOR, true),
            );
        };
        line("Tool results", self.tools_before, self.tools_after);
        line("Tool surface", self.surface_before, self.surface_after);
        if self.brevity_requests > 0 && self.brevity_rate > 0.0 {
            any = true;
            let pct = (self.brevity_rate / self.brevity_requests as f64 * 100.0).clamp(0.0, 100.0);
            println!(
                "  {} {}{:>7} {:<9} {} {}",
                paint("●", COMPRESSION_COLOR, false),
                style(format!("{:<15}", "Output brevity")).dim(),
                fmt_compact(self.brevity_requests),
                style("requests").dim(),
                share_bar(pct, bar, COMPRESSION_COLOR),
                paint(format!("~{pct:.0}% saved (est.)"), COMPRESSION_COLOR, true),
            );
        }
        any
    }
}

/// Tokens line shared by the totals and the latest session.
fn render_tokens(input: u64, cached: u64, cache_write: u64, output: u64, reasoning: u64) {
    let mut line = format!(
        "  {}{} {}  {} {}",
        style(format!("{:<11}", "tokens")).dim(),
        style("in").dim(),
        style(fmt_compact(input)).cyan(),
        style("cached").dim(),
        style(fmt_compact(cached)).green(),
    );
    if cache_write > 0 {
        line.push_str(&format!(
            "  {} {}",
            style("cache-write").dim(),
            style(fmt_compact(cache_write)).green()
        ));
    }
    line.push_str(&format!(
        "  {} {}",
        style("out").dim(),
        style(fmt_compact(output)).yellow()
    ));
    if reasoning > 0 {
        line.push_str(&format!(
            "  {} {}",
            style("reasoning").dim(),
            style(fmt_compact(reasoning)).yellow()
        ));
    }
    println!("{line}");
}

fn render_cache_hit(input: u64, cached: u64, cache_write: u64) {
    let all = input + cached + cache_write;
    if all == 0 {
        return;
    }
    let hit = cached as f64 * 100.0 / all as f64;
    let w = 20;
    let filled = ((hit / 100.0) * w as f64).round() as usize;
    println!(
        "  {}{}{} {}",
        style(format!("{:<11}", "cache hit")).dim(),
        style("█".repeat(filled)).green(),
        style("░".repeat(w - filled)).dim(),
        style(format!("{hit:.0}%")).green().bold(),
    );
}

fn requests_line(requests: u64, errors: u64, cost_nano: u64) {
    let errors = if errors > 0 {
        style(format!("{errors} error{}", if errors == 1 { "" } else { "s" })).red().to_string()
    } else {
        style("0 errors").dim().to_string()
    };
    println!(
        "  {}{}   {}   {}{}",
        style(format!("{:<11}", "requests")).dim(),
        paint(fmt_compact(requests), ACCENT, true),
        errors,
        style("cost ").dim(),
        paint(fmt_nano_usd(cost_nano), ACCENT, true),
    );
}

fn render_local(logs: &[util::SessionLogEntry], limit: Option<usize>, note: Option<&str>) {
    let width = report_width();
    let totals = compute_totals(logs);
    let total_cost: u64 = logs.iter().map(|e| e.stats.total_cost).sum();

    println!();
    println!(
        "  {}  {}",
        paint("◆ EDGEE", ACCENT, true),
        style(format!("local logs · this machine · {} sessions", logs.len())).dim(),
    );
    if let Some(note) = note {
        println!("  {}", style(note).yellow());
    }

    // ── Totals across every stored session.
    rule("Usage · all sessions", width);
    requests_line(totals.requests, totals.errors, total_cost);
    render_tokens(
        totals.input_tokens,
        totals.cached_input_tokens,
        totals.cache_creation_input_tokens,
        totals.output_tokens,
        totals.reasoning_output_tokens,
    );
    render_cache_hit(
        totals.input_tokens,
        totals.cached_input_tokens,
        totals.cache_creation_input_tokens,
    );

    rule("Compression · all sessions", width);
    if !LocalCompression::of(logs.iter().map(|e| &e.stats)).render(width) {
        println!("  {}", style("No compression recorded yet.").dim());
    }

    // ── Spend per coding agent, from each log's tool name.
    let mut agents: Vec<(String, u64, u64)> = Vec::new();
    for e in logs {
        match agents.iter_mut().find(|(n, ..)| *n == e.tool_name) {
            Some(a) => {
                a.1 += e.stats.total_cost;
                a.2 += e.stats.total_requests;
            }
            None => agents.push((e.tool_name.clone(), e.stats.total_cost, e.stats.total_requests)),
        }
    }
    agents.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    let agents: Vec<Breakdown> = agents
        .into_iter()
        .map(|(label, cost, requests)| Breakdown {
            label,
            cost_usd: nano_usd_to_dollars(cost),
            requests,
            tokens: 0,
            pct: if total_cost > 0 {
                clamp_pct(cost as f64 * 100.0 / total_cost as f64)
            } else {
                0.0
            },
        })
        .collect();
    if agents.len() > 1 {
        render_breakdown("Coding agents", &agents, 5, width, |_| ACCENT);
    }

    // ── Latest session, in full.
    let latest = &logs[0];
    let st = &latest.stats;
    rule("Latest session", width);
    println!(
        "  {}  {}  {}",
        paint(&latest.tool_name, ACCENT, true),
        style(format!("{} UTC", util::fmt_timestamp(&latest.ended_at))).dim(),
        style(latest.visible_session_id()).dim(),
    );
    requests_line(st.total_requests, st.total_errors, st.total_cost);
    render_tokens(
        st.total_input_tokens,
        st.total_cached_input_tokens,
        st.total_cache_creation_input_tokens,
        st.total_output_tokens,
        st.total_reasoning_output_tokens,
    );
    LocalCompression::of([st]).render(width);
    if let Some(tools) = st.tool_compression_stats.as_ref().filter(|t| !t.is_empty()) {
        let mut tools: Vec<_> = tools.iter().collect();
        tools.sort_by_key(|(_, t)| std::cmp::Reverse(t.before));
        let name_width = tools.iter().map(|(n, _)| n.chars().count()).max().unwrap_or(4).clamp(4, 20);
        println!(
            "  {}  {}  {}",
            style(format!("  {:<name_width$}", "tool")).dim(),
            style(format!("{:>5}", "calls")).dim(),
            style(format!("{:>7} → {:<7}", "before", "after")).dim(),
        );
        for (name, t) in tools {
            let pct = compression_pct(t.before, t.after).unwrap_or(0);
            let name: String = name.chars().take(name_width).collect();
            println!(
                "    {}  {:>5}  {:>7} → {:<7} {} {}",
                paint(format!("{name:<name_width$}"), ACCENT, false),
                t.count,
                style(fmt_compact(t.before)).dim(),
                paint(fmt_compact(t.after), COMPRESSION_COLOR, false),
                share_bar(pct as f64, 10, COMPRESSION_COLOR),
                paint(format!("{pct}%"), COMPRESSION_COLOR, true),
            );
        }
    }
    println!(
        "  {} {}",
        paint("↗", ACCENT, true),
        style(&latest.logs_url).underlined()
    );

    // ── Every session (or `--limit`).
    let visible: Vec<_> = logs.iter().take(limit.unwrap_or(logs.len()).max(1)).collect();
    rule("Sessions", width);
    let tool_width = visible.iter().map(|e| e.tool_name.len()).max().unwrap_or(4).max(4);
    println!(
        "  {}  {}  {}  {}  {}  {}  {}  {}",
        style(format!("{:<16}", "ended (UTC)")).dim(),
        style(format!("{:<tool_width$}", "tool")).dim(),
        style(format!("{:>5}", "req")).dim(),
        style(format!("{:>7}", "in")).dim(),
        style(format!("{:>7}", "out")).dim(),
        style(format!("{:>8}", "cost")).dim(),
        style(format!("{:<12}", "compression")).dim(),
        style(format!("{:>3}", "err")).dim(),
    );
    for e in visible {
        let s = &e.stats;
        let compression = match compression_pct(
            s.total_uncompressed_tools_tokens,
            s.total_compressed_tools_tokens,
        ) {
            Some(pct) => format!(
                "{} {}",
                share_bar(pct as f64, 8, COMPRESSION_COLOR),
                paint(format!("{pct:>2}%"), COMPRESSION_COLOR, false)
            ),
            None => format!("{} {}", style("▱".repeat(8)).dim(), style(" -").dim()),
        };
        println!(
            "  {}  {}  {:>5}  {}  {}  {}  {}  {}",
            style(util::fmt_timestamp(&e.ended_at)).dim(),
            paint(format!("{:<tool_width$}", e.tool_name), ACCENT, false),
            fmt_compact(s.total_requests),
            style(format!("{:>7}", fmt_compact(s.total_input_tokens))).cyan(),
            style(format!("{:>7}", fmt_compact(s.total_output_tokens))).yellow(),
            style(format!("{:>8}", fmt_nano_usd(s.total_cost))).bold(),
            compression,
            if s.total_errors > 0 {
                style(format!("{:>3}", s.total_errors)).red()
            } else {
                style(format!("{:>3}", s.total_errors)).dim()
            },
        );
    }
    println!();
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
            "cache_creation_input_tokens",
            "reasoning_output_tokens",
            "cost_usd",
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
            cost_usd: 0.05,
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
            "cost_usd",
            "compression_pct",
            "logs_url",
        ] {
            assert!(bv.get(key).is_some(), "missing session.{key}");
        }
    }

    // The API `/usage` envelope decodes and maps onto the JSON shape.
    #[test]
    fn api_usage_maps_to_stats_json() {
        let envelope = r#"{
            "summary": {
                "total_requests": 241,
                "distinct_sessions": 12,
                "error_requests": 3,
                "input_tokens": 189000,
                "cached_input_tokens": 3300000,
                "cache_creation_input_tokens": 12000,
                "reasoning_output_tokens": 7000,
                "output_tokens": 34000,
                "total_tokens": 3535000,
                "total_cost": 1250000000,
                "input_cost": 1000000000,
                "output_cost": 250000000,
                "avg_duration_ms": 3400.0,
                "token_cost_savings": 42,
                "tool_compression_cost_savings": 100000000,
                "mcp_surface_cost_savings": 50000000,
                "output_cost_savings": 50000000,
                "uncompressed_tools_tokens": 100,
                "compressed_tools_tokens": 60
            },
            "stats_by_model": [
                {"key_uuid": "k1", "model": "claude-opus", "total_cost": 1000000000, "total_requests": 200, "total_tokens": 10},
                {"key_uuid": "k2", "model": "kimi", "total_cost": 250000000, "total_requests": 41, "total_tokens": 5}
            ],
            "stats_by_api_key": [
                {"id": "k1", "type": "coding_agent", "coding_assistant": "claude_code"},
                {"id": "k2", "type": "default"}
            ],
            "stats_by_time": {},
            "delta": {"total_cost_percent": 12.5, "total_requests_percent": -3.0},
            "fallback": {"routed_requests": 9, "estimated_routing_savings": -5, "routing_savings_estimate_count": 2}
        }"#;
        let usage: crate::api::OrgUsage = serde_json::from_str(envelope).unwrap();
        let out = stats_json_from_usage(&usage, "1h", Some(2));
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["source"], "api");
        assert_eq!(v["window"], "1h");
        assert_eq!(v["sessions"], 12);
        assert_eq!(v["active_sessions"], 2);
        assert_eq!(v["totals"]["requests"], 241);
        assert_eq!(v["totals"]["errors"], 3);
        assert_eq!(v["totals"]["cached_input_tokens"], 3_300_000u64);
        assert_eq!(v["totals"]["cache_creation_input_tokens"], 12000);
        assert_eq!(v["totals"]["reasoning_output_tokens"], 7000);
        assert_eq!(v["totals"]["cost_usd"], 1.25);
        assert_eq!(v["totals"]["token_cost_savings"], 42);
        assert_eq!(v["totals"]["compression_pct"], 40); // (100-60)/100
        assert_eq!(v["recent"].as_array().unwrap().len(), 0);

        let i = &v["insights"];
        // Negative routing never adds to savings: 0.2 / (1.25 + 0.2) = 13.8%.
        assert_eq!(i["savings"]["compression"], 200_000_000u64);
        assert_eq!(i["savings"]["routing"], -5);
        assert_eq!(i["savings"]["saved"], 200_000_000u64);
        assert_eq!(i["savings"]["pct"], 13.8);
        assert_eq!(i["cache_hit_pct"], 94.3); // 3.3M / (189k + 3.3M + 12k)
        assert_eq!(i["avg_latency_ms"], 3400.0);
        assert_eq!(i["routed_requests"], 9);
        assert_eq!(i["delta"]["total_cost_percent"], 12.5);
        assert_eq!(i["top_models"][0]["label"], "claude-opus");
        assert_eq!(i["top_models"][0]["pct"], 80.0);
        assert_eq!(i["coding_agents"][0]["label"], "Claude Code");
        assert_eq!(i["coding_agents"][1]["label"], "Other");
    }

    // Go emits `null` for empty slices and may omit delta/fallback entirely.
    #[test]
    fn api_usage_tolerates_nulls_and_missing_fields() {
        let usage: crate::api::OrgUsage = serde_json::from_str(
            r#"{"summary": {"total_cost": 0}, "stats_by_model": null, "stats_by_api_key": null, "delta": null}"#,
        )
        .unwrap();
        let out = stats_json_from_usage(&usage, "7d", None);
        let i = out.insights.unwrap();
        assert_eq!(i.savings.pct, None);
        assert_eq!(i.cache_hit_pct, None);
        assert!(i.top_models.is_empty());
    }

    fn savings_of(total: u64, tool: u64, surface: u64, output: u64, routing: Option<(i64, u64)>) -> Savings {
        let summary = crate::api::OrgUsageSummary {
            total_cost: total,
            tool_compression_cost_savings: tool,
            mcp_surface_cost_savings: surface,
            output_cost_savings: output,
            ..Default::default()
        };
        let fallback = routing.map(|(savings, count)| crate::api::FallbackSummary {
            estimated_routing_savings: Some(savings),
            routing_savings_estimate_count: count,
            ..Default::default()
        });
        compute_savings(&summary, fallback.as_ref())
    }

    // Same vectors as console `dashboard-savings.utils.test.ts`.
    #[test]
    fn savings_match_console_dashboard() {
        let s = savings_of(60, 10, 5, 5, Some((20, 3)));
        assert_eq!((s.compression, s.saved, s.baseline, s.pct), (20, 40, 100, Some(40.0)));

        for added_cost in [-10, -30] {
            let s = savings_of(60, 10, 5, 5, Some((added_cost, 3)));
            assert_eq!((s.routing, s.saved, s.baseline, s.pct), (Some(added_cost), 20, 80, Some(25.0)));
        }

        let s = savings_of(60, 10, 5, 5, None);
        assert_eq!((s.routing, s.saved, s.pct), (None, 20, Some(25.0)));

        // An estimate with no priced requests isn't an estimate.
        let s = savings_of(60, 10, 5, 5, Some((20, 0)));
        assert_eq!(s.routing, None);

        let s = savings_of(0, 0, 0, 0, None);
        assert_eq!((s.saved, s.baseline, s.pct), (0, 0, None));
    }
}
