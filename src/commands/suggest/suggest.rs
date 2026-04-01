use std::collections::{HashMap, HashSet};

use super::loader::{Session, Turn};

const DEP_MIN_LEN: usize = 30;
const BURST_THRESHOLD: usize = 3;
const FREQ_MIN_COUNT: usize = 3;    // absolute: must appear in at least N sessions
const FREQ_MIN_RATIO: f64 = 0.15;   // relative: must be >= 15% of project sessions
const READ_EDIT_WINDOW: usize = 2;

#[derive(Debug)]
pub struct Suggestion {
    pub severity: String,
    pub title: String,
    pub example_after: Option<String>,
}

pub struct ProjectFile {
    pub fname: String,
    pub session_count: usize,
}

pub struct SessionResult {
    pub suggestions: Vec<Suggestion>,
    pub start_date: String,  // "YYYY-MM-DD" from first turn timestamp
    pub max_severity: u8,    // 2=high, 1=medium, 0=low
}

#[allow(dead_code)]
pub struct SuggestResult {
    pub sessions: HashMap<String, SessionResult>,
    pub project_files: HashMap<String, Vec<ProjectFile>>,  // CLAUDE.md candidates per project
    pub total_sessions: usize,
    pub opening_burst_count: usize,
    pub read_then_edit_count: usize,
    pub redundant_read_count: usize,
}

// ── Cross-session repeat detection ───────────────────────────────────────────

pub fn fingerprint_sessions(sessions: &mut Vec<Session>) {
    let mut all_refs: Vec<(usize, usize, String)> = sessions
        .iter()
        .enumerate()
        .flat_map(|(si, session)| {
            session
                .turns
                .iter()
                .enumerate()
                .map(move |(ti, turn)| (si, ti, turn.timestamp.clone()))
        })
        .collect();
    all_refs.sort_by(|a, b| a.2.cmp(&b.2));

    let mut fp_map: HashMap<String, (String, usize)> = HashMap::new();
    for &(si, ti, _) in &all_refs {
        let turn = &sessions[si].turns[ti];
        for block in &turn.content {
            if block.block_type != "text" && block.block_type != "tool_result" {
                continue;
            }
            if let Some(fp) = &block.fingerprint {
                let entry = fp_map
                    .entry(fp.clone())
                    .or_insert_with(|| (turn.uuid.clone(), 0));
                entry.1 += 1;
            }
        }
    }

    for session in sessions.iter_mut() {
        for turn in session.turns.iter_mut() {
            let uuid = turn.uuid.clone();
            for block in turn.content.iter_mut() {
                if block.block_type != "text" && block.block_type != "tool_result" {
                    continue;
                }
                if let Some(fp) = &block.fingerprint {
                    if let Some((first_uuid, count)) = fp_map.get(fp) {
                        if *count > 1 && first_uuid != &uuid {
                            block.is_repeat = true;
                        }
                    }
                }
            }
        }
    }
}

// ── Detection helpers ─────────────────────────────────────────────────────────

fn read_file_paths(turns: &[Turn]) -> Vec<String> {
    let mut paths = Vec::new();
    for turn in turns {
        for block in &turn.content {
            if block.block_type == "tool_use" && block.tool_name.as_deref() == Some("Read") {
                if let Some(input) = &block.tool_input {
                    if let Some(fp) = input["file_path"].as_str() {
                        if !fp.is_empty() {
                            paths.push(fp.to_string());
                        }
                    }
                }
            }
        }
    }
    paths
}

fn opening_burst(turns: &[Turn]) -> usize {
    let mut count = 0;
    for turn in turns {
        if turn.role != "assistant" {
            continue;
        }
        let tool_blocks: Vec<_> = turn
            .content
            .iter()
            .filter(|b| b.block_type == "tool_use")
            .collect();
        let has_text = turn
            .content
            .iter()
            .any(|b| b.block_type == "text" && b.text.is_some());

        if has_text {
            break;
        }
        if !tool_blocks.is_empty()
            && tool_blocks
                .iter()
                .all(|b| b.tool_name.as_deref() == Some("Read"))
        {
            count += tool_blocks.len();
        } else {
            break;
        }
    }
    count
}

fn read_then_edit_pairs(turns: &[Turn]) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for (i, turn) in turns.iter().enumerate() {
        if turn.role != "assistant" {
            continue;
        }
        for block in &turn.content {
            if block.block_type != "tool_use" || block.tool_name.as_deref() != Some("Read") {
                continue;
            }
            let read_path = match block
                .tool_input
                .as_ref()
                .and_then(|inp| inp["file_path"].as_str())
            {
                Some(p) if !p.is_empty() => p.to_string(),
                _ => continue,
            };

            let result_text: Option<String> = (i + 1 < turns.len())
                .then(|| {
                    turns[i + 1].content.iter().find(|rb| {
                        rb.block_type == "tool_result" && rb.tool_use_id == block.tool_use_id
                    })
                })
                .flatten()
                .and_then(|rb| rb.text.clone());

            let window_end = (i + 1 + READ_EDIT_WINDOW * 2 + 1).min(turns.len());
            'outer: for j in (i + 1)..window_end {
                let edit_turn = &turns[j];
                if edit_turn.role != "assistant" {
                    continue;
                }
                for eb in &edit_turn.content {
                    if eb.block_type != "tool_use" || eb.tool_name.as_deref() != Some("Edit") {
                        continue;
                    }
                    let edit_path = eb
                        .tool_input
                        .as_ref()
                        .and_then(|inp| inp["file_path"].as_str())
                        .unwrap_or("");
                    if edit_path != read_path {
                        continue;
                    }
                    let old_string = eb
                        .tool_input
                        .as_ref()
                        .and_then(|inp| inp["old_string"].as_str())
                        .unwrap_or("");
                    if old_string.len() >= DEP_MIN_LEN {
                        if let Some(ref rt) = result_text {
                            let prefix = &old_string[..DEP_MIN_LEN];
                            if rt.contains(prefix) && !seen.contains(&read_path) {
                                files.push(read_path.clone());
                                seen.insert(read_path.clone());
                                break 'outer;
                            }
                        }
                    }
                }
            }
        }
    }
    files
}

/// Returns (file_path, total_read_count) for files read 2+ times with
/// identical content and no intervening Edit between the repeated reads.
pub fn redundant_reads(turns: &[Turn]) -> Vec<(String, usize)> {
    // Pre-build: tool_use_id → file_path for every Read call
    let mut read_id_to_path: HashMap<String, String> = HashMap::new();
    for turn in turns {
        for block in &turn.content {
            if block.block_type == "tool_use" && block.tool_name.as_deref() == Some("Read") {
                if let (Some(id), Some(path)) = (
                    &block.tool_use_id,
                    block.tool_input.as_ref().and_then(|i| i["file_path"].as_str()),
                ) {
                    if !path.is_empty() {
                        read_id_to_path.insert(id.clone(), path.to_string());
                    }
                }
            }
        }
    }

    struct FileState {
        last_fp: Option<String>, // fingerprint of the last Read result
        edited_since: bool,      // was there an Edit between last read and now?
        total_reads: usize,
        redundant: usize,
    }

    let mut state: HashMap<String, FileState> = HashMap::new();

    for turn in turns {
        for block in &turn.content {
            match block.block_type.as_str() {
                // An Edit resets the "safe to compare" window for that file
                "tool_use" if block.tool_name.as_deref() == Some("Edit") => {
                    if let Some(path) =
                        block.tool_input.as_ref().and_then(|i| i["file_path"].as_str())
                    {
                        if let Some(s) = state.get_mut(path) {
                            s.edited_since = true;
                        }
                    }
                }
                // A Read result: compare fingerprint to previous read of same file
                "tool_result" => {
                    if let Some(id) = &block.tool_use_id {
                        if let Some(path) = read_id_to_path.get(id) {
                            let fp = block.fingerprint.clone();
                            let s = state.entry(path.clone()).or_insert(FileState {
                                last_fp: None,
                                edited_since: false,
                                total_reads: 0,
                                redundant: 0,
                            });
                            s.total_reads += 1;
                            if !s.edited_since {
                                if let (Some(last), Some(current)) = (&s.last_fp, &fp) {
                                    if last == current {
                                        s.redundant += 1;
                                    }
                                }
                            }
                            s.last_fp = fp;
                            s.edited_since = false;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let mut result: Vec<(String, usize)> = state
        .into_iter()
        .filter(|(_, s)| s.redundant > 0)
        .map(|(path, s)| (path, s.total_reads))
        .collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn generate_suggestions(sessions: &[Session]) -> SuggestResult {
    // Total sessions per project (for relative threshold)
    let mut project_session_count: HashMap<String, usize> = HashMap::new();
    for session in sessions {
        *project_session_count.entry(session.project.clone()).or_default() += 1;
    }

    // Collect per-project file → session set (for CLAUDE.md candidates)
    let mut project_file_sessions: HashMap<String, HashMap<String, HashSet<String>>> =
        HashMap::new();
    for session in sessions {
        for fp in read_file_paths(&session.turns) {
            project_file_sessions
                .entry(session.project.clone())
                .or_default()
                .entry(fp)
                .or_default()
                .insert(session.session_id.clone());
        }
    }

    // Build project_files: CLAUDE.md candidates per project
    let mut project_files: HashMap<String, Vec<ProjectFile>> = HashMap::new();
    for (project, freq_map) in &project_file_sessions {
        let total_project_sessions = *project_session_count.get(project).unwrap_or(&1);
        let mut candidates: Vec<ProjectFile> = freq_map
            .iter()
            .filter_map(|(fp, session_set)| {
                let count = session_set.len();
                let ratio = count as f64 / total_project_sessions as f64;
                if count < FREQ_MIN_COUNT || ratio < FREQ_MIN_RATIO {
                    return None;
                }
                let fname = fp.split('/').next_back().unwrap_or(fp).to_string();
                Some(ProjectFile {
                    fname,
                    session_count: count,
                })
            })
            .collect();
        candidates.sort_by(|a, b| b.session_count.cmp(&a.session_count).then(a.fname.cmp(&b.fname)));
        if !candidates.is_empty() {
            project_files.insert(project.clone(), candidates);
        }
    }

    let mut session_results: HashMap<String, SessionResult> = HashMap::new();
    let mut opening_burst_count = 0usize;
    let mut read_then_edit_count = 0usize;
    let mut redundant_read_count = 0usize;

    for session in sessions {
        let mut suggestions: Vec<Suggestion> = Vec::new();

        let burst = opening_burst(&session.turns);
        if burst >= BURST_THRESHOLD {
            opening_burst_count += 1;
            let read_paths = read_file_paths(&session.turns);
            let examples: Vec<String> = read_paths
                .iter()
                .take(4)
                .map(|p| format!("@{}", p.split('/').next_back().unwrap_or(p)))
                .collect();
            suggestions.push(Suggestion {
                severity: if burst >= 5 { "high" } else { "medium" }.to_string(),
                title: format!("{} consecutive reads at session open", burst),
                example_after: Some(format!(
                    "\"@loader.py — explain how data flows.\"  ({})",
                    examples.join("  ")
                )),
            });
        }

        let rte_pairs = read_then_edit_pairs(&session.turns);
        read_then_edit_count += rte_pairs.len();
        for fp in rte_pairs {
            let fname = fp.split('/').next_back().unwrap_or(&fp).to_string();
            suggestions.push(Suggestion {
                severity: "medium".to_string(),
                title: format!("Read → Edit on {}", fname),
                example_after: Some(format!("\"@{} — fix the bug.\"", fname)),
            });
        }

        let rr_pairs = redundant_reads(&session.turns);
        redundant_read_count += rr_pairs.len();
        for (fp, total_reads) in rr_pairs {
            let fname = fp.split('/').next_back().unwrap_or(&fp).to_string();
            suggestions.push(Suggestion {
                severity: "medium".to_string(),
                title: format!("{} read {}× with unchanged content", fname, total_reads),
                example_after: Some(format!(
                    "@{} — pass the content once instead of re-reading",
                    fname
                )),
            });
        }

        // frequent_read: flag files in this session that qualify as CLAUDE.md candidates
        if let Some(candidates) = project_files.get(&session.project) {
            let candidate_fnames: HashSet<&str> =
                candidates.iter().map(|pf| pf.fname.as_str()).collect();
            let session_reads = read_file_paths(&session.turns);
            let mut flagged: HashSet<String> = HashSet::new();
            for fp in &session_reads {
                let fname = fp.split('/').next_back().unwrap_or(fp);
                if candidate_fnames.contains(fname) && flagged.insert(fname.to_string()) {
                    let count = candidates
                        .iter()
                        .find(|pf| pf.fname == fname)
                        .map(|pf| pf.session_count)
                        .unwrap_or(0);
                    suggestions.push(Suggestion {
                        severity: "low".to_string(),
                        title: format!("{} read in {} sessions — add to CLAUDE.md", fname, count),
                        example_after: Some(format!(
                            "Add @{} to CLAUDE.md so Claude loads it automatically",
                            fname
                        )),
                    });
                }
            }
        }

        if !suggestions.is_empty() {
            let start_date = session
                .turns
                .first()
                .map(|t| t.timestamp.get(..10).unwrap_or("unknown").to_string())
                .unwrap_or_default();
            let max_severity = suggestions
                .iter()
                .map(|s| match s.severity.as_str() {
                    "high" => 2u8,
                    "medium" => 1,
                    _ => 0,
                })
                .max()
                .unwrap_or(0);
            session_results.insert(
                session.session_id.clone(),
                SessionResult {
                    suggestions,
                    start_date,
                    max_severity,
                },
            );
        }
    }

    SuggestResult {
        total_sessions: sessions.len(),
        sessions: session_results,
        project_files,
        opening_burst_count,
        read_then_edit_count,
        redundant_read_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::suggest::loader::{ContentBlock, Turn};

    fn make_turn(role: &str, blocks: Vec<ContentBlock>) -> Turn {
        Turn {
            uuid: uuid::Uuid::new_v4().to_string(),
            role: role.to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            content: blocks,
        }
    }

    fn read_use(id: &str, path: &str) -> ContentBlock {
        ContentBlock {
            block_type: "tool_use".to_string(),
            text: None,
            tool_use_id: Some(id.to_string()),
            tool_name: Some("Read".to_string()),
            tool_input: Some(serde_json::json!({"file_path": path})),
            fingerprint: None,
            is_repeat: false,
        }
    }

    fn read_result(id: &str, content: &str) -> ContentBlock {
        ContentBlock {
            block_type: "tool_result".to_string(),
            text: Some(content.to_string()),
            tool_use_id: Some(id.to_string()),
            tool_name: None,
            tool_input: None,
            fingerprint: Some(format!("fp-{}", content)),
            is_repeat: false,
        }
    }

    fn edit_use(path: &str) -> ContentBlock {
        ContentBlock {
            block_type: "tool_use".to_string(),
            text: None,
            tool_use_id: Some("edit-1".to_string()),
            tool_name: Some("Edit".to_string()),
            tool_input: Some(serde_json::json!({"file_path": path})),
            fingerprint: None,
            is_repeat: false,
        }
    }

    #[test]
    fn detects_redundant_read() {
        let turns = vec![
            make_turn("assistant", vec![read_use("r1", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r1", "content-A")]),
            make_turn("assistant", vec![read_use("r2", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r2", "content-A")]), // same → redundant
        ];
        let result = redundant_reads(&turns);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "/project/foo.py");
        assert_eq!(result[0].1, 2);
    }

    #[test]
    fn no_flag_after_edit() {
        let turns = vec![
            make_turn("assistant", vec![read_use("r1", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r1", "content-A")]),
            make_turn("assistant", vec![edit_use("/project/foo.py")]),
            make_turn("assistant", vec![read_use("r2", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r2", "content-A")]), // same content but edit happened
        ];
        let result = redundant_reads(&turns);
        assert!(result.is_empty());
    }

    #[test]
    fn no_flag_when_content_changed() {
        let turns = vec![
            make_turn("assistant", vec![read_use("r1", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r1", "content-A")]),
            make_turn("assistant", vec![read_use("r2", "/project/foo.py")]),
            make_turn("user",      vec![read_result("r2", "content-B")]), // different
        ];
        let result = redundant_reads(&turns);
        assert!(result.is_empty());
    }
}
