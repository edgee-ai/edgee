pub mod claude;
pub mod codex;
pub mod opencode;
pub mod statusline;

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

/// Build the per-session "agent instructions" prompt that tells the model to
/// call the Edgee MCP tools (`setSessionName`, `addSessionPullRequest`,
/// optionally `setSessionGitHubRepo`).
///
/// Shared by every `edgee launch <agent>` that wires Edgee MCP. The text
/// references only the bare MCP tool names, so it works regardless of how the
/// host agent namespaces them.
pub fn agent_session_prompt(session_id: &str, repo: Option<&str>, session_url: &str) -> String {
    let mut prompt = format!(
        r#"You are running inside the Edgee CLI and have access to the Edgee MCP server for tracking session metadata.

Your Edgee session ID is: {session_id}
Your Edgee public session page is: {session_url}

You MUST use the following Edgee MCP tools during this session:

1. `setSessionName` — call this immediately after the user's first message with a short descriptive name (3-6 words) summarizing what the user is asking for. Arguments:
   - sessionId: "{session_id}"
   - name: the descriptive name.
   If at any later point during the session you come up with a clearly better name (e.g., the task's real scope becomes obvious only after exploring the code, or the user pivots the request), call `setSessionName` again with the improved name. Prefer calling it once, but do not hesitate to update when a materially better name emerges.

2. `addSessionPullRequest` — call this EVERY TIME you create OR edit a pull request (e.g., via `gh pr create`, `gh pr edit`, or any other tool). Immediately after the PR is created or modified, call this tool with:
   - sessionId: "{session_id}"
   - pullRequest: the full PR URL.
   This is required for every PR you touch during this session, with no exceptions. Always call it on edits too — the PR may not yet be associated with this session, and the API handles duplicates safely, so redundant calls are harmless."#
    );

    if let Some(repo) = repo {
        prompt.push_str(&format!(
            "\n\n3. `setSessionGitHubRepo` — call this EXACTLY ONCE at the start of the session, together with (or right after) `setSessionName`. Arguments:\n   - sessionId: \"{session_id}\"\n   - repo: \"{repo}\"\n   Do not call this tool again during the session."
        ));
    }

    prompt
}

/// Wrap `s` as a TOML basic string literal (including surrounding `"`), escaping
/// the characters TOML requires per the spec. Used to embed multi-line prompts
/// into Codex's `-c key=value` overrides without TOML parse errors.
pub fn toml_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            '\t' => out.push_str(r"\t"),
            '\x08' => out.push_str(r"\b"),
            '\x0c' => out.push_str(r"\f"),
            c if (c as u32) < 0x20 || c as u32 == 0x7F => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_escape_round_trip_through_parser() {
        let prompt = agent_session_prompt(
            "abc-123",
            Some("git@github.com:owner/repo.git"),
            "https://www.edgee.ai/session/abc-123",
        );
        let escaped = toml_escape_string(&prompt);
        let doc = format!("value = {escaped}");
        let parsed: toml::Value =
            toml::from_str(&doc).expect("escaped TOML literal must parse cleanly");
        let round_tripped = parsed
            .get("value")
            .and_then(|v| v.as_str())
            .expect("value must be a string");
        assert_eq!(round_tripped, prompt);
    }

    #[test]
    fn toml_escape_handles_quotes_backslashes_and_control_chars() {
        let raw = "He said \"hi\"\nback\\slash\ttab\x07bell";
        let escaped = toml_escape_string(raw);
        let doc = format!("value = {escaped}");
        let parsed: toml::Value = toml::from_str(&doc).expect("must parse");
        assert_eq!(parsed.get("value").and_then(|v| v.as_str()).unwrap(), raw);
    }

    #[test]
    fn agent_prompt_omits_repo_block_when_absent() {
        let p = agent_session_prompt("sid", None, "https://example/sid");
        assert!(!p.contains("setSessionGitHubRepo"));
    }

    #[test]
    fn agent_prompt_includes_repo_block_when_present() {
        let p = agent_session_prompt("sid", Some("owner/repo"), "https://example/sid");
        assert!(p.contains("setSessionGitHubRepo"));
        assert!(p.contains("owner/repo"));
    }
}
