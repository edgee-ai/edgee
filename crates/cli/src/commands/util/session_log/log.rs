use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::fs;
use std::path::{Path, PathBuf};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLogEntry {
    pub session_id: String,
    pub tool_name: String,
    pub ended_at: String,
    pub ended_at_unix: i64,
    pub logs_url: String,
    pub stats: crate::api::SessionStats,
}

pub fn session_logs_dir() -> PathBuf {
    crate::config::config_dir().join("session-stats")
}

pub fn session_log_path(session_id: &str) -> PathBuf {
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

pub fn read_session_log(path: &Path) -> Result<SessionLogEntry> {
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

    entries.sort_by_key(|b| Reverse(b.ended_at_unix));
    Ok(entries)
}

pub fn store_session_log(entry: &SessionLogEntry) -> Result<()> {
    let dir = session_logs_dir();
    fs::create_dir_all(&dir).with_context(|| format!("Failed to create {}", dir.display()))?;
    let path = session_log_path(&entry.session_id);
    let tmp_path = path.with_extension("tmp");
    let content = serde_json::to_string_pretty(entry)?;
    fs::write(&tmp_path, content)?;
    fs::rename(&tmp_path, &path)?;
    Ok(())
}

pub fn build_session_log_entry(
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
