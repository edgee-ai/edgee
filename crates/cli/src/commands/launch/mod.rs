pub mod claude;
pub mod codex;
pub mod opencode;

use anyhow::{Context, Result};
use console::style;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Resolve a CLI tool binary, using the `which` crate on Windows
/// with an npm global prefix fallback.
pub fn resolve_binary(name: &str) -> std::ffi::OsString {
    #[cfg(not(windows))]
    {
        name.into()
    }

    #[cfg(windows)]
    {
        if let Ok(found) = which::which(name) {
            return found.into_os_string();
        }

        if let Some(npm_bin) = npm_global_bin_dir() {
            for ext in &["cmd", "exe", "ps1"] {
                let candidate = npm_bin.join(format!("{name}.{ext}"));
                if candidate.is_file() {
                    return candidate.into_os_string();
                }
            }
        }

        name.into()
    }
}

#[cfg(windows)]
fn npm_global_bin_dir() -> Option<PathBuf> {
    let output = std::process::Command::new("npm")
        .args(["config", "get", "prefix"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let prefix = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if prefix.is_empty() {
        return None;
    }
    Some(PathBuf::from(prefix))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLogEntry {
    pub session_id: String,
    pub tool_name: String,
    pub ended_at: String,
    pub ended_at_unix: i64,
    pub logs_url: String,
    pub stats: crate::api::SessionStats,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Launch Claude Code routed through Edgee
    Claude(claude::Options),
    /// Launch Codex routed through Edgee
    Codex(codex::Options),
    /// Launch OpenCode routed through Edgee
    #[command(name = "opencode")]
    OpenCode(opencode::Options),
}

#[derive(Debug, clap::Parser)]
pub struct Options {
    #[command(subcommand)]
    command: Command,
}

pub async fn run(opts: Options) -> anyhow::Result<()> {
    match opts.command {
        Command::Claude(o) => claude::run(o).await,
        Command::Codex(o) => codex::run(o).await,
        Command::OpenCode(o) => opencode::run(o).await,
    }
}

fn fmt_tokens(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

pub fn fmt_timestamp(ts: &str) -> String {
    // Convert RFC3339 "2024-01-15T10:30:45+00:00" → "2024-01-15 10:30"
    if ts.len() >= 16 {
        ts[..16].replace('T', " ")
    } else {
        ts.to_string()
    }
}

fn compression_pct(before: u64, after: u64) -> u64 {
    if before == 0 {
        return 0;
    }
    (before - after) * 100 / before
}

fn pad_left(s: &str, width: usize) -> String {
    format!("{s:>width$}")
}

fn pad_right(s: &str, width: usize) -> String {
    format!("{s:<width$}")
}

pub fn fmt_bar(pct: u64, width: usize) -> String {
    let filled = (pct as usize * width / 100).min(width);
    let empty = width - filled;
    format!(
        "{}{}",
        style("█".repeat(filled)).green(),
        style("░".repeat(empty)).dim()
    )
}

pub fn session_logs_dir() -> PathBuf {
    crate::config::config_dir().join("session-stats")
}

fn session_log_path(session_id: &str) -> PathBuf {
    session_logs_dir().join(format!("{session_id}.json"))
}

pub fn logs_url_for_session(creds: &crate::config::Credentials, session_id: &str) -> String {
    match creds.org_slug.as_deref() {
        Some(slug) if !slug.is_empty() => format!(
            "{}/~/{}/sessions/{}",
            crate::config::console_base_url(),
            slug,
            session_id
        ),
        _ => format!(
            "{}/~/me/sessions/{}",
            crate::config::console_base_url(),
            session_id
        ),
    }
}

fn read_session_log(path: &Path) -> Result<SessionLogEntry> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read session log {}", path.display()))?;
    serde_json::from_str(&content)
        .with_context(|| format!("Invalid session log {}", path.display()))
}

pub fn read_all_session_logs() -> Result<Vec<SessionLogEntry>> {
    let dir = session_logs_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        match read_session_log(&path) {
            Ok(log) => entries.push(log),
            Err(_) => continue,
        }
    }

    entries.sort_by_key(|b| std::cmp::Reverse(b.ended_at_unix));
    Ok(entries)
}

fn store_session_log(entry: &SessionLogEntry) -> Result<()> {
    let dir = session_logs_dir();
    fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    let path = session_log_path(&entry.session_id);
    let tmp_path = path.with_extension("tmp");
    let content = serde_json::to_string_pretty(entry)?;
    fs::write(&tmp_path, content)?;
    fs::rename(&tmp_path, &path)?;
    Ok(())
}

fn build_session_log_entry(
    session_id: &str,
    tool_name: &str,
    logs_url: String,
    stats: crate::api::SessionStats,
) -> Result<SessionLogEntry> {
    let now = OffsetDateTime::now_utc();
    Ok(SessionLogEntry {
        session_id: session_id.to_string(),
        tool_name: tool_name.to_string(),
        ended_at: now
            .format(&Rfc3339)
            .context("Failed to format session timestamp")?,
        ended_at_unix: now.unix_timestamp(),
        logs_url,
        stats,
    })
}

pub fn render_session_stats(entry: &SessionLogEntry, heading: Option<&str>) {
    if let Some(heading) = heading {
        println!("  {}", style(heading).bold());
        println!();
    }

    println!(
        "  {} {}  {} {}",
        style("Tool").bold().underlined(),
        style(&entry.tool_name).cyan(),
        style("Ended").bold().underlined(),
        style(fmt_timestamp(&entry.ended_at)).dim(),
    );
    println!(
        "  {} {}",
        style("Session").bold().underlined(),
        style(&entry.session_id).dim()
    );

    let stats = &entry.stats;

    println!();
    let error_note = if stats.total_errors > 0 {
        format!("  ·  {} errors", style(stats.total_errors).red())
    } else {
        String::new()
    };
    println!(
        "  {}  {} requests{}",
        style("Overview").bold().underlined(),
        style(stats.total_requests).cyan(),
        error_note,
    );

    println!();
    println!("  {}", style("Tokens").bold().underlined());

    let mut input_detail = String::new();
    if stats.total_cached_input_tokens > 0 {
        input_detail.push_str(&format!(
            "  cached: {}",
            style(fmt_tokens(stats.total_cached_input_tokens)).dim()
        ));
    }
    if stats.total_cache_creation_input_tokens > 0 {
        input_detail.push_str(&format!(
            "  cache-write: {}",
            style(fmt_tokens(stats.total_cache_creation_input_tokens)).dim()
        ));
    }
    println!(
        "  Input   {}{}",
        style(fmt_tokens(stats.total_input_tokens)).cyan(),
        input_detail,
    );

    let reasoning_note = if stats.total_reasoning_output_tokens > 0 {
        format!(
            "  reasoning: {}",
            style(fmt_tokens(stats.total_reasoning_output_tokens)).dim()
        )
    } else {
        String::new()
    };
    println!(
        "  Output  {}{}",
        style(fmt_tokens(stats.total_output_tokens)).cyan(),
        reasoning_note,
    );

    let has_tool_compression = stats.total_uncompressed_tools_tokens > 0
        && stats.total_compressed_tools_tokens < stats.total_uncompressed_tools_tokens;

    if has_tool_compression {
        println!();
        println!("  {}", style("Compression").bold().underlined());

        let pct = compression_pct(
            stats.total_uncompressed_tools_tokens,
            stats.total_compressed_tools_tokens,
        );
        println!(
            "  Tools   {} -> {}  {} {}% saved",
            style(fmt_tokens(stats.total_uncompressed_tools_tokens)).dim(),
            style(fmt_tokens(stats.total_compressed_tools_tokens)).cyan(),
            fmt_bar(pct, 20),
            style(pct).green(),
        );

        if let Some(tool_stats) = &stats.tool_compression_stats {
            if !tool_stats.is_empty() {
                let mut tools: Vec<_> = tool_stats.iter().collect();
                tools.sort_by_key(|b| std::cmp::Reverse(b.1.before));
                println!();
                println!("  {}", style("Tool breakdown").bold().underlined());
                println!(
                    "  {} {} {} Savings",
                    pad_right("Tool", 20),
                    pad_left("Calls", 5),
                    pad_right("Tokens", 20)
                );
                for (name, ts) in &tools {
                    let pct = compression_pct(ts.before, ts.after);
                    println!(
                        "  {} {} {} -> {} {} {}% saved",
                        style(pad_right(name.as_str(), 20)).cyan(),
                        pad_left(&ts.count.to_string(), 5),
                        style(pad_left(&fmt_tokens(ts.before), 9)).dim(),
                        style(pad_left(&fmt_tokens(ts.after), 9)).cyan(),
                        fmt_bar(pct, 10),
                        style(pct).green(),
                    );
                }
            }
        }
    }

    println!();
    println!(
        "  {} {}",
        style("Full details at").dim(),
        style(&entry.logs_url).cyan().underlined()
    );
    println!();
}

pub async fn print_session_stats(
    creds: &crate::config::Credentials,
    session_id: &str,
    tool_name: &str,
) {
    let logs_url = logs_url_for_session(creds, session_id);

    println!();
    println!(
        "  {} {}",
        style("Session ended.").bold(),
        style(format!("Thanks for using Edgee + {}!", tool_name)).dim()
    );

    // Try to fetch stats; if it fails, just show the URL
    let stats = match fetch_stats(creds, session_id).await {
        Ok(s) => s,
        Err(_) => {
            println!(
                "  {} {}",
                style("View your usage & compression stats at").dim(),
                style(&logs_url).cyan().underlined()
            );
            println!();
            return;
        }
    };

    match build_session_log_entry(session_id, tool_name, logs_url.clone(), stats.clone()) {
        Ok(entry) => {
            let _ = store_session_log(&entry);
            render_session_stats(&entry, None);
        }
        Err(_) => {
            let fallback = SessionLogEntry {
                session_id: session_id.to_string(),
                tool_name: tool_name.to_string(),
                ended_at: "unknown".to_string(),
                ended_at_unix: 0,
                logs_url,
                stats,
            };
            render_session_stats(&fallback, None);
        }
    }
}

async fn fetch_stats(
    creds: &crate::config::Credentials,
    session_id: &str,
) -> anyhow::Result<crate::api::SessionStats> {
    let token = creds
        .user_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow::anyhow!("not authenticated"))?;
    let org_id = creds
        .org_id
        .as_deref()
        .filter(|o| !o.is_empty())
        .ok_or_else(|| anyhow::anyhow!("no org selected"))?;
    let client = crate::api::ApiClient::new(token)?;
    client.get_session_stats(org_id, session_id).await
}

/// Run the user-level Claude integration installer the first time the user
/// launches any agent through Edgee, so they don't have to remember a
/// separate setup step.
///
/// Idempotent and best-effort:
/// - If the disable marker exists, do nothing (user opted out).
/// - If the installed marker exists, do nothing (already ran once).
/// - Otherwise run the installer and create the installed marker.
/// - Any error is logged and swallowed; never blocks the launch.
pub async fn ensure_first_run_installed() {
    use crate::commands::statusline::claude::{install, toggle};

    if toggle::is_disabled() {
        return;
    }

    // Always heal legacy/stale `statusLine.command` values on every launch
    // (silent and cheap). Covers users upgrading from older Edgee versions
    // whose `~/.claude/settings.json` still has the bare `edgee statusline`
    // form (which now prints help) or a wrapper-script path from the old
    // transient install. No-op if the field is already current or third-party.
    install::heal_legacy_statusline();

    let marker = toggle::installed_marker_path();
    if marker.is_file() {
        return;
    }

    if let Err(e) = install::run(install::Options::default()).await {
        eprintln!(
            "  {} edgee: skipped first-run statusline install: {e}",
            console::style("⚠").yellow()
        );
        return;
    }

    if let Some(parent) = marker.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&marker, b"");
}

/// Fire-and-forget: record the running CLI version on the session metadata.
///
/// No-op when the active profile has no user token or no selected org. All
/// errors are swallowed — this is best-effort telemetry and must never block
/// the launch flow or surface output to the user.
pub fn spawn_cli_version_report(creds: &crate::config::Credentials, session_id: &str) {
    let token = creds
        .user_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    let org_id = creds
        .org_id
        .as_deref()
        .filter(|o| !o.is_empty())
        .map(str::to_owned);
    let (Some(token), Some(org_id)) = (token, org_id) else {
        return;
    };
    let session_id = session_id.to_owned();

    tokio::spawn(async move {
        if let Ok(client) = crate::api::ApiClient::new(&token) {
            let _ = client
                .set_session_cli_version(&org_id, &session_id, env!("CARGO_PKG_VERSION"))
                .await;
        }
    });
}
